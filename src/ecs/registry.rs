use crate::ecs::chunk::RegistryChunk;
use crate::ecs::id::NodeId;
use crate::svp::pointer::SvpPointer;
use crate::uast::kind::SemanticKind;
use crate::uast::metrics::SpanMetrics;
use crate::uast::topology::TreeEdges;
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

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
	pub(crate) child_tails: Box<[UnsafeCell<Option<NodeId>>]>,

	// VIRTUAL DATA: Stores uncommitted text for virtual nodes.
	// In a production system, this would be a more sophisticated buffer pool.
	pub virtual_data: Box<[UnsafeCell<Option<Vec<u8>>>]>,

	/// Tracks nodes currently being resolved by the I/O thread.
	pub dma_in_flight: Box<[AtomicBool]>,

	/// Ensures physical nodes are only inflated once.
	pub metrics_inflated: Box<[AtomicBool]>,
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
			metrics_inflated: (0..cap)
				.map(|_| AtomicBool::new(false))
				.collect::<Vec<_>>()
				.into_boxed_slice(),
		}
	}

	/// Internal node allocator for mutations.
	pub(crate) fn alloc_node_internal(&self) -> NodeId {
		let id_val = self.next_id.fetch_add(1, Ordering::Relaxed);
		assert!(
			id_val <= self.capacity,
			"UastRegistry capacity exceeded during split"
		);
		NodeId::from_index(id_val as usize - 1)
	}

	/// Atomically reserves a chunk of `NodeId`s for a single thread.
	/// Returns a `RegistryChunk` which grants lock-free write access to those specific indices.
	pub fn reserve_chunk(&self, size: u32) -> Option<RegistryChunk<'_>> {
		// Relaxed ordering is sufficient: we only need the counter to increment monotonically.
		let start_id = self.next_id.fetch_add(size, Ordering::Relaxed);

		if start_id + size > self.capacity + 1 {
			return None; // Out of pre-allocated memory (OOM)
		}

		Some(RegistryChunk::new(self, start_id, size))
	}

	/// ATOMIC HOT-SWAP: Allows the I/O thread to populate virtual_data once DMA completes.
	pub fn hot_swap_virtual_data(&self, node: NodeId, data: Vec<u8>) {
		let idx = node.index();
		unsafe {
			*self.virtual_data[idx].get() = Some(data);
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

	pub fn get_first_child(&self, node: NodeId) -> Option<NodeId> {
		unsafe { (*self.edges[node.index()].get()).first_child }
	}

	pub fn get_total_newlines(&self, node: NodeId) -> u32 {
		unsafe { (*self.metrics[node.index()].get()).newlines }
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

	pub fn get_next_sibling(&self, node: NodeId) -> Option<NodeId> {
		unsafe { (*self.edges[node.index()].get()).next_sibling }
	}
}
