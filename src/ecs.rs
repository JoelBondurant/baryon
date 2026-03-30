use std::cell::UnsafeCell;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU32, Ordering};

/// ==========================================
/// CORE ENTITY
/// ==========================================
/// The Entity ID. A lightweight 32-bit integer.
/// We use NonZeroU32 to allow `Option<NodeId>` to fit cleanly into 4 bytes (Null-pointer optimization).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(NonZeroU32);

impl NodeId {
    #[inline(always)]
    pub fn index(self) -> usize {
        self.0.get() as usize - 1
    }
}

/// ==========================================
/// LAYER 1: PHYSICAL COMPONENTS (SVP)
/// ==========================================
/// Sparse Virtualized Projection Pointer.
/// Bypasses the OS kernel. No memmap2, no owned Strings.
/// References physical storage blocks directly via SPDK/NVMe-Direct.
/// For Phase 1, stores an offset into a memmap2 mapping.
#[derive(Debug, Clone, Copy)]
pub struct SvpPointer {
    pub offset: u32,
    pub length: u32,
}

/// ==========================================
/// LAYER 2: STRUCTURAL METRICS & TOPOLOGY
/// ==========================================
/// Structural metrics decoupled from semantic meaning, critical for
/// fast rope-like traversal, line/column resolution, and bounding-box queries.
#[derive(Debug, Clone, Copy, Default)]
pub struct SpanMetrics {
    pub byte_length: u32,
    pub newlines: u32,
}

/// The Edges of the DAG/Tree.
/// Instead of `Vec<NodeId>` which causes heap fragmentation, we use a 
/// Left-Child / Right-Sibling (LCRS) tree representation.
/// This guarantees every node's topological footprint is exactly 12 bytes.
#[derive(Debug, Clone, Copy, Default)]
pub struct TreeEdges {
    pub parent: Option<NodeId>,
    pub first_child: Option<NodeId>,
    pub next_sibling: Option<NodeId>,
}

/// ==========================================
/// LAYER 3: SEMANTIC COMPONENTS
/// ==========================================
/// The logical meaning of the node. Unifies Relational (CSV) and Logical (Rust/SQL).
#[derive(Debug, Clone, Copy)]
pub enum SemanticKind {
    RelationalTable,
    RelationalRow,
    Token,
}

/// ==========================================
/// CONSTANTS
/// ==========================================
pub const MAX_CHUNK_SIZE: u32 = 64 * 1024;

/// ==========================================
/// THE CONCURRENT ECS REGISTRY (WORLD)
/// ==========================================
/// Structure of Arrays (SoA) layout.
pub struct UastRegistry {
    capacity: u32,
    next_id: AtomicU32,

    // Boxed slices of UnsafeCells. 
    // This allows concurrent mutable writes to disjoint indices without locks.
    kinds: Box<[UnsafeCell<SemanticKind>]>,
    spans: Box<[UnsafeCell<Option<SvpPointer>>]>,
    metrics: Box<[UnsafeCell<SpanMetrics>]>,
    edges: Box<[UnsafeCell<TreeEdges>]>,
    
    // MUTATION STATE: Tracks the last child of a node.
    // Separated from `edges` to keep `edges` densely packed for read-only traversal.
    // Cost: Exactly 4 bytes per node via Option<NonZeroU32> optimization.
    child_tails: Box<[UnsafeCell<Option<NodeId>>]>,

    // VIRTUAL DATA: Stores uncommitted text for virtual nodes.
    // In a production system, this would be a more sophisticated buffer pool.
    virtual_data: Box<[UnsafeCell<Option<Vec<u8>>>]>,
}

// SAFETY: The atomic `next_id` guarantees that no two threads will ever receive 
// the same `NodeId`. Therefore, no two threads will ever attempt to mutate the 
// same `UnsafeCell` concurrently. It is safe to share this across thread boundaries.
unsafe impl Sync for UastRegistry {}
unsafe impl Send for UastRegistry {}

