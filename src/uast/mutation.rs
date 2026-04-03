use crate::ecs::{NodeId, UastRegistry};
use crate::svp::pointer::SvpPointer;
use crate::svp::resolver::DMA_CHUNK_SIZE;
use crate::uast::kind::SemanticKind;
use std::sync::atomic::Ordering;

pub trait UastMutation {
	fn apply_edit(&self, target: NodeId, added_bytes: i32, added_newlines: i32);
	fn insert_text(&self, target: NodeId, offset_in_node: u32, new_text: &[u8]) -> (NodeId, u32);
	fn delete_backwards(&self, target: NodeId, offset_in_node: u32) -> (NodeId, u32);
	fn split_node_pvp(&self, target: NodeId, offset: u32, new_text: &[u8]) -> NodeId;
	fn split_node_pvp_delete(&self, target: NodeId, offset: u32, delete_len: u32) -> NodeId;
}

fn spawn_physical_tail_chain(
	registry: &UastRegistry,
	parent: NodeId,
	start_offset: u64,
	total_len: u32,
	device_id: u16,
	resolved_tail: Option<&[u8]>,
	next_sibling: Option<NodeId>,
) -> Option<(NodeId, NodeId)> {
	if total_len == 0 {
		return None;
	}

	let mut first: Option<NodeId> = None;
	let mut prev: Option<NodeId> = None;
	let mut offset_in_tail = 0u32;

	while offset_in_tail < total_len {
		let chunk_len = (total_len - offset_in_tail).min(DMA_CHUNK_SIZE as u32);
		let chunk_start = start_offset + u64::from(offset_in_tail);
		let node = registry.alloc_node_internal();
		let idx = node.index();

		unsafe {
			*registry.kinds[idx].get() = SemanticKind::Token;
			*registry.spans[idx].get() = Some(SvpPointer {
				lba: chunk_start / 512,
				byte_length: chunk_len,
				device_id,
				head_trim: (chunk_start % 512) as u16,
			});

			let metrics = &mut *registry.metrics[idx].get();
			metrics.byte_length = chunk_len;
			metrics.newlines = 0;

			if let Some(data) = resolved_tail {
				let start = offset_in_tail as usize;
				let end = start + chunk_len as usize;
				let chunk_data = &data[start..end];
				metrics.newlines = chunk_data.iter().filter(|&&b| b == b'\n').count() as u32;
				*registry.virtual_data[idx].get() = Some(chunk_data.to_vec());
				registry.metrics_inflated[idx].store(true, Ordering::Relaxed);
			}

			let edges = &mut *registry.edges[idx].get();
			edges.parent = Some(parent);
			edges.next_sibling = None;
		}

		if let Some(prev_node) = prev {
			unsafe {
				(*registry.edges[prev_node.index()].get()).next_sibling = Some(node);
			}
		} else {
			first = Some(node);
		}
		prev = Some(node);
		offset_in_tail += chunk_len;
	}

	let first = first.expect("non-empty tail should create a first node");
	let last = prev.expect("non-empty tail should create a last node");
	unsafe {
		(*registry.edges[last.index()].get()).next_sibling = next_sibling;
	}
	Some((first, last))
}

impl UastMutation for UastRegistry {
	fn apply_edit(&self, target: NodeId, added_bytes: i32, added_newlines: i32) {
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

	fn insert_text(&self, target: NodeId, offset_in_node: u32, new_text: &[u8]) -> (NodeId, u32) {
		let added_bytes = new_text.len() as i32;
		let added_newlines = new_text.iter().filter(|&&b| b == b'\n').count() as i32;

		self.apply_edit(target, added_bytes, added_newlines);

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
			unsafe {
				if let Some(v_data) = &mut *self.virtual_data[idx].get() {
					*self.spans[idx].get() = None;
					v_data.splice(
						(offset_in_node as usize)..(offset_in_node as usize),
						new_text.iter().copied(),
					);
					self.metrics_inflated[idx].store(true, std::sync::atomic::Ordering::Relaxed);
					return (target, offset_in_node + new_text.len() as u32);
				}
			}

			let v_id = self.split_node_pvp(target, offset_in_node, new_text);
			(v_id, new_text.len() as u32)
		}
	}

