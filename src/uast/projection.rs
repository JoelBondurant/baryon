use crate::ecs::{NodeId, UastRegistry};
use crate::uast::kind::SemanticKind;

#[derive(Debug)]
pub struct RenderToken {
	pub node_id: NodeId,
	pub kind: SemanticKind,
	pub text: String,
	#[allow(dead_code)]
	pub absolute_start_line: u32,
	pub absolute_start_byte: u64,
	pub is_virtual: bool,
}

pub struct Viewport {
	pub tokens: Vec<RenderToken>,
	pub cursor_abs_pos: (u32, u32),
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
	pub global_start_byte: u64,
	pub highlights: Vec<(u64, u64, crate::svp::highlight::TokenCategory)>,
	pub yank_flash: Option<(u64, u64)>,
}

fn advance_visual_col(col: &mut u32, byte: u8) {
	if byte == b'\t' {
		*col += 4 - (*col % 4);
	} else {
		*col += 1;
	}
}

fn line_start_offset(text: &[u8], target_line_in_node: u32) -> Option<usize> {
	if target_line_in_node == 0 {
		return Some(0);
	}

	let mut line = 0u32;
	for (i, &b) in text.iter().enumerate() {
		if b == b'\n' {
			line += 1;
			if line == target_line_in_node {
				return Some(i + 1);
			}
		}
	}

	None
}

fn offset_for_visual_col(line_bytes: &[u8], target_col: u32) -> (u32, u32) {
	let mut visual_col = 0u32;
	let mut byte_offset = 0usize;

	for (i, &b) in line_bytes.iter().enumerate() {
		if b == b'\n' || visual_col >= target_col {
			break;
		}

		byte_offset = i + 1;
		let mut next_col = visual_col;
		advance_visual_col(&mut next_col, b);

		if target_col <= next_col {
			return (byte_offset as u32, target_col);
		}

		visual_col = next_col;
	}

	(byte_offset as u32, visual_col)
}

pub trait UastProjection {
	fn query_viewport(&self, root: NodeId, target_line: u32, line_count: u32) -> Vec<RenderToken>;
	fn get_next_node_in_walk(&self, node: NodeId) -> Option<NodeId>;
	fn find_node_at_line_col(
		&self,
		root: NodeId,
		target_line: u32,
		target_col: u32,
	) -> (NodeId, u32, u32);
	fn collect_document_bytes(&self, root: NodeId) -> Result<Vec<u8>, &'static str>;
}

impl UastProjection for UastRegistry {
	fn query_viewport(&self, root: NodeId, target_line: u32, line_count: u32) -> Vec<RenderToken> {
		let mut tokens = Vec::new();
		let mut curr = Some(root);
		let mut line_accumulator = 0;
		let mut byte_accumulator: u64 = 0;

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
				line_accumulator += m.newlines;
				byte_accumulator += m.byte_length as u64;
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
					line_accumulator += m.newlines;
					byte_accumulator += m.byte_length as u64;
					visit = self.get_next_node_in_walk(node);
					continue;
				}

				if !text.is_empty() {
					let to_skip = target_line - line_accumulator;
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
					token_start_byte += byte_offset as u64;
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
				line_accumulator += m.newlines;
				collected_lines += m.newlines;
			} else {
				let lines_shown = tokens
					.last()
					.unwrap()
					.text
					.chars()
					.filter(|&c| c == '\n')
					.count() as u32;
				line_accumulator += lines_shown;
				collected_lines += lines_shown;
			}

			byte_accumulator += m.byte_length as u64;

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
		target_line: u32,
		target_col: u32,
	) -> (NodeId, u32, u32) {
		let mut curr = Some(root);
		let mut line_accumulator = 0;

			while let Some(node) = curr {
				let idx = node.index();
				let m = unsafe { &*self.metrics[idx].get() };

				if line_accumulator + m.newlines >= target_line {
					if let Some(child) = unsafe { (*self.edges[idx].get()).first_child } {
						curr = Some(child);
						continue;
					} else {
						let text = self.resolve_physical_bytes(node);

						let target_line_in_node = target_line.saturating_sub(line_accumulator);
						if let Some(line_start) =
							line_start_offset(text.as_bytes(), target_line_in_node)
						{
							let (line_offset, clamped_col) =
								offset_for_visual_col(&text.as_bytes()[line_start..], target_col);
							return (node, line_start as u32 + line_offset, clamped_col);
						}

						return (node, text.len() as u32, 0);
					}
				} else {
					line_accumulator += m.newlines;
					curr = unsafe { (*self.edges[idx].get()).next_sibling };
				}
			}

		(root, 0, 0)
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
}

#[cfg(test)]
mod tests {
	use super::UastProjection;
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

	#[test]
	fn find_node_at_line_col_clamps_to_visual_width_with_tabs() {
		let (registry, root) = build_document("\t\tlet x = 1;\n");
		let (_node, offset, col) = registry.find_node_at_line_col(root, 0, 30);

		assert_eq!(offset, 12);
		assert_eq!(col, 18);
	}

	#[test]
	fn find_node_at_line_col_preserves_visual_positions_inside_tabs() {
		let (registry, root) = build_document("\tfoo\n");
		let (_node, offset, col) = registry.find_node_at_line_col(root, 0, 3);

		assert_eq!(offset, 1);
		assert_eq!(col, 3);
	}
}
