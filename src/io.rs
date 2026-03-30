use crate::ecs::{NodeId, SvpPointer, UastRegistry, SemanticKind, SpanMetrics};
use io_uring::{opcode, types, IoUring};
use crossbeam_queue::ArrayQueue;
use std::sync::Arc;
use std::thread;
use std::ptr;

/// 64KB aligned DMA buffer size
pub const DMA_CHUNK_SIZE: usize = 64 * 1024;

/// ==========================================
/// SPSC REQUEST QUEUE
/// ==========================================
pub struct SvpRequest {
	pub node_id: NodeId,
	pub pointer: SvpPointer,
}

#[allow(dead_code)]
pub struct SvpResolver {
	_registry: Arc<UastRegistry>,
	request_queue: Arc<ArrayQueue<SvpRequest>>,
}

impl SvpResolver {
	pub fn new(registry: Arc<UastRegistry>) -> Self {
		let request_queue = Arc::new(ArrayQueue::new(4096));
		
		let registry_clone = registry.clone();
		let queue_clone = request_queue.clone();
		
		thread::spawn(move || {
			Self::io_completion_loop(registry_clone, queue_clone);
		});

		Self {
			_registry: registry,
			request_queue,
		}
	}

	#[allow(dead_code)]
	pub fn _request_dma(&self, node_id: NodeId, pointer: SvpPointer) {
		let _ = self.request_queue.push(SvpRequest { node_id, pointer });
	}

	/// THE IO-URING BRIDGE (O_DIRECT + DMA)
	fn io_completion_loop(registry: Arc<UastRegistry>, queue: Arc<ArrayQueue<SvpRequest>>) {
		let mut ring = IoUring::new(256).expect("Failed to init io_uring");
		
		// PRE-ALLOCATED FRAME POOL (Aligned for O_DIRECT)
		let mut buffer_vec: Vec<u8>;
		unsafe {
			let mut ptr: *mut libc::c_void = ptr::null_mut();
			libc::posix_memalign(&mut ptr, 4096, DMA_CHUNK_SIZE);
			buffer_vec = Vec::from_raw_parts(ptr as *mut u8, 0, DMA_CHUNK_SIZE);
		}

		loop {
			// 1. Drain SPSC queue and submit to ring
			while let Some(req) = queue.pop() {
				// MOCK: In a real system, we'd open the device_id via libc::open with O_DIRECT
				// For this prototype, we simulate the LBA to buffer DMA.
				let read_op = opcode::Read::new(
					types::Fd(1), // Mock FD
					buffer_vec.as_mut_ptr(),
					req.pointer.byte_length,
				)
				.offset(req.pointer.lba * 512)
				.build()
				.user_data(req.node_id.index() as u64);

				unsafe {
					ring.submission()
						.push(&read_op)
						.expect("Submission queue full");
				}
			}

			ring.submit().expect("Ring submission failed");

			// 2. Reap completions and Hot-Swap virtual_data
			let mut cq = ring.completion();
			while let Some(cqe) = cq.next() {
				let node_idx = cqe.user_data() as usize;
				// In a real system, we'd copy from the DMA buffer to a permanent virtual_data Vec
				let data = buffer_vec[..].to_vec(); // Simulate DMA transfer
				
				// SAFETY: Atomically update the ECS SoA component
				registry.hot_swap_virtual_data(NodeId::from_index(node_idx), data);
			}
		}
	}
}

/// ==========================================
/// BULK INGESTION (O(N/64KB))
/// ==========================================
pub fn ingest_svp_file(registry: &UastRegistry, file_size: u64, device_id: u16) -> NodeId {
	let chunk_count = (file_size + DMA_CHUNK_SIZE as u64 - 1) / DMA_CHUNK_SIZE as u64;
	let mut chunk = registry.reserve_chunk(chunk_count as u32 + 1).expect("OOM");

	// 1. Create a File Root node to hold the chunks
	let root_id = chunk.spawn_node(
		SemanticKind::RelationalTable,
		None,
		SpanMetrics {
			byte_length: file_size as u32,
			newlines: 0,
		},
	);

	// 2. Spawn and link all chunks as children
	for i in 0..chunk_count {
		let lba = i * (DMA_CHUNK_SIZE as u64 / 512);
		let byte_length = if i == chunk_count - 1 {
			(file_size % DMA_CHUNK_SIZE as u64) as u32
		} else {
			DMA_CHUNK_SIZE as u32
		};

		let leaf_id = chunk.spawn_node(
			SemanticKind::Token,
			Some(SvpPointer {
				lba,
				byte_length,
				device_id,
				head_trim: 0,
			}),
			SpanMetrics {
				byte_length,
				newlines: 0,
			},
		);

		chunk.append_local_child(root_id, leaf_id);
	}

	root_id
}