	fn split_node_pvp(&self, target: NodeId, offset: u32, new_text: &[u8]) -> NodeId {
		let target_idx = target.index();

		let (parent, old_next_sibling, old_svp) = unsafe {
			let e = &*self.edges[target_idx].get();
			let s = (*self.spans[target_idx].get()).expect("Split target must be Physical");
			(e.parent, e.next_sibling, s)
		};
		let parent = parent.expect("Cannot split a root node");

		let v_id = self.alloc_node_internal();
		let v_idx = v_id.index();

		let p1_len = offset;
		// Split virtual_data if the node was already DMA-resolved
		let resolved_data = unsafe { (*self.virtual_data[target_idx].get()).take() };

		unsafe {
			let s = &mut *self.spans[target_idx].get();
			s.as_mut().unwrap().byte_length = p1_len;

			let m = &mut *self.metrics[target_idx].get();
			m.byte_length = p1_len;
			if let Some(ref data) = resolved_data {
				let p1_data = &data[..offset as usize];
				m.newlines = p1_data.iter().filter(|&&b| b == b'\n').count() as u32;
				*self.virtual_data[target_idx].get() = Some(p1_data.to_vec());
			}

			let e = &mut *self.edges[target_idx].get();
			e.next_sibling = Some(v_id);
		}

		let v_len = new_text.len() as u32;
		let v_newlines = new_text.iter().filter(|&&b| b == b'\n').count() as u32;
		unsafe {
			*self.kinds[v_idx].get() = SemanticKind::Token;
			*self.spans[v_idx].get() = None;
			*self.virtual_data[v_idx].get() = Some(new_text.to_vec());

			let m = &mut *self.metrics[v_idx].get();
			m.byte_length = v_len;
			m.newlines = v_newlines;

			let e = &mut *self.edges[v_idx].get();
			e.parent = Some(parent);
			e.next_sibling = old_next_sibling;
		}

		let p2_len = old_svp.byte_length.saturating_sub(offset);
		let base_offset = old_svp.lba * 512 + u64::from(old_svp.head_trim);
		let split_offset = base_offset + u64::from(offset);
		let tail_chain = spawn_physical_tail_chain(
			self,
			parent,
			split_offset,
			p2_len,
			old_svp.device_id,
			resolved_data.as_ref().map(|data| &data[offset as usize..]),
			old_next_sibling,
		);
		unsafe {
			(*self.edges[v_idx].get()).next_sibling =
				tail_chain.map(|(first, _)| first).or(old_next_sibling);
		}

		let p_idx = parent.index();
		unsafe {
			let tail_ptr = &mut *self.child_tails[p_idx].get();
			if *tail_ptr == Some(target) {
				*tail_ptr = Some(tail_chain.map(|(_, last)| last).unwrap_or(v_id));
			}
		}

		v_id
	}

