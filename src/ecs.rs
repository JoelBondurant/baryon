use std::cell::UnsafeCell;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// ==========================================
/// CORE ENTITY
/// ==========================================
/// The Entity ID. A lightweight 32-bit integer.
/// We use NonZeroU32 to allow `Option<NodeId>` to fit cleanly into 4 bytes (Null-pointer optimization).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(pub NonZeroU32);

impl NodeId {
	#[inline(always)]
	pub fn index(self) -> usize {
		self.0.get() as usize - 1
	}

	pub fn from_index(idx: usize) -> Self {
		Self(NonZeroU32::new(idx as u32 + 1).unwrap())
	}
}

/// ==========================================
/// LAYER 1: PHYSICAL COMPONENTS (SVP)
/// ==========================================
/// Sparse Virtualized Projection Pointer.
/// Bypasses the OS kernel. No memmap2, no owned Strings.
/// References physical storage blocks directly via SPDK/NVMe-Direct.
#[repr(C, align(16))]
#[derive(Debug, Clone, Copy)]
pub struct SvpPointer {
	pub lba: u64,
	pub byte_length: u32,
	pub device_id: u16,
	pub head_trim: u16,
}

/// ==========================================
/// LAYER 2: STRUCTURAL METRICS & TOPOLOGY
/// ==========================================
/// Structural metrics decoupled from semantic meaning, critical for
/// fast rope-like traversal, line/column resolution, and bounding-box queries.
#[derive(Debug, Clone, Copy, Default)]
pub struct SpanMetrics {
	#[allow(dead_code)]
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
/// SEMANTIC COMPONENTS
/// ==========================================
/// The logical meaning of the node. Unifies Relational (CSV) and Logical (Rust/SQL).
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub enum SemanticKind {
	RelationalTable,
	RelationalRow,
	Token,
}

/// ==========================================
/// VIEWPORT PROJECTION
/// ==========================================
#[derive(Debug)]
pub struct RenderToken {
	pub node_id: NodeId,
	pub kind: SemanticKind,
	pub text: String,
	#[allow(dead_code)]
	pub absolute_start_line: u32,
	pub is_virtual: bool,
}

/// ==========================================
/// THE CONCURRENT ECS REGISTRY (WORLD)
/// ==========================================
/// Structure of Arrays (SoA) layout.
pub struct UastRegistry {
	pub capacity: u32,
	pub next_id: AtomicU32,

	// Boxed slices of UnsafeCells.
	// This allows concurrent mutable writes to disjoint indices without locks.
	pub kinds: Box<[UnsafeCell<SemanticKind>]>,
	pub spans: Box<[UnsafeCell<Option<SvpPointer>>]>,
	pub metrics: Box<[UnsafeCell<SpanMetrics>]>,
	pub edges: Box<[UnsafeCell<TreeEdges>]>,

	// MUTATION STATE: Tracks the last child of a node.
	// Separated from `edges` to keep `edges` densely packed for read-only traversal.
	// Cost: Exactly 4 bytes per node via Option<NonZeroU32> optimization.
	#[allow(dead_code)]
	child_tails: Box<[UnsafeCell<Option<NodeId>>]>,

	// VIRTUAL DATA: Stores uncommitted text for virtual nodes.
	// In a production system, this would be a more sophisticated buffer pool.
	pub virtual_data: Box<[UnsafeCell<Option<Vec<u8>>>]>,