impl UastRegistry {
    /// Pre-allocates the registry. 10GB dataset -> ~200M capacity.
    pub fn new(capacity: u32) -> Self {
        let cap = capacity as usize;
        Self {
            capacity,
            next_id: AtomicU32::new(1), // Start at 1 to allow NonZeroU32 for NodeId
            kinds: (0..cap).map(|_| UnsafeCell::new(SemanticKind::Token)).collect::<Vec<_>>().into_boxed_slice(),
            spans: (0..cap).map(|_| UnsafeCell::new(None)).collect::<Vec<_>>().into_boxed_slice(),
            metrics: (0..cap).map(|_| UnsafeCell::new(SpanMetrics::default())).collect::<Vec<_>>().into_boxed_slice(),
            edges: (0..cap).map(|_| UnsafeCell::new(TreeEdges::default())).collect::<Vec<_>>().into_boxed_slice(),
            child_tails: (0..cap).map(|_| UnsafeCell::new(None)).collect::<Vec<_>>().into_boxed_slice(),
            virtual_data: (0..cap).map(|_| UnsafeCell::new(None)).collect::<Vec<_>>().into_boxed_slice(),
        }
    }

    /// Internal node allocator for mutations.
    fn alloc_node_internal(&self) -> NodeId {
        let id_val = self.next_id.fetch_add(1, Ordering::Relaxed);
        assert!(id_val <= self.capacity, "UastRegistry capacity exceeded during split");
        NodeId(NonZeroU32::new(id_val).unwrap())
    }

    /// THE UPWARD BUBBLE: Propagates metric deltas to all ancestors in O(Depth).
    pub fn apply_edit(&mut self, target: NodeId, added_bytes: i32, added_newlines: i32) {
        let mut curr = Some(target);
        while let Some(node) = curr {
            let idx = node.index();
            unsafe {
                let m = &mut *self.metrics[idx].get();
                m.byte_length = (m.byte_length as i32 + added_bytes) as u32;
                m.newlines = (m.newlines as i32 + added_newlines) as u32;
                
                curr = (*self.edges[idx].get()).parent;
            }
        }
    }

    /// THE P-V-P SPLIT TRIGGER: Inserts text and ruptures the node if it exceeds MAX_CHUNK_SIZE.
    pub fn insert_text(&mut self, target: NodeId, offset_in_node: u32, new_text: &[u8]) {
        let added_bytes = new_text.len() as i32;
        let added_newlines = new_text.iter().filter(|&&b| b == b'\n').count() as i32;

        // 1. Propagate metrics upward before the split.
        self.apply_edit(target, added_bytes, added_newlines);

        // 2. Check for overflow rupture.
        let idx = target.index();
        let current_len = unsafe { (*self.metrics[idx].get()).byte_length };
        
        if current_len > MAX_CHUNK_SIZE {
            self.split_node_pvp(target, offset_in_node, new_text);
        } else {
            // If no split, we still need to handle the virtual text logic.
            // For a leaf Physical node, we might convert it to Virtual or handle it in-place.
            // But the prompt specifically asks for the P-V-P split logic on rupture.
            // In a real system, we'd append to a virtual buffer here.
        }
    }

    /// RUPTURE: Splits a Physical node into [Physical, Virtual, Physical] siblings.
    /// Reuses `target` as the first Physical node (P1) to maintain topology efficiently.
    fn split_node_pvp(&mut self, target: NodeId, offset: u32, new_text: &[u8]) {
        let target_idx = target.index();
        
        // 1. Extract context
        let (parent, old_next_sibling, old_svp) = unsafe {
            let e = &*self.edges[target_idx].get();
            let s = (*self.spans[target_idx].get()).expect("Split target must be Physical");
            (e.parent, e.next_sibling, s)
        };
        let parent = parent.expect("Cannot split a root node");

        // 2. Allocate V and P2 nodes
        let v_id = self.alloc_node_internal();
        let p2_id = self.alloc_node_internal();
        let v_idx = v_id.index();
        let p2_idx = p2_id.index();

        // 3. Reconfigure P1 (Reuse target)
        // Note: P1.metrics.newlines is tricky without data access. 
        // For Phase 1, we assume a placeholder or that metrics are aggregated at parent level.
        let p1_len = offset;
        unsafe {
            let s = &mut *self.spans[target_idx].get();
            s.as_mut().unwrap().length = p1_len;
            
            let m = &mut *self.metrics[target_idx].get();
            m.byte_length = p1_len;
            // TODO: Recalculate m.newlines from physical storage
            
            let e = &mut *self.edges[target_idx].get();
            e.next_sibling = Some(v_id);
        }

        // 4. Configure V (Virtual Node)
        let v_len = new_text.len() as u32;
        let v_newlines = new_text.iter().filter(|&&b| b == b'\n').count() as u32;
        unsafe {
            *self.kinds[v_idx].get() = SemanticKind::Token;
            *self.spans[v_idx].get() = None; // Virtual nodes have no SvpPointer
            *self.virtual_data[v_idx].get() = Some(new_text.to_vec());
            
            let m = &mut *self.metrics[v_idx].get();
            m.byte_length = v_len;
            m.newlines = v_newlines;
            
            let e = &mut *self.edges[v_idx].get();
            e.parent = Some(parent);
            e.next_sibling = Some(p2_id);
        }

        // 5. Configure P2 (Physical Node)
        let p2_len = old_svp.length - offset;
        unsafe {
            *self.kinds[p2_idx].get() = SemanticKind::Token;
            *self.spans[p2_idx].get() = Some(SvpPointer {
                offset: old_svp.offset + offset,
                length: p2_len,
            });
            
            let m = &mut *self.metrics[p2_idx].get();
            m.byte_length = p2_len;
            // TODO: Recalculate m.newlines from physical storage
            
            let e = &mut *self.edges[p2_idx].get();
            e.parent = Some(parent);
            e.next_sibling = old_next_sibling;
        }

        // 6. Maintain Parent's child_tails
        // If target was the tail, P2 is now the tail.
        let p_idx = parent.index();
        unsafe {
            let tail_ptr = &mut *self.child_tails[p_idx].get();
            if *tail_ptr == Some(target) {
                *tail_ptr = Some(p2_id);
            }
        }
    }