	fn delete_backwards(&self, target: NodeId, offset_in_node: u32) -> (NodeId, u32) {
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
			let mut removed_newlines = 0i32;
			unsafe {
				if let Some(v_data) = &mut *self.virtual_data[idx].get() {
					let mut start = offset_in_node as usize - 1;
					while start > 0 && !v_data[start].is_ascii() && (v_data[start] & 0xC0) == 0x80 {
						start -= 1;
					}
					bytes_to_remove = offset_in_node as usize - start;
					removed_newlines = v_data[start..offset_in_node as usize]
						.iter()
						.filter(|&&b| b == b'\n')
						.count() as i32;
					v_data.drain(start..offset_in_node as usize);
				}
			}
			self.apply_edit(target, -(bytes_to_remove as i32), -removed_newlines);
			(target, offset_in_node - bytes_to_remove as u32)
		} else {
			unsafe {
				if let Some(v_data) = &mut *self.virtual_data[idx].get() {
					let mut start = offset_in_node as usize - 1;
					while start > 0 && (v_data[start] & 0xC0) == 0x80 {
						start -= 1;
					}
					let bytes_to_remove = offset_in_node as usize - start;
					let removed_newlines = v_data[start..offset_in_node as usize]
						.iter()
						.filter(|&&b| b == b'\n')
						.count() as i32;
					v_data.drain(start..offset_in_node as usize);
					*self.spans[idx].get() = None;
					self.metrics_inflated[idx].store(true, std::sync::atomic::Ordering::Relaxed);
					self.apply_edit(target, -(bytes_to_remove as i32), -removed_newlines);
					return (target, start as u32);
				}
			}

			let bytes_to_remove = 1u32;
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
		let v_idx = v_id.index();

		let resolved_data = unsafe { (*self.virtual_data[target_idx].get()).take() };
		let deleted_newlines = resolved_data
			.as_ref()
			.map(|data| {
				data[offset as usize..(offset + delete_len) as usize]
					.iter()
					.filter(|&&b| b == b'\n')
					.count() as i32
			})
			.unwrap_or(0);

		unsafe {
			let s = &mut *self.spans[target_idx].get();
			s.as_mut().unwrap().byte_length = offset;
			let m = &mut *self.metrics[target_idx].get();
			m.byte_length = offset;
			if let Some(ref data) = resolved_data {
				let p1_data = &data[..offset as usize];
				m.newlines = p1_data.iter().filter(|&&b| b == b'\n').count() as u32;
				*self.virtual_data[target_idx].get() = Some(p1_data.to_vec());
			}
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
			e.next_sibling = old_next_sibling;
		}

		let p2_len = old_svp.byte_length.saturating_sub(offset + delete_len);
		let base_offset = old_svp.lba * 512 + u64::from(old_svp.head_trim);
		let split_offset = base_offset + u64::from(offset + delete_len);
		let tail_chain = spawn_physical_tail_chain(
			self,
			parent,
			split_offset,
			p2_len,
			old_svp.device_id,
			resolved_data
				.as_ref()
				.map(|data| &data[(offset + delete_len) as usize..]),
			old_next_sibling,
		);
		unsafe {
			(*self.edges[v_idx].get()).next_sibling =
				tail_chain.map(|(first, _)| first).or(old_next_sibling);
		}

		let p_idx = parent.index();
		unsafe {
			let tail_ptr = &mut *self.child_tails[p_idx].get();
			if *tail_ptr == Some(target) {
				*tail_ptr = Some(tail_chain.map(|(_, last)| last).unwrap_or(v_id));
			}
		}

		self.apply_edit(parent, -(delete_len as i32), -deleted_newlines);

		v_id
	}
}

#[cfg(test)]
mod tests {
	use super::UastMutation;
	use crate::ecs::UastRegistry;
	use crate::svp::pointer::SvpPointer;
	use crate::svp::resolver::DMA_CHUNK_SIZE;
	use crate::uast::kind::SemanticKind;
	use crate::uast::metrics::SpanMetrics;

	fn build_physical_leaf(registry: &UastRegistry, span: SvpPointer) -> crate::ecs::NodeId {
		let mut chunk = registry.reserve_chunk(2).expect("OOM");
		let root = chunk.spawn_node(
			SemanticKind::RelationalTable,
			None,
			SpanMetrics {
				byte_length: span.byte_length,
				newlines: 0,
			},
		);
		let leaf = chunk.spawn_node(
			SemanticKind::Token,
			Some(span),
			SpanMetrics {
				byte_length: span.byte_length,
				newlines: 0,
			},
		);
		chunk.append_local_child(root, leaf);
		leaf
	}

