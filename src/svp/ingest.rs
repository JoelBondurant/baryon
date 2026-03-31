use std::sync::Arc;
use crate::ecs::id::NodeId;
use crate::ecs::registry::UastRegistry;
use crate::uast::kind::SemanticKind;
use crate::uast::metrics::SpanMetrics;
use crate::svp::pointer::SvpPointer;
use crate::svp::resolver::{SvpResolver, SvpRequest, RequestPriority, DMA_CHUNK_SIZE};

pub fn ingest_svp_file(resolver: &SvpResolver, registry: &Arc<UastRegistry>, file_size: u64, device_id: u16, path: String) -> NodeId {
	let chunk_count = (file_size + DMA_CHUNK_SIZE as u64 - 1) / DMA_CHUNK_SIZE as u64;
	let mut chunk = registry.reserve_chunk(chunk_count as u32 + 1).expect("OOM");

	let root_id = chunk.spawn_node(
		SemanticKind::RelationalTable,
		None,
		SpanMetrics { byte_length: file_size as u32, newlines: 0 },
	);

	let mut first_leaf_id = None;
	let mut last_leaf_id = None;

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

		if first_leaf_id.is_none() { first_leaf_id = Some(leaf_id); }
		last_leaf_id = Some(leaf_id);

		chunk.append_local_child(root_id, leaf_id);
	}

	resolver.register_device(device_id, &path);

	// --- START SEQUENTIAL SCAN ---
	// Saturate the pipeline (16 in flight) to ensure high-speed background inflation
	if let (Some(first), Some(last)) = (first_leaf_id, last_leaf_id) {
		let max_initial = 16;
		for i in 0..max_initial {
			let idx = first.index() + i;
			if idx <= last.index() {
				if let Some(svp) = unsafe { *registry.spans[idx].get() } {
					let _ = resolver.request_queue.push(SvpRequest {
						node_id: NodeId::from_index(idx),
						pointer: svp,
						priority: RequestPriority::Low,
						last_node_id: Some(last),
					});
				}
			}
		}
	}

	root_id
}