	/// Tracks nodes currently being resolved by the I/O thread.
	pub dma_in_flight: Box<[AtomicBool]>,
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
			kinds: (0..cap)
				.map(|_| UnsafeCell::new(SemanticKind::Token))
				.collect::<Vec<_>>()
				.into_boxed_slice(),
			spans: (0..cap)
				.map(|_| UnsafeCell::new(None))
				.collect::<Vec<_>>()
				.into_boxed_slice(),
			metrics: (0..cap)
				.map(|_| UnsafeCell::new(SpanMetrics::default()))
				.collect::<Vec<_>>()
				.into_boxed_slice(),
			edges: (0..cap)
				.map(|_| UnsafeCell::new(TreeEdges::default()))
				.collect::<Vec<_>>()
				.into_boxed_slice(),
			child_tails: (0..cap)
				.map(|_| UnsafeCell::new(None))
				.collect::<Vec<_>>()
				.into_boxed_slice(),
			virtual_data: (0..cap)
				.map(|_| UnsafeCell::new(None))
				.collect::<Vec<_>>()
				.into_boxed_slice(),
			dma_in_flight: (0..cap)
				.map(|_| AtomicBool::new(false))
				.collect::<Vec<_>>()
				.into_boxed_slice(),
		}
	}

	/// Resolves either physical SvpPointer or virtual_data into a String.
	pub fn resolve_physical_bytes(&self, node: NodeId) -> String {
		let idx = node.index();
		unsafe {
			if let Some(v_data) = &*self.virtual_data[idx].get() {
				return String::from_utf8_lossy(v_data).to_string();
			}
			// Physical nodes return empty strings until the resolver hot-swaps them.
		}
		String::new()
	}

	/// VIEWPORT QUERY: O(log N) descent + non-recursive iteration.
	pub fn query_viewport(
		&self,
		root: NodeId,
		target_line: u32,
		line_count: u32,
	) -> Vec<RenderToken> {
		let mut tokens = Vec::new();
		let mut curr = Some(root);
		let mut line_accumulator = 0;

		// PHASE 1: DESCENT (O(log N))
		// Skip siblings and drop into children until we find the leaf where target_line starts.
		while let Some(node) = curr {
			let idx = node.index();
			let m = unsafe { &*self.metrics[idx].get() };

			if line_accumulator + m.newlines >= target_line {
				// Target line is in this subtree.
				let first_child = unsafe { (*self.edges[idx].get()).first_child };
				if let Some(child) = first_child {
					curr = Some(child);
					continue;
				} else {
					// Reached the leaf.
					break;
				}
			} else {
				// Target line is after this subtree.
				line_accumulator += m.newlines;
				let next_sibling = unsafe { (*self.edges[idx].get()).next_sibling };
				curr = next_sibling;
			}
		}

		// PHASE 2: ITERATION
		// Non-recursive walk forward from the found leaf.
		let mut collected_lines = 0;
		let mut visit = curr;

		while let Some(node) = visit {
			let idx = node.index();
			let text = self.resolve_physical_bytes(node);

			let is_virtual = unsafe { (*self.spans[idx].get()).is_none() };
			let kind = unsafe { *self.kinds[idx].get() };

			tokens.push(RenderToken {
				node_id: node,
				kind,
				text: text.clone(),
				absolute_start_line: line_accumulator,
				is_virtual,
			});

			let lines_in_token = text.chars().filter(|&c| c == '\n').count() as u32;
			line_accumulator += lines_in_token;
			collected_lines += lines_in_token;

			if collected_lines >= line_count {
				break;
			}

			// Move to next node in tree
			visit = self.get_next_node_in_walk(node);
		}

		tokens
	}

	fn get_next_node_in_walk(&self, node: NodeId) -> Option<NodeId> {
		unsafe {
			if let Some(child) = (*self.edges[node.index()].get()).first_child {
				return Some(child);
			}

			let mut curr = node;
			loop {
				if let Some(sib) = (*self.edges[curr.index()].get()).next_sibling {
					return Some(sib);
				}
				if let Some(p) = (*self.edges[curr.index()].get()).parent {
					curr = p;
				} else {
					return None;
				}
			}
		}
	}

	/// Internal node allocator for mutations.
	fn alloc_node_internal(&self) -> NodeId {
		let id_val = self.next_id.fetch_add(1, Ordering::Relaxed);
		assert!(
			id_val <= self.capacity,
			"UastRegistry capacity exceeded during split"
		);
		NodeId(NonZeroU32::new(id_val).unwrap())
	}

	/// THE UPWARD BUBBLE: Propagates metric deltas to all ancestors in O(Depth).
	pub fn apply_edit(&self, target: NodeId, added_bytes: i32, added_newlines: i32) {
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
	pub fn insert_text(
		&self,
		target: NodeId,
		offset_in_node: u32,
		new_text: &[u8],
	) -> (NodeId, u32) {
		let added_bytes = new_text.len() as i32;
		let added_newlines = new_text.iter().filter(|&&b| b == b'\n').count() as i32;

		// 1. Propagate metrics upward before the split.
		self.apply_edit(target, added_bytes, added_newlines);

		// 2. Check for overflow rupture.
		let idx = target.index();
		let is_virtual = unsafe { (*self.spans[idx].get()).is_none() };

		if is_virtual {
			unsafe {
				if let Some(v_data) = &mut *self.virtual_data[idx].get() {
					v_data.splice(
						(offset_in_node as usize)..(offset_in_node as usize),
						new_text.iter().copied(),
					);
				}
			}
			(target, offset_in_node + new_text.len() as u32)
		} else {
			let v_id = self.split_node_pvp(target, offset_in_node, new_text);
			(v_id, new_text.len() as u32)
		}
	}

	/// RUPTURE: Splits a Physical node into [Physical, Virtual, Physical] siblings.
	/// Reuses `target` as the first Physical node (P1) to maintain topology efficiently.
	fn split_node_pvp(&self, target: NodeId, offset: u32, new_text: &[u8]) -> NodeId {
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
			s.as_mut().unwrap().byte_length = p1_len;

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
		let p2_len = old_svp.byte_length.saturating_sub(offset);
		let total_offset = old_svp.head_trim as u32 + offset;
		unsafe {
			*self.kinds[p2_idx].get() = SemanticKind::Token;
			*self.spans[p2_idx].get() = Some(SvpPointer {
				lba: old_svp.lba + (total_offset / 4096) as u64,
				byte_length: p2_len,
				device_id: old_svp.device_id,
				head_trim: (total_offset % 4096) as u16,
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

		v_id
	}

	pub fn get_first_child(&self, node: NodeId) -> Option<NodeId> {
		unsafe { (*self.edges[node.index()].get()).first_child }
	}

	pub fn get_prev_sibling(&self, node: NodeId) -> Option<NodeId> {
		let parent = unsafe { (*self.edges[node.index()].get()).parent }?;
		let first_child = unsafe { (*self.edges[parent.index()].get()).first_child }?;
		if first_child == node {
			return None;
		}
		let mut curr = first_child;
		loop {
			let next = unsafe { (*self.edges[curr.index()].get()).next_sibling };
			if next == Some(node) {
				return Some(curr);
			}
			curr = next?;
		}
	}

	pub fn delete_backwards(&self, target: NodeId, offset_in_node: u32) -> (NodeId, u32) {
		if offset_in_node == 0 {
			if let Some(prev) = self.get_prev_sibling(target) {
				let prev_idx = prev.index();
				let prev_len = unsafe { (*self.metrics[prev_idx].get()).byte_length };
				return self.delete_backwards(prev, prev_len);
			} else {
				return (target, 0);
			}
		}

		let idx = target.index();
		let is_virtual = unsafe { (*self.spans[idx].get()).is_none() };

		if is_virtual {
			let mut bytes_to_remove = 1;
			unsafe {
				if let Some(v_data) = &mut *self.virtual_data[idx].get() {
					let mut start = offset_in_node as usize - 1;
					while start > 0 && !v_data[start].is_ascii() && (v_data[start] & 0xC0) == 0x80 {
						start -= 1;
					}
					bytes_to_remove = offset_in_node as usize - start;
					v_data.drain(start..offset_in_node as usize);
				}
			}
			self.apply_edit(target, -(bytes_to_remove as i32), 0);
			(target, offset_in_node - bytes_to_remove as u32)
		} else {
			let bytes_to_remove = 1;
			let split_offset = offset_in_node.saturating_sub(bytes_to_remove);
			let v_id = self.split_node_pvp_delete(target, split_offset, bytes_to_remove);
			(v_id, 0)
		}
	}

	fn split_node_pvp_delete(&self, target: NodeId, offset: u32, delete_len: u32) -> NodeId {
		let target_idx = target.index();

		let (parent, old_next_sibling, old_svp) = unsafe {
			let e = &*self.edges[target_idx].get();
			let s = (*self.spans[target_idx].get()).expect("Split target must be Physical");
			(e.parent, e.next_sibling, s)
		};
		let parent = parent.expect("Cannot split a root node");

		let v_id = self.alloc_node_internal();
		let p2_id = self.alloc_node_internal();
		let v_idx = v_id.index();
		let p2_idx = p2_id.index();

		unsafe {
			let s = &mut *self.spans[target_idx].get();
			s.as_mut().unwrap().byte_length = offset;
			let m = &mut *self.metrics[target_idx].get();
			m.byte_length = offset;
			let e = &mut *self.edges[target_idx].get();
			e.next_sibling = Some(v_id);
		}

		unsafe {
			*self.kinds[v_idx].get() = SemanticKind::Token;
			*self.spans[v_idx].get() = None;
			*self.virtual_data[v_idx].get() = Some(Vec::new());
			let m = &mut *self.metrics[v_idx].get();
			m.byte_length = 0;
			m.newlines = 0;
			let e = &mut *self.edges[v_idx].get();
			e.parent = Some(parent);
			e.next_sibling = Some(p2_id);
		}

		let p2_len = old_svp.byte_length.saturating_sub(offset + delete_len);
		let total_offset = old_svp.head_trim as u32 + offset + delete_len;
		unsafe {
			*self.kinds[p2_idx].get() = SemanticKind::Token;
			*self.spans[p2_idx].get() = Some(SvpPointer {
				lba: old_svp.lba + (total_offset / 4096) as u64,
				byte_length: p2_len,
				device_id: old_svp.device_id,
				head_trim: (total_offset % 4096) as u16,
			});
			let m = &mut *self.metrics[p2_idx].get();
			m.byte_length = p2_len;
			let e = &mut *self.edges[p2_idx].get();
			e.parent = Some(parent);
			e.next_sibling = old_next_sibling;
		}

		let p_idx = parent.index();
		unsafe {
			let tail_ptr = &mut *self.child_tails[p_idx].get();
			if *tail_ptr == Some(target) {
				*tail_ptr = Some(p2_id);
			}
		}

		self.apply_edit(parent, -(delete_len as i32), 0);

		v_id
	}


	/// ATOMIC HOT-SWAP: Allows the I/O thread to populate virtual_data once DMA completes.
	pub fn hot_swap_virtual_data(&self, node: NodeId, data: Vec<u8>) {
		let idx = node.index();
		unsafe {
			*self.virtual_data[idx].get() = Some(data);
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
	pub fn spawn_node(
		&mut self,
		kind: SemanticKind,
		span: Option<SvpPointer>,
		metric: SpanMetrics,
	) -> NodeId {
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
		assert!(
			p_val >= self.start_id && p_val < self.start_id + self.len,
			"Parent out of chunk bounds"
		);
		assert!(
			c_val >= self.start_id && c_val < self.start_id + self.len,
			"Child out of chunk bounds"
		);

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