	fn parent_of(registry: &UastRegistry, node: crate::ecs::NodeId) -> crate::ecs::NodeId {
		unsafe {
			(*registry.edges[node.index()].get())
				.parent
				.expect("node should have parent")
		}
	}

	fn collect_physical_spans(
		registry: &UastRegistry,
		start: Option<crate::ecs::NodeId>,
	) -> Vec<SvpPointer> {
		let mut spans = Vec::new();
		let mut current = start;
		while let Some(node) = current {
			let span = unsafe { *registry.spans[node.index()].get() };
			let Some(span) = span else {
				break;
			};
			spans.push(span);
			current = registry.get_next_sibling(node);
		}
		spans
	}

	#[test]
	fn split_node_pvp_preserves_512_byte_span_addressing() {
		let registry = UastRegistry::new(8);
		let span = SvpPointer {
			lba: 1,
			byte_length: 10_000,
			device_id: 7,
			head_trim: 88,
		};
		let leaf = build_physical_leaf(&registry, span);

		let inserted = registry.split_node_pvp(leaf, 9_000, b"hi");
		let tail = registry
			.get_next_sibling(inserted)
			.expect("tail span should exist");
		let tail_span = unsafe { (*registry.spans[tail.index()].get()).expect("tail span") };

		let expected_offset = span.lba * 512 + u64::from(span.head_trim) + 9_000;
		assert_eq!(tail_span.lba, expected_offset / 512);
		assert_eq!(u64::from(tail_span.head_trim), expected_offset % 512);
	}

	#[test]
	fn split_node_pvp_delete_preserves_512_byte_span_addressing() {
		let registry = UastRegistry::new(8);
		let span = SvpPointer {
			lba: 3,
			byte_length: 12_000,
			device_id: 9,
			head_trim: 144,
		};
		let leaf = build_physical_leaf(&registry, span);

		let tombstone = registry.split_node_pvp_delete(leaf, 8_000, 3);
		let tail = registry
			.get_next_sibling(tombstone)
			.expect("tail span should exist");
		let tail_span = unsafe { (*registry.spans[tail.index()].get()).expect("tail span") };

		let expected_offset = span.lba * 512 + u64::from(span.head_trim) + 8_003;
		assert_eq!(tail_span.lba, expected_offset / 512);
		assert_eq!(u64::from(tail_span.head_trim), expected_offset % 512);
	}

	#[test]
	fn resolved_physical_leaf_edits_promote_the_whole_chunk_to_virtual() {
		let registry = UastRegistry::new(8);
		let span = SvpPointer {
			lba: 0,
			byte_length: 6,
			device_id: 1,
			head_trim: 0,
		};
		let leaf = build_physical_leaf(&registry, span);
		unsafe {
			*registry.virtual_data[leaf.index()].get() = Some(b"abcdef".to_vec());
		}

		let (inserted, insert_offset) = registry.insert_text(leaf, 3, b"ZZ");
		assert_eq!(inserted, leaf);
		assert_eq!(insert_offset, 5);
		assert!(registry.get_next_sibling(leaf).is_none());
		assert!(unsafe { (*registry.spans[leaf.index()].get()).is_none() });
		assert_eq!(
			unsafe { (*registry.virtual_data[leaf.index()].get()).as_deref() },
			Some("abcZZdef".as_bytes()),
		);
		assert_eq!(
			unsafe { (*registry.metrics[leaf.index()].get()).byte_length },
			8
		);

		let (deleted, delete_offset) = registry.delete_backwards(leaf, 5);
		assert_eq!(deleted, leaf);
		assert_eq!(delete_offset, 4);
		assert!(registry.get_next_sibling(leaf).is_none());
		assert!(unsafe { (*registry.spans[leaf.index()].get()).is_none() });
		assert_eq!(
			unsafe { (*registry.virtual_data[leaf.index()].get()).as_deref() },
			Some("abcZdef".as_bytes()),
		);
		assert_eq!(
			unsafe { (*registry.metrics[leaf.index()].get()).byte_length },
			7
		);
	}

