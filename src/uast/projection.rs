use crate::core::{CursorPosition, DocByte, DocLine, NodeByteOffset, TAB_SIZE, VisualCol};
use crate::ecs::{NodeId, UastRegistry};
use crate::svp::highlight::HighlightSpan;
use crate::uast::kind::SemanticKind;

#[derive(Debug)]
pub struct RenderToken {
	pub node_id: NodeId,
	pub kind: SemanticKind,
	pub text: String,
	#[allow(dead_code)]
	pub absolute_start_line: DocLine,
	pub absolute_start_byte: DocByte,
	pub is_virtual: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct NodeCursorTarget {
	pub node_id: NodeId,
	pub node_byte: NodeByteOffset,
	pub visual_col: VisualCol,
}

#[derive(Debug, Clone, Copy)]
pub struct NodeByteTarget {
	pub node_id: NodeId,
	pub node_start_byte: DocByte,
	pub node_end_byte: DocByte,
	pub node_byte: NodeByteOffset,
}

pub struct Viewport {
	pub tokens: Vec<RenderToken>,
	pub cursor_abs_pos: CursorPosition,
	pub total_lines: u32,
	pub status_message: Option<String>,
	pub should_quit: bool,
	pub file_name: Option<String>,
	pub file_size: u64,
	pub is_dirty: bool,
	pub search_pattern: Option<String>,
	pub search_case_insensitive: bool,
	pub search_match_info: Option<String>,
	pub confirm_prompt: Option<String>,
	pub mode_override: Option<crate::engine::EditorMode>,
	pub global_start_byte: DocByte,
	pub highlights: Vec<HighlightSpan>,
	pub yank_flash: Option<(DocByte, DocByte)>,
}

fn advance_visual_col(col: &mut VisualCol, byte: u8) {
	if byte == b'\t' {
		*col = col.saturating_add(TAB_SIZE - (col.get() % TAB_SIZE));
	} else {
		*col = col.saturating_add(1);
	}
}

fn line_start_offset(text: &[u8], target_line_in_node: DocLine) -> Option<usize> {
	if target_line_in_node == DocLine::ZERO {
		return Some(0);
	}

	let mut line = DocLine::ZERO;
	for (i, &b) in text.iter().enumerate() {
		if b == b'\n' {
			line = line.saturating_add(1);
			if line == target_line_in_node {
				return Some(i + 1);
			}
		}
	}

	None
}

fn offset_for_visual_col(line_bytes: &[u8], target_col: VisualCol) -> (NodeByteOffset, VisualCol) {
	let mut visual_col = VisualCol::ZERO;
	let mut byte_offset = 0usize;

	for (i, &b) in line_bytes.iter().enumerate() {
		if b == b'\n' || visual_col >= target_col {
			break;
		}

		byte_offset = i + 1;
		let mut next_col = visual_col;
		advance_visual_col(&mut next_col, b);

		if target_col <= next_col {
			return (NodeByteOffset::new(byte_offset as u32), next_col);
		}

		visual_col = next_col;
	}

	(NodeByteOffset::new(byte_offset as u32), visual_col)
}

pub trait UastProjection {
	fn query_viewport(
		&self,
		root: NodeId,
		target_line: DocLine,
		line_count: u32,
	) -> Vec<RenderToken>;
	fn get_next_node_in_walk(&self, node: NodeId) -> Option<NodeId>;
	fn find_node_at_line_col(
		&self,
		root: NodeId,
		target_line: DocLine,
		target_col: VisualCol,
	) -> NodeCursorTarget;
	fn find_node_at_doc_byte(&self, root: NodeId, target_byte: DocByte) -> NodeByteTarget;
	fn collect_document_bytes(&self, root: NodeId) -> Result<Vec<u8>, &'static str>;
}

impl UastProjection for UastRegistry {
	fn query_viewport(
		&self,
		root: NodeId,
		target_line: DocLine,
		line_count: u32,
	) -> Vec<RenderToken> {
		let mut tokens = Vec::new();
		let mut curr = Some(root);
		let mut line_accumulator = DocLine::ZERO;
		let mut byte_accumulator = DocByte::ZERO;

		// PHASE 1: DESCENT (O(log N))
		while let Some(node) = curr {
			let idx = node.index();
			let m = unsafe { &*self.metrics[idx].get() };

			if line_accumulator + m.newlines >= target_line {
				let first_child = unsafe { (*self.edges[idx].get()).first_child };
				if let Some(child) = first_child {
					curr = Some(child);
					continue;
				} else {
					break;
				}
			} else {
				line_accumulator = line_accumulator.saturating_add(m.newlines);
				byte_accumulator = byte_accumulator.saturating_add(m.byte_length as u64);
				let next_sibling = unsafe { (*self.edges[idx].get()).next_sibling };
				curr = next_sibling;
			}
		}

		// PHASE 2: ITERATION
		let mut collected_lines = 0;
		let mut visit = curr;

		while let Some(node) = visit {
			let idx = node.index();
			let m = unsafe { &*self.metrics[idx].get() };

			if m.byte_length == 0 && m.newlines == 0 {
				visit = self.get_next_node_in_walk(node);
				continue;
			}

			let text = self.resolve_physical_bytes(node);

			let is_virtual = unsafe { (*self.spans[idx].get()).is_none() };
			let kind = unsafe { *self.kinds[idx].get() };

			let mut display_text = text.clone();
			let mut token_start_byte = byte_accumulator;

			if line_accumulator < target_line {
				if line_accumulator + m.newlines < target_line {
					line_accumulator = line_accumulator.saturating_add(m.newlines);
					byte_accumulator = byte_accumulator.saturating_add(m.byte_length as u64);
					visit = self.get_next_node_in_walk(node);
					continue;
				}

				if !text.is_empty() {
					let to_skip = target_line.get().saturating_sub(line_accumulator.get());
					let mut skipped = 0;
					let mut byte_offset = 0;
					for (i, b) in text.as_bytes().iter().enumerate() {
						if skipped == to_skip {
							byte_offset = i;
							break;
						}
						if *b == b'\n' {
							skipped += 1;
						}
					}
					display_text = text[byte_offset..].to_string();
					token_start_byte = token_start_byte.saturating_add(byte_offset as u64);
				}
				line_accumulator = target_line;
			}

			tokens.push(RenderToken {
				node_id: node,
				kind,
				text: display_text,
				absolute_start_line: line_accumulator,
				absolute_start_byte: token_start_byte,
				is_virtual,
			});

			if tokens.len() > 200 {
				break;
			}

			if text.is_empty() {
				line_accumulator = line_accumulator.saturating_add(m.newlines);
				collected_lines += m.newlines;
			} else {
				let lines_shown = tokens
					.last()
					.unwrap()
					.text
					.chars()
					.filter(|&c| c == '\n')
					.count() as u32;
				line_accumulator = line_accumulator.saturating_add(lines_shown);
				collected_lines += lines_shown;
			}

			byte_accumulator = byte_accumulator.saturating_add(m.byte_length as u64);

			if collected_lines >= line_count {
				break;
			}

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

	fn find_node_at_line_col(
		&self,
		root: NodeId,
		target_line: DocLine,
		target_col: VisualCol,
	) -> NodeCursorTarget {
		let mut curr = Some(root);
		let mut line_accumulator = DocLine::ZERO;

		while let Some(node) = curr {
			let idx = node.index();
			let m = unsafe { &*self.metrics[idx].get() };

			if line_accumulator + m.newlines >= target_line {
				if let Some(child) = unsafe { (*self.edges[idx].get()).first_child } {
					curr = Some(child);
					continue;
				} else {
					let text = self.resolve_physical_bytes(node);

					let target_line_in_node = target_line.saturating_sub(line_accumulator.get());
					if let Some(line_start) =
						line_start_offset(text.as_bytes(), target_line_in_node)
					{
						let (line_offset, clamped_col) =
							offset_for_visual_col(&text.as_bytes()[line_start..], target_col);
						return NodeCursorTarget {
							node_id: node,
							node_byte: NodeByteOffset::new(line_start as u32 + line_offset.get()),
							visual_col: clamped_col,
						};
					}

					return NodeCursorTarget {
						node_id: node,
						node_byte: NodeByteOffset::new(text.len() as u32),
						visual_col: VisualCol::ZERO,
					};
				}
			} else {
				line_accumulator = line_accumulator.saturating_add(m.newlines);
				curr = unsafe { (*self.edges[idx].get()).next_sibling };
			}
		}

		NodeCursorTarget {
			node_id: root,
			node_byte: NodeByteOffset::ZERO,
			visual_col: VisualCol::ZERO,
		}
	}

	fn collect_document_bytes(&self, root: NodeId) -> Result<Vec<u8>, &'static str> {
		let mut result = Vec::new();
		let mut visit = self.get_first_child(root);

		while let Some(node) = visit {
			let idx = node.index();
			let has_children = unsafe { (*self.edges[idx].get()).first_child.is_some() };

			if has_children {
				visit = self.get_first_child(node);
				continue;
			}

			unsafe {
				if let Some(v_data) = &*self.virtual_data[idx].get() {
					result.extend_from_slice(v_data);
				} else if (*self.spans[idx].get()).is_some() {
					return Err("File still loading, cannot write yet");
				}
			}

			visit = self.get_next_node_in_walk(node);
		}

		Ok(result)
	}

	fn find_node_at_doc_byte(&self, root: NodeId, target_byte: DocByte) -> NodeByteTarget {
		let root_len = unsafe { (*self.metrics[root.index()].get()).byte_length as u64 };
		let clamped_target = target_byte.get().min(root_len);
		let mut curr = Some(root);
		let mut byte_accumulator = DocByte::ZERO;
		let mut last_leaf: Option<NodeByteTarget> = None;

		while let Some(node) = curr {
			let idx = node.index();
			let m = unsafe { &*self.metrics[idx].get() };
			let node_start = byte_accumulator;
			let node_end = node_start.saturating_add(m.byte_length as u64);

			if clamped_target < node_end.get()
				|| (clamped_target == root_len && node_end.get() == root_len)
			{
				if let Some(child) = unsafe { (*self.edges[idx].get()).first_child } {
					curr = Some(child);
					continue;
				}

				let local_offset = clamped_target.saturating_sub(node_start.get()) as u32;
				return NodeByteTarget {
					node_id: node,
					node_start_byte: node_start,
					node_end_byte: node_end,
					node_byte: NodeByteOffset::new(local_offset.min(m.byte_length)),
				};
			}

			if unsafe { (*self.edges[idx].get()).first_child.is_none() } {
				last_leaf = Some(NodeByteTarget {
					node_id: node,
					node_start_byte: node_start,
					node_end_byte: node_end,
					node_byte: NodeByteOffset::new(m.byte_length),
				});
			}

			byte_accumulator = node_end;
			curr = unsafe { (*self.edges[idx].get()).next_sibling };
		}

		last_leaf.unwrap_or(NodeByteTarget {
			node_id: root,
			node_start_byte: DocByte::ZERO,
			node_end_byte: DocByte::ZERO,
			node_byte: NodeByteOffset::ZERO,
		})
	}
}

#[cfg(test)]
mod tests {
	use super::UastProjection;
	use crate::core::{DocByte, DocLine, TAB_SIZE, VisualCol};
	use crate::ecs::UastRegistry;
	use crate::uast::kind::SemanticKind;
	use crate::uast::metrics::SpanMetrics;

