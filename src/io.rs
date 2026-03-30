use crate::ecs::{NodeId, SemanticKind, SpanMetrics, SvpPointer, UastRegistry};
use crossbeam_queue::ArrayQueue;
use io_uring::{opcode, types, IoUring};
use std::collections::HashMap;
use std::fs::File;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::ptr;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;

/// 64KB aligned DMA buffer size
pub const DMA_CHUNK_SIZE: usize = 64 * 1024;

pub struct SvpRequest {
	pub node_id: NodeId,
	pub pointer: SvpPointer,
}

pub struct SvpResolver {
	_registry: Arc<UastRegistry>,
	request_queue: Arc<ArrayQueue<SvpRequest>>,
	fds: Arc<Mutex<HashMap<u16, File>>>,
}

impl SvpResolver {
	pub fn new(registry: Arc<UastRegistry>, notifier: mpsc::Sender<()>) -> Self {
		let request_queue = Arc::new(ArrayQueue::new(4096));
		let fds = Arc::new(Mutex::new(HashMap::new()));

		let registry_clone = registry.clone();
		let queue_clone = request_queue.clone();
		let fds_clone = fds.clone();

		thread::spawn(move || {
			Self::io_completion_loop(registry_clone, queue_clone, fds_clone, notifier);
		});

		Self {
			_registry: registry,
			request_queue,
			fds,
		}
	}

	pub fn register_device(&self, device_id: u16, path: &str) {
		// Remove O_DIRECT for prototype compatibility with unaligned source files
		if let Ok(file) = File::open(path) {
			self.fds.lock().unwrap().insert(device_id, file);
		}
	}

	pub fn request_dma(&self, node_id: NodeId, pointer: SvpPointer) {
		let _ = self.request_queue.push(SvpRequest { node_id, pointer });
	}

	fn io_completion_loop(
		registry: Arc<UastRegistry>,
		queue: Arc<ArrayQueue<SvpRequest>>,
		fds: Arc<Mutex<HashMap<u16, File>>>,
		notifier: mpsc::Sender<()>,
	) {
		let mut ring = IoUring::new(256).expect("Failed to init io_uring");
		
		// Stable DMA buffer
		let mut buffer = vec![0u8; DMA_CHUNK_SIZE];

		loop {
			// 1. Process requests sequentially for buffer stability
			if let Some(req) = queue.pop() {
				let fd = {
					let guard = fds.lock().unwrap();
					guard.get(&req.pointer.device_id).map(|f| f.as_raw_fd())
				};

				if let Some(raw_fd) = fd {
					let read_op = opcode::Read::new(
						types::Fd(raw_fd),
						buffer.as_mut_ptr(),
						req.pointer.byte_length,
					)
					.offset(req.pointer.lba * 512 + req.pointer.head_trim as u64)
					.build()
					.user_data(req.node_id.0.get() as u64);

					unsafe {
						ring.submission()
							.push(&read_op)
							.expect("Ring submission failed");
					}
					
					// Submit and wait for this specific read to complete
					if let Ok(_) = ring.submit_and_wait(1) {
						let mut cq = ring.completion();
						if let Some(cqe) = cq.next() {
							let node_id_val = cqe.user_data() as u32;
							let node_id = NodeId(std::num::NonZeroU32::new(node_id_val).unwrap());

							if cqe.result() >= 0 {
								let byte_count = cqe.result() as usize;
								let data = buffer[..byte_count].to_vec();
								let newlines = data.iter().filter(|&&b| b == b'\n').count() as i32;

								// Hot-swap data and update metrics
								registry.hot_swap_virtual_data(node_id, data);
								registry.apply_edit(node_id, 0, newlines);
								
								// Signal completion
								registry.dma_in_flight[node_id.index()].store(false, Ordering::Relaxed);
								let _ = notifier.send(());
							} else {
								// Read error (e.g. EINVAL), clear flag to allow retry/UI update
								registry.dma_in_flight[node_id.index()].store(false, Ordering::Relaxed);
							}
						}
					}
				} else {
					// Missing FD, clear flag
					registry.dma_in_flight[req.node_id.index()].store(false, Ordering::Relaxed);
				}
			}

			thread::sleep(std::time::Duration::from_millis(1));
		}
	}
}

pub fn ingest_svp_file(registry: &UastRegistry, file_size: u64, device_id: u16) -> NodeId {
	let chunk_count = (file_size + DMA_CHUNK_SIZE as u64 - 1) / DMA_CHUNK_SIZE as u64;
	let mut chunk = registry.reserve_chunk(chunk_count as u32 + 1).expect("OOM");

	let root_id = chunk.spawn_node(
		SemanticKind::RelationalTable,
		None,
		SpanMetrics { byte_length: file_size as u32, newlines: 0 },
	);

	for i in 0..chunk_count {
		let byte_offset = i * DMA_CHUNK_SIZE as u64;
		let byte_length = if i == chunk_count - 1 {
			(file_size % DMA_CHUNK_SIZE as u64) as u32
		} else {
			DMA_CHUNK_SIZE as u32
		};

		let leaf_id = chunk.spawn_node(
			SemanticKind::Token,
			Some(SvpPointer {
				lba: byte_offset / 512,
				byte_length,
				device_id,
				head_trim: (byte_offset % 512) as u16,
			}),
			SpanMetrics { byte_length, newlines: 0 },
		);

		chunk.append_local_child(root_id, leaf_id);
	}

	root_id
}