	#[test]
	fn delete_backwards_uses_utf8_width_for_promoted_physical_leaves() {
		let registry = UastRegistry::new(8);
		let span = SvpPointer {
			lba: 0,
			byte_length: 4,
			device_id: 1,
			head_trim: 0,
		};
		let leaf = build_physical_leaf(&registry, span);
		unsafe {
			*registry.virtual_data[leaf.index()].get() = Some("aéb".as_bytes().to_vec());
		}

		let (deleted, delete_offset) = registry.delete_backwards(leaf, 3);
		assert_eq!(deleted, leaf);
		assert_eq!(delete_offset, 1);
		assert!(registry.get_next_sibling(leaf).is_none());
		assert!(unsafe { (*registry.spans[leaf.index()].get()).is_none() });
		assert_eq!(
			unsafe { (*registry.virtual_data[leaf.index()].get()).as_deref() },
			Some("ab".as_bytes()),
		);
	}

	#[test]
	fn insert_text_rechunks_large_physical_tails_to_dma_sized_spans() {
		let registry = UastRegistry::new(16);
		let span = SvpPointer {
			lba: 2,
			byte_length: (DMA_CHUNK_SIZE as u32) * 2 + 17,
			device_id: 11,
			head_trim: 96,
		};
		let leaf = build_physical_leaf(&registry, span);
		let root = parent_of(&registry, leaf);

		let (inserted, insert_offset) = registry.insert_text(leaf, 1, b"XY");
		assert_eq!(insert_offset, 2);

		let tail_spans = collect_physical_spans(&registry, registry.get_next_sibling(inserted));
		assert_eq!(tail_spans.len(), 3);
		assert!(
			tail_spans
				.iter()
				.all(|span| span.byte_length as usize <= DMA_CHUNK_SIZE)
		);
		assert_eq!(
			tail_spans.iter().map(|span| span.byte_length).sum::<u32>(),
			span.byte_length - 1,
		);

		let expected_offset = span.lba * 512 + u64::from(span.head_trim) + 1;
		assert_eq!(tail_spans[0].lba, expected_offset / 512);
		assert_eq!(u64::from(tail_spans[0].head_trim), expected_offset % 512);
		assert_eq!(
			unsafe { (*registry.metrics[root.index()].get()).byte_length },
			span.byte_length + 2,
		);
	}

	#[test]
	fn delete_backwards_rechunks_large_physical_tails_to_dma_sized_spans() {
		let registry = UastRegistry::new(16);
		let span = SvpPointer {
			lba: 4,
			byte_length: (DMA_CHUNK_SIZE as u32) * 2 + 33,
			device_id: 12,
			head_trim: 144,
		};
		let leaf = build_physical_leaf(&registry, span);
		let root = parent_of(&registry, leaf);

		let (tombstone, delete_offset) = registry.delete_backwards(leaf, 1);
		assert_eq!(delete_offset, 0);

		let tail_spans = collect_physical_spans(&registry, registry.get_next_sibling(tombstone));
		assert_eq!(tail_spans.len(), 3);
		assert!(
			tail_spans
				.iter()
				.all(|span| span.byte_length as usize <= DMA_CHUNK_SIZE)
		);
		assert_eq!(
			tail_spans.iter().map(|span| span.byte_length).sum::<u32>(),
			span.byte_length - 1,
		);

		let expected_offset = span.lba * 512 + u64::from(span.head_trim) + 1;
		assert_eq!(tail_spans[0].lba, expected_offset / 512);
		assert_eq!(u64::from(tail_spans[0].head_trim), expected_offset % 512);
		assert_eq!(
			unsafe { (*registry.metrics[root.index()].get()).byte_length },
			span.byte_length - 1,
		);
	}
}