	fn build_document(text: &str) -> (UastRegistry, crate::ecs::NodeId) {
		let registry = UastRegistry::new(8);
		let mut chunk = registry.reserve_chunk(2).expect("OOM");
		let newlines = text.bytes().filter(|&b| b == b'\n').count() as u32;
		let root = chunk.spawn_node(
			SemanticKind::RelationalTable,
			None,
			SpanMetrics {
				byte_length: text.len() as u32,
				newlines,
			},
		);
		let leaf = chunk.spawn_node(
			SemanticKind::Token,
			None,
			SpanMetrics {
				byte_length: text.len() as u32,
				newlines,
			},
		);
		chunk.append_local_child(root, leaf);
		unsafe {
			*registry.virtual_data[leaf.index()].get() = Some(text.as_bytes().to_vec());
		}
		(registry, root)
	}

	fn build_document_with_leaves(chunks: &[&str]) -> (UastRegistry, crate::ecs::NodeId) {
		let registry = UastRegistry::new((chunks.len() + 1) as u32 + 4);
		let mut chunk = registry
			.reserve_chunk((chunks.len() + 1) as u32)
			.expect("OOM");
		let full_text: String = chunks.concat();
		let newlines = full_text.bytes().filter(|&b| b == b'\n').count() as u32;
		let root = chunk.spawn_node(
			SemanticKind::RelationalTable,
			None,
			SpanMetrics {
				byte_length: full_text.len() as u32,
				newlines,
			},
		);
		for part in chunks {
			let bytes = part.as_bytes();
			let leaf = chunk.spawn_node(
				SemanticKind::Token,
				None,
				SpanMetrics {
					byte_length: bytes.len() as u32,
					newlines: bytes.iter().filter(|&&b| b == b'\n').count() as u32,
				},
			);
			chunk.append_local_child(root, leaf);
			unsafe {
				*registry.virtual_data[leaf.index()].get() = Some(bytes.to_vec());
			}
		}
		(registry, root)
	}

