use crate::ecs::{NodeId, UastRegistry};
use crate::uast::kind::SemanticKind;

#[derive(Debug)]
pub struct RenderToken {
	pub node_id: NodeId,
	pub kind: SemanticKind,
	pub text: String,
	#[allow(dead_code)]
	pub absolute_start_line: u32,
	pub is_virtual: bool,
}

pub struct Viewport {
	pub tokens: Vec<RenderToken>,
	pub cursor_abs_pos: (u32, u32),
	pub total_lines: u32,
}

pub trait UastProjection {
	fn query_viewport(&self, root: NodeId, target_line: u32, line_count: u32) -> Vec<RenderToken>;
	fn get_next_node_in_walk(&self, node: NodeId) -> Option<NodeId>;
	fn find_node_at_line_col(&self, root: NodeId, target_line: u32, target_col: u32) -> (NodeId, u32);
}

impl UastProjection for UastRegistry {
	fn query_viewport(&self, root: NodeId, target_line: u32, line_count: u32) -> Vec<RenderToken> {
		let mut tokens = Vec::new();
		let mut curr = Some(root);
		let mut line_accumulator = 0;

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
			let text = self.resolve_physical_bytes(node);

			let is_virtual = unsafe { (*self.spans[idx].get()).is_none() };
			let kind = unsafe { *self.kinds[idx].get() };

			let mut display_text = text.clone();
			if line_accumulator < target_line {
				if line_accumulator + m.newlines < target_line {
					line_accumulator += m.newlines;
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
				}
				line_accumulator = target_line;
			}

			tokens.push(RenderToken {
				node_id: node,
				kind,
				text: display_text,
				absolute_start_line: line_accumulator,
				is_virtual,
			});

			if tokens.len() > 200 {
				break;
			}

			if text.is_empty() {
				line_accumulator += m.newlines;
				collected_lines += m.newlines;
			} else {
				let lines_shown = tokens.last().unwrap().text.chars().filter(|&c| c == '\n').count() as u32;
				line_accumulator += lines_shown;
				collected_lines += lines_shown;
			}

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

	fn find_node_at_line_col(&self, root: NodeId, target_line: u32, target_col: u32) -> (NodeId, u32) {
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
					let mut current_line_in_node = 0;

					for (i, b) in text.as_bytes().iter().enumerate() {
						if line_accumulator + current_line_in_node == target_line {
							let col_offset = target_col as usize;
							let line_end = text.as_bytes()[i..]
								.iter()
								.position(|&x| x == b'\n')
								.unwrap_or(text.len() - i);
							let actual_col = col_offset.min(line_end);
							return (node, (i + actual_col) as u32);
						}
						if *b == b'\n' {
							current_line_in_node += 1;
						}
					}
					return (node, text.len() as u32);
				}
			} else {
				line_accumulator += m.newlines;
				curr = unsafe { (*self.edges[idx].get()).next_sibling };
			}
		}

		(root, 0)
	}
}