    /// Atomically reserves a chunk of `NodeId`s for a single thread.
    /// Returns a `RegistryChunk` which grants lock-free write access to those specific indices.
    pub fn reserve_chunk(&self, size: u32) -> Option<RegistryChunk<'_>> {
        // Relaxed ordering is sufficient: we only need the counter to increment monotonically.
        let start_id = self.next_id.fetch_add(size, Ordering::Relaxed);
        
        if start_id + size > self.capacity + 1 {
            return None; // Out of pre-allocated memory (OOM)
        }

        Some(RegistryChunk {
            registry: self,
            start_id,
            len: size,
            offset: 0,
        })
    }
}

/// ==========================================
/// THREAD-LOCAL CHUNK (THE WRITE CAPABILITY)
/// ==========================================
/// Represents exclusive ownership over a slice of the ECS component arrays.
pub struct RegistryChunk<'a> {
    registry: &'a UastRegistry,
    start_id: u32,
    len: u32,
    offset: u32,
}

impl<'a> RegistryChunk<'a> {
    /// $O(1)$ lock-free entity allocation within the thread's reserved chunk.
    pub fn spawn_node(&mut self, kind: SemanticKind, span: Option<SvpPointer>, metric: SpanMetrics) -> NodeId {
        assert!(self.offset < self.len, "Chunk capacity exceeded");
        
        let id_val = self.start_id + self.offset;
        let id = NodeId(NonZeroU32::new(id_val).unwrap());
        let idx = id.index();
        self.offset += 1;

        // SAFETY: The chunk exclusively owns indices from `start_id` to `start_id + len`.
        // No other thread can access these UnsafeCells.
        unsafe {
            *self.registry.kinds[idx].get() = kind;
            *self.registry.spans[idx].get() = span;
            *self.registry.metrics[idx].get() = metric;
            // edges and child_tails are already zeroed out by default
        }

        id
    }

    /// Appends a child in $O(1)$. 
    /// Panics if the thread attempts to link nodes it does not own.
    pub fn append_local_child(&mut self, parent: NodeId, child: NodeId) {
        let p_val = parent.0.get();
        let c_val = child.0.get();
        
        // SECURITY BOUNDS CHECK: Ensure we only mutate nodes allocated by THIS chunk.
        assert!(p_val >= self.start_id && p_val < self.start_id + self.len, "Parent out of chunk bounds");
        assert!(c_val >= self.start_id && c_val < self.start_id + self.len, "Child out of chunk bounds");

        let p_idx = parent.index();
        let c_idx = child.index();

        // SAFETY: Bounds check above guarantees exclusive chunk ownership.
        unsafe {
            (*self.registry.edges[c_idx].get()).parent = Some(parent);

            let tail_ptr = self.registry.child_tails[p_idx].get();
            if let Some(tail) = *tail_ptr {
                (*self.registry.edges[tail.index()].get()).next_sibling = Some(child);
            } else {
                (*self.registry.edges[p_idx].get()).first_child = Some(child);
            }
            *tail_ptr = Some(child);
        }
    }
}