	fn visual_boundaries_for_line(text: &str) -> Vec<VisualCol> {
		let mut boundaries = vec![VisualCol::ZERO];
		let mut col = VisualCol::ZERO;

		for &b in text.as_bytes() {
			if b == b'\n' {
				break;
			}
			if b == b'\t' {
				col += TAB_SIZE - (col.get() % TAB_SIZE);
			} else {
				col += 1;
			}
			boundaries.push(col);
		}

		boundaries
	}

	#[test]
	fn find_node_at_line_col_clamps_to_visual_width_with_tabs() {
		let (registry, root) = build_document("\t\tlet x = 1;\n");
		let target = registry.find_node_at_line_col(root, DocLine::ZERO, VisualCol::new(30));

		assert_eq!(target.node_byte, crate::core::NodeByteOffset::new(12));
		assert_eq!(target.visual_col, VisualCol::new(18));
	}

	#[test]
	fn find_node_at_line_col_only_returns_character_boundaries_for_tabbed_lines() {
		let (registry, root) = build_document("\tfoo\n");
		let valid_boundaries = visual_boundaries_for_line("\tfoo\n");

		for clicked_col in 0..=7 {
			let target =
				registry.find_node_at_line_col(root, DocLine::ZERO, VisualCol::new(clicked_col));
			assert!(
				valid_boundaries.contains(&target.visual_col),
				"clicked visual col {} landed at invalid boundary {:?}",
				clicked_col,
				target.visual_col,
			);
		}
	}

