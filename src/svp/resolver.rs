use crate::ecs::id::NodeId;
use crate::ecs::registry::UastRegistry;
use crate::svp::pointer::SvpPointer;
use crate::uast::mutation::UastMutation;
use crossbeam_queue::ArrayQueue;
use io_uring::{IoUring, opcode, types};
use memchr::memchr_iter;
use std::collections::HashMap;
use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::thread;

/// 64KB aligned DMA buffer size
pub const DMA_CHUNK_SIZE: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestPriority {
	High, // Viewport (Immediate hot-swap)
	Low,  // Metric Scan (Inflate SpanMetrics)
}

pub struct SvpRequest {
	pub node_id: NodeId,
	pub pointer: SvpPointer,
	pub priority: RequestPriority,
	pub last_node_id: Option<NodeId>, // For sequential scanning
}

#[allow(dead_code)]
pub struct SvpResolver {
	_registry: Arc<UastRegistry>,
	pub(crate) request_queue: Arc<ArrayQueue<SvpRequest>>,
	fds: Arc<Mutex<HashMap<u16, File>>>,
}

impl SvpResolver {
	pub fn new(registry: Arc<UastRegistry>, notifier: mpsc::Sender<()>) -> Self {
		let request_queue = Arc::new(ArrayQueue::new(8192));
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
		if let Ok(file) = File::open(path) {
			self.fds.lock().unwrap().insert(device_id, file);
		}
	}

	pub fn request_dma(&self, node_id: NodeId, pointer: SvpPointer, priority: RequestPriority) {
		let _ = self.request_queue.push(SvpRequest {
			node_id,
			pointer,
			priority,
			last_node_id: None,
		});
	}

	/// Internal helper to continue sequential scanning
	fn request_scan_internal(
		queue: &ArrayQueue<SvpRequest>,
		node_id: NodeId,
		pointer: SvpPointer,
		last_node_id: NodeId,
	) {
		let _ = queue.push(SvpRequest {
			node_id,
			pointer,
			priority: RequestPriority::Low,
			last_node_id: Some(last_node_id),
		});
	}

	fn io_completion_loop(
		registry: Arc<UastRegistry>,
		queue: Arc<ArrayQueue<SvpRequest>>,
		fds: Arc<Mutex<HashMap<u16, File>>>,
		notifier: mpsc::Sender<()>,
	) {
		let mut ring = IoUring::new(256).expect("Failed to init io_uring");

		// Use a pool of buffers for concurrent requests
		const BATCH_SIZE: usize = 16;
		let mut buffers = vec![vec![0u8; DMA_CHUNK_SIZE]; BATCH_SIZE];
		let mut in_flight = 0;

		loop {
			// 1. Submit as many requests as possible
			while in_flight < BATCH_SIZE {
				if let Some(req) = queue.pop() {
					let fd = {
						let guard = fds.lock().unwrap();
						guard.get(&req.pointer.device_id).map(|f| f.as_raw_fd())
					};

					if let Some(raw_fd) = fd {
						let is_viewport = req.priority == RequestPriority::High;
						let buf_idx = in_flight;
						let last_idx_plus_1 =
							req.last_node_id.map(|id| id.index() + 1).unwrap_or(0);

						let user_data = (req.node_id.index() as u64)
							| ((is_viewport as u64) << 32)
							| ((buf_idx as u64) << 33)
							| ((last_idx_plus_1 as u64) << 40);

						let read_op = opcode::Read::new(
							types::Fd(raw_fd),
							buffers[buf_idx].as_mut_ptr(),
							req.pointer.byte_length,
						)
						.offset(req.pointer.lba * 512 + req.pointer.head_trim as u64)
						.build()
						.user_data(user_data);

						unsafe {
							ring.submission()
								.push(&read_op)
								.expect("Submission queue full");
						}
						in_flight += 1;
					} else {
						registry.dma_in_flight[req.node_id.index()].store(false, Ordering::Relaxed);
					}
				} else {
					break;
				}
			}

			if in_flight > 0 {
				ring.submit_and_wait(1).expect("Ring wait failed");

				let mut cq = ring.completion();
				let mut got_any = false;
				while let Some(cqe) = cq.next() {
					let res = cqe.result();
					let ud = cqe.user_data();
					let node_idx = (ud & 0xFFFFFFFF) as usize;
					let was_viewport = (ud >> 32) & 1 == 1;
					let buf_idx = ((ud >> 33) & 0x7F) as usize;
					let last_idx_plus_1 = (ud >> 40) as usize;

					let node_id = NodeId::from_index(node_idx);

					if res >= 0 {
						let byte_count = res as usize;
						let newlines =
							memchr_iter(b'\n', &buffers[buf_idx][..byte_count]).count() as i32;

						// 1. ATOMIC INFLATION: Ensure every physical node is counted exactly once
						if !registry.metrics_inflated[node_idx].swap(true, Ordering::Relaxed) {
							registry.apply_edit(node_id, 0, newlines);
						}

						if was_viewport {
							let data = buffers[buf_idx][..byte_count].to_vec();
							registry.hot_swap_virtual_data(node_id, data);
						}

						registry.dma_in_flight[node_idx].store(false, Ordering::Relaxed);

						if last_idx_plus_1 > 0 && node_idx + 1 < last_idx_plus_1 {
							let next_idx = node_idx + 1;
							if let Some(next_svp) = unsafe { *registry.spans[next_idx].get() } {
								Self::request_scan_internal(
									&queue,
									NodeId::from_index(next_idx),
									next_svp,
									NodeId::from_index(last_idx_plus_1 - 1),
								);
							}
						}
						got_any = true;
					} else {
						registry.dma_in_flight[node_idx].store(false, Ordering::Relaxed);
					}
					in_flight -= 1;
				}
				if got_any {
					let _ = notifier.send(());
				}
			} else {
				thread::sleep(std::time::Duration::from_millis(1));
			}
		}
	}
}