	#[test]
	fn find_node_at_line_col_snaps_tab_clicks_to_tab_boundary() {
		let (registry, root) = build_document("\tfoo\n");

		for clicked_col in 1..=3 {
			let target =
				registry.find_node_at_line_col(root, DocLine::ZERO, VisualCol::new(clicked_col));
			assert_eq!(target.node_byte, crate::core::NodeByteOffset::new(1));
			assert_eq!(target.visual_col, VisualCol::new(4));
		}
	}

	#[test]
	fn find_node_at_line_col_only_returns_character_boundaries_for_mixed_indent() {
		let (registry, root) = build_document(" \tfoo\n");
		let valid_boundaries = visual_boundaries_for_line(" \tfoo\n");

		for clicked_col in 0..=7 {
			let target =
				registry.find_node_at_line_col(root, DocLine::ZERO, VisualCol::new(clicked_col));
			assert!(
				valid_boundaries.contains(&target.visual_col),
				"clicked visual col {} landed at invalid boundary {:?}",
				clicked_col,
				target.visual_col,
			);
		}
	}

	#[test]
	fn find_node_at_doc_byte_tracks_leaf_start_and_local_offset() {
		let (registry, root) = build_document_with_leaves(&["alpha", "beta", "gamma"]);

		let at_start_of_second = registry.find_node_at_doc_byte(root, DocByte::new(5));
		assert_eq!(at_start_of_second.node_start_byte, DocByte::new(5));
		assert_eq!(at_start_of_second.node_end_byte, DocByte::new(9));
		assert_eq!(
			at_start_of_second.node_byte,
			crate::core::NodeByteOffset::ZERO
		);

		let inside_third = registry.find_node_at_doc_byte(root, DocByte::new(11));
		assert_eq!(inside_third.node_start_byte, DocByte::new(9));
		assert_eq!(inside_third.node_end_byte, DocByte::new(14));
		assert_eq!(inside_third.node_byte, crate::core::NodeByteOffset::new(2),);

		let eof = registry.find_node_at_doc_byte(root, DocByte::new(14));
		assert_eq!(eof.node_start_byte, DocByte::new(9));
		assert_eq!(eof.node_end_byte, DocByte::new(14));
		assert_eq!(eof.node_byte, crate::core::NodeByteOffset::new(5));
	}
}
