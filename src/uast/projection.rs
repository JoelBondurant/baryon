use crate::core::{CursorPosition, DocByte, DocLine, NodeByteOffset, TAB_SIZE, VisualCol};
use crate::ecs::{NodeId, UastRegistry};
use crate::engine::VisibleRow;
use crate::svp::diagnostic::DiagnosticSpan;
use crate::svp::highlight::{CATEGORY_COUNT, HighlightSpan};
use crate::uast::kind::SemanticKind;
use ratatui::style::Color;
use std::sync::atomic::Ordering;

#[derive(Debug)]
pub struct RenderToken {
	pub node_id: NodeId,
	pub kind: SemanticKind,
	pub text: Vec<u8>,
	pub physical_byte_len: u32,
	#[allow(dead_code)]
	pub absolute_start_line: DocLine,
	pub absolute_start_byte: DocByte,
	pub is_virtual: bool,
	pub is_folded: bool,
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

#[derive(Debug, Clone)]
pub struct RootChildLineIndex {
	entries: Vec<RootChildLineIndexEntry>,
}

#[derive(Debug, Clone, Copy)]
struct RootChildLineIndexEntry {
	node_id: NodeId,
	start_line: DocLine,
	end_line: DocLine,
	start_byte: DocByte,
}

pub struct Viewport {
	pub tokens: Vec<RenderToken>,
	pub scroll_y: u32,
	pub viewport_start_line: DocLine,
	pub viewport_row_offset: u32,
	pub viewport_line_count: u32,
	pub wrap_enabled: bool,
	pub cursor_lines: Vec<DocLine>,
	pub cursor_visual_row: u32,
	pub cursor_abs_pos: CursorPosition,
	pub cursor_screen_col: VisualCol,
	pub cursor_abs_byte: DocByte,
	pub cursor_line_start_byte: DocByte,
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
	pub diagnostics: Vec<DiagnosticSpan>,
	pub selection_ranges: Vec<(DocByte, DocByte)>,
	pub yank_flash: Option<(DocByte, DocByte)>,
	pub minimap: Option<MinimapSnapshot>,
	pub theme_colors: [Option<Color>; CATEGORY_COUNT],
	pub visible_rows: Vec<VisibleRow>,
	pub visible_line_starts: Vec<DocLine>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MinimapMode {
	TextDensity,
	ByteFallback,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinimapSnapshot {
	pub mode: MinimapMode,
	pub bands: Vec<u8>,
	pub search_bands: Vec<u8>,
	pub active_search_band: Option<usize>,
	pub total_lines: u32,
	pub viewport_start_line: DocLine,
	pub viewport_end_line: DocLine,
	pub viewport_line_count: u32,
	pub cursor_line: DocLine,
}

const FOLDED_TOKEN_TEXT: &str = "❯❯❯";

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

fn line_starts_at_node_end(text: &[u8], target_line_in_node: DocLine) -> bool {
	matches!(
		line_start_offset(text, target_line_in_node),
		Some(offset) if offset == text.len()
	)
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
	fn doc_byte_for_node_offset(
		&self,
		root: NodeId,
		node: NodeId,
		node_offset: NodeByteOffset,
	) -> DocByte;
	fn read_loaded_slice(
		&self,
		root: NodeId,
		start: DocByte,
		end: DocByte,
	) -> Result<Vec<u8>, &'static str>;
}

impl UastRegistry {
	pub(crate) fn build_root_child_line_index(&self, root: NodeId) -> RootChildLineIndex {
		let mut entries = Vec::new();
		let mut line_accumulator = DocLine::ZERO;
		let mut byte_accumulator = DocByte::ZERO;
		let mut child = self.get_first_child(root);

		while let Some(node) = child {
			let metrics = unsafe { &*self.metrics[node.index()].get() };
			let end_line = line_accumulator.saturating_add(metrics.newlines);
			entries.push(RootChildLineIndexEntry {
				node_id: node,
				start_line: line_accumulator,
				end_line,
				start_byte: byte_accumulator,
			});
			line_accumulator = end_line;
			byte_accumulator = byte_accumulator.saturating_add(metrics.byte_length as u64);
			child = self.get_next_sibling(node);
		}

		RootChildLineIndex { entries }
	}

	pub(crate) fn next_node_skipping_children(&self, node: NodeId) -> Option<NodeId> {
		unsafe {
			let mut curr = node;
			loop {
				if let Some(sib) = (*self.edges[curr.index()].get()).next_sibling {
					return Some(sib);
				}
				if let Some(parent) = (*self.edges[curr.index()].get()).parent {
					curr = parent;
				} else {
					return None;
				}
			}
		}
	}

	fn find_node_at_line_col_internal(
		&self,
		root: NodeId,
		target_line: DocLine,
		target_col: VisualCol,
		respect_folds: bool,
		line_index: Option<&RootChildLineIndex>,
	) -> NodeCursorTarget {
		let (mut curr, mut line_accumulator, _) =
			seek_root_child_for_line(self, root, target_line, line_index);
		let mut last_effective_leaf: Option<NodeCursorTarget> = None;

		while let Some(node) = curr {
			let idx = node.index();
			let has_children = unsafe { (*self.edges[idx].get()).first_child.is_some() };
			let is_folded =
				respect_folds && has_children && self.is_folded[idx].load(Ordering::Acquire);

			if has_children && !is_folded {
				curr = self.get_first_child(node);
				continue;
			}

			if !has_children && !self.metrics_inflated[idx].load(Ordering::Acquire) {
				self.ensure_metrics_inflated(node);
			}

			let metrics = unsafe { &*self.metrics[idx].get() };
			let node_end_line = line_accumulator.saturating_add(metrics.newlines);
			last_effective_leaf = Some(if is_folded {
				NodeCursorTarget {
					node_id: node,
					node_byte: NodeByteOffset::ZERO,
					visual_col: VisualCol::ZERO,
				}
			} else {
				let bytes = self.read_node_bytes_sync(node).unwrap_or_default();
				NodeCursorTarget {
					node_id: node,
					node_byte: NodeByteOffset::new(bytes.len() as u32),
					visual_col: VisualCol::ZERO,
				}
			});

			let contains_target_line = target_line < node_end_line
				|| (target_line == node_end_line
					&& (!is_folded || self.next_node_skipping_children(node).is_none()));
			if contains_target_line {
				if is_folded {
					return NodeCursorTarget {
						node_id: node,
						node_byte: NodeByteOffset::ZERO,
						visual_col: VisualCol::ZERO,
					};
				}

				let bytes = self.read_node_bytes_sync(node).unwrap_or_default();
				let target_line_in_node = target_line.saturating_sub(line_accumulator.get());
				if line_starts_at_node_end(&bytes, target_line_in_node)
					&& self.next_node_skipping_children(node).is_some()
				{
					line_accumulator = node_end_line;
					curr = self.next_node_skipping_children(node);
					continue;
				}
				if let Some(line_start) = line_start_offset(&bytes, target_line_in_node) {
					let (line_offset, clamped_col) =
						offset_for_visual_col(&bytes[line_start..], target_col);
					return NodeCursorTarget {
						node_id: node,
						node_byte: NodeByteOffset::new(line_start as u32 + line_offset.get()),
						visual_col: clamped_col,
					};
				}

				return NodeCursorTarget {
					node_id: node,
					node_byte: NodeByteOffset::new(bytes.len() as u32),
					visual_col: VisualCol::ZERO,
				};
			}

			line_accumulator = node_end_line;
			curr = self.next_node_skipping_children(node);
		}

		last_effective_leaf.unwrap_or(NodeCursorTarget {
			node_id: root,
			node_byte: NodeByteOffset::ZERO,
			visual_col: VisualCol::ZERO,
		})
	}

	fn find_node_at_doc_byte_internal(
		&self,
		root: NodeId,
		target_byte: DocByte,
		respect_folds: bool,
	) -> NodeByteTarget {
		let root_len = unsafe { (*self.metrics[root.index()].get()).byte_length as u64 };
		let clamped_target = target_byte.get().min(root_len);
		let mut curr = Some(root);
		let mut byte_accumulator = DocByte::ZERO;
		let mut last_effective_leaf: Option<NodeByteTarget> = None;

		while let Some(node) = curr {
			let idx = node.index();
			let has_children = unsafe { (*self.edges[idx].get()).first_child.is_some() };
			let is_folded =
				respect_folds && has_children && self.is_folded[idx].load(Ordering::Acquire);
			let metrics = unsafe { &*self.metrics[idx].get() };
			let node_start = byte_accumulator;
			let node_end = node_start.saturating_add(metrics.byte_length as u64);

			if clamped_target < node_end.get()
				|| (clamped_target == root_len && node_end.get() == root_len)
			{
				if has_children && !is_folded {
					curr = self.get_first_child(node);
					continue;
				}

				let local_offset = if is_folded {
					0
				} else {
					clamped_target.saturating_sub(node_start.get()) as u32
				};
				return NodeByteTarget {
					node_id: node,
					node_start_byte: node_start,
					node_end_byte: node_end,
					node_byte: NodeByteOffset::new(local_offset.min(metrics.byte_length)),
				};
			}

			last_effective_leaf = Some(NodeByteTarget {
				node_id: node,
				node_start_byte: node_start,
				node_end_byte: node_end,
				node_byte: NodeByteOffset::new(if is_folded { 0 } else { metrics.byte_length }),
			});

			byte_accumulator = node_end;
			curr = self.next_node_skipping_children(node);
		}

		last_effective_leaf.unwrap_or(NodeByteTarget {
			node_id: root,
			node_start_byte: DocByte::ZERO,
			node_end_byte: DocByte::ZERO,
			node_byte: NodeByteOffset::ZERO,
		})
	}

	pub(crate) fn find_node_at_line_col_raw(
		&self,
		root: NodeId,
		target_line: DocLine,
		target_col: VisualCol,
	) -> NodeCursorTarget {
		self.find_node_at_line_col_internal(root, target_line, target_col, false, None)
	}

	pub(crate) fn find_node_at_line_col_with_root_line_index(
		&self,
		root: NodeId,
		target_line: DocLine,
		target_col: VisualCol,
		line_index: &RootChildLineIndex,
	) -> NodeCursorTarget {
		self.find_node_at_line_col_internal(root, target_line, target_col, true, Some(line_index))
	}

	pub(crate) fn find_node_at_doc_byte_raw(
		&self,
		root: NodeId,
		target_byte: DocByte,
	) -> NodeByteTarget {
		self.find_node_at_doc_byte_internal(root, target_byte, false)
	}

	pub fn for_each_loaded_slice_fragment<F>(
		&self,
		root: NodeId,
		start: DocByte,
		end: DocByte,
		mut callback: F,
	) -> Result<(), &'static str>
	where
		F: FnMut(&[u8]),
	{
		let root_len = self.get_total_bytes(root);
		let start = start.get().min(root_len);
		let end = end.get().min(root_len);
		if start >= end {
			return Ok(());
		}

		let start_target = self.find_node_at_doc_byte_raw(root, DocByte::new(start));
		let mut visit = Some(start_target.node_id);
		let mut local_start = start_target.node_byte.get() as usize;
		let mut absolute_start = start_target.node_start_byte;
		let mut remaining = (end - start) as usize;

		while let Some(node) = visit {
			let idx = node.index();
			let has_children = unsafe { (*self.edges[idx].get()).first_child.is_some() };
			if has_children {
				visit = self.get_first_child(node);
				continue;
			}

			let node_len = unsafe { (*self.metrics[idx].get()).byte_length as usize };
			if node_len > 0 && absolute_start.get() < end {
				let bytes = unsafe {
					if let Some(v_data) = &*self.virtual_data[idx].get() {
						v_data.as_slice()
					} else if (*self.spans[idx].get()).is_some() {
						return Err("File still loading, cannot read slice yet");
					} else {
						return Err("Leaf text unavailable");
					}
				};

				let take_len = (node_len.saturating_sub(local_start)).min(remaining);
				if take_len > 0 {
					callback(&bytes[local_start..local_start + take_len]);
					remaining -= take_len;
					if remaining == 0 {
						return Ok(());
					}
				}
			}

			absolute_start = absolute_start.saturating_add(node_len as u64);
			local_start = 0;
			visit = self.get_next_node_in_walk(node);
		}

		if remaining == 0 {
			Ok(())
		} else {
			Err("Slice exceeded loaded document bounds")
		}
	}
}

fn seek_root_child_for_line(
	_registry: &UastRegistry,
	root: NodeId,
	target_line: DocLine,
	line_index: Option<&RootChildLineIndex>,
) -> (Option<NodeId>, DocLine, DocByte) {
	let Some(line_index) = line_index else {
		return (Some(root), DocLine::ZERO, DocByte::ZERO);
	};
	if line_index.entries.is_empty() {
		return (Some(root), DocLine::ZERO, DocByte::ZERO);
	}

	let target = target_line.get();
	let index = line_index
		.entries
		.partition_point(|entry| entry.end_line.get() < target)
		.min(line_index.entries.len().saturating_sub(1));
	let entry = line_index.entries[index];
	(Some(entry.node_id), entry.start_line, entry.start_byte)
}

impl UastProjection for UastRegistry {
	fn query_viewport(
		&self,
		root: NodeId,
		target_line: DocLine,
		line_count: u32,
	) -> Vec<RenderToken> {
		self.query_viewport_with_root_line_index(root, target_line, line_count, None)
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
		self.find_node_at_line_col_internal(root, target_line, target_col, true, None)
	}

	fn doc_byte_for_node_offset(
		&self,
		root: NodeId,
		node: NodeId,
		node_offset: NodeByteOffset,
	) -> DocByte {
		let mut absolute = DocByte::new(node_offset.get() as u64);
		let mut curr = node;

		while curr != root {
			let Some(parent) = (unsafe { (*self.edges[curr.index()].get()).parent }) else {
				break;
			};

			let mut sibling = unsafe { (*self.edges[parent.index()].get()).first_child };
			while let Some(candidate) = sibling {
				if candidate == curr {
					break;
				}

				let sibling_len = unsafe { (*self.metrics[candidate.index()].get()).byte_length };
				absolute = absolute.saturating_add(sibling_len as u64);
				sibling = unsafe { (*self.edges[candidate.index()].get()).next_sibling };
			}

			curr = parent;
		}

		absolute
	}

	fn read_loaded_slice(
		&self,
		root: NodeId,
		start: DocByte,
		end: DocByte,
	) -> Result<Vec<u8>, &'static str> {
		let root_len = self.get_total_bytes(root);
		let start = start.get().min(root_len);
		let end = end.get().min(root_len);
		let mut result = Vec::with_capacity((end.saturating_sub(start)) as usize);
		self.for_each_loaded_slice_fragment(
			root,
			DocByte::new(start),
			DocByte::new(end),
			|fragment| {
				result.extend_from_slice(fragment);
			},
		)?;
		Ok(result)
	}

	fn find_node_at_doc_byte(&self, root: NodeId, target_byte: DocByte) -> NodeByteTarget {
		self.find_node_at_doc_byte_internal(root, target_byte, true)
	}
}

impl UastRegistry {
	pub(crate) fn query_viewport_with_root_line_index(
		&self,
		root: NodeId,
		target_line: DocLine,
		line_count: u32,
		line_index: Option<&RootChildLineIndex>,
	) -> Vec<RenderToken> {
		let mut tokens = Vec::new();
		let (mut curr, mut line_accumulator, mut byte_accumulator) =
			seek_root_child_for_line(self, root, target_line, line_index);
		let line_count = line_count.max(1);

		// PHASE 1: DESCENT (O(log N))
		while let Some(node) = curr {
			let idx = node.index();
			let m = unsafe { &*self.metrics[idx].get() };
			let has_children = unsafe { (*self.edges[idx].get()).first_child.is_some() };
			let is_folded = has_children && self.is_folded[idx].load(Ordering::Acquire);
			let node_end_line = line_accumulator.saturating_add(m.newlines);
			let contains_target_line = target_line < node_end_line
				|| (target_line == node_end_line
					&& (!is_folded || self.next_node_skipping_children(node).is_none()));

			if contains_target_line {
				if !has_children && !is_folded && target_line == node_end_line {
					let text = self.resolve_physical_bytes(node);
					let target_line_in_node = target_line.saturating_sub(line_accumulator.get());
					if line_starts_at_node_end(&text, target_line_in_node)
						&& self.next_node_skipping_children(node).is_some()
					{
						line_accumulator = node_end_line;
						byte_accumulator = byte_accumulator.saturating_add(m.byte_length as u64);
						curr = self.next_node_skipping_children(node);
						continue;
					}
				}
				if has_children && !is_folded {
					let first_child = unsafe { (*self.edges[idx].get()).first_child };
					if let Some(child) = first_child {
						curr = Some(child);
						continue;
					}
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
			let has_children = unsafe { (*self.edges[idx].get()).first_child.is_some() };
			let is_folded = has_children && self.is_folded[idx].load(Ordering::Acquire);

			if has_children && !is_folded {
				visit = self.get_first_child(node);
				continue;
			}

			let m = unsafe { &*self.metrics[idx].get() };
			let node_end_line = line_accumulator.saturating_add(m.newlines);

			if m.byte_length == 0 && m.newlines == 0 {
				visit = self.next_node_skipping_children(node);
				continue;
			}

			let text = if is_folded {
				FOLDED_TOKEN_TEXT.as_bytes().to_vec()
			} else {
				self.resolve_physical_bytes(node)
			};

			let is_virtual = is_folded || unsafe { (*self.spans[idx].get()).is_none() };
			let kind = unsafe { *self.kinds[idx].get() };

			let mut display_text = text.clone();
			let mut token_start_line = line_accumulator;
			let mut token_start_byte = byte_accumulator;

			if is_folded && line_accumulator < target_line {
				let skip_fold = node_end_line < target_line
					|| (node_end_line == target_line
						&& self.next_node_skipping_children(node).is_some());
				if skip_fold {
					line_accumulator = node_end_line;
					byte_accumulator = byte_accumulator.saturating_add(m.byte_length as u64);
					visit = self.next_node_skipping_children(node);
					continue;
				}
			}

			if !is_folded && line_accumulator < target_line {
				if line_accumulator + m.newlines < target_line {
					line_accumulator = node_end_line;
					byte_accumulator = byte_accumulator.saturating_add(m.byte_length as u64);
					visit = self.next_node_skipping_children(node);
					continue;
				}

				if !text.is_empty() {
					let to_skip = target_line.saturating_sub(line_accumulator.get());
					let byte_offset = line_start_offset(&text, to_skip).unwrap_or(text.len());
					if byte_offset == text.len() && self.next_node_skipping_children(node).is_some()
					{
						line_accumulator = node_end_line;
						byte_accumulator = byte_accumulator.saturating_add(m.byte_length as u64);
						visit = self.next_node_skipping_children(node);
						continue;
					}
					display_text = text.get(byte_offset..).unwrap_or(&[]).to_vec();
					token_start_byte = token_start_byte.saturating_add(byte_offset as u64);
				}
				line_accumulator = target_line;
				token_start_line = target_line;
			}

			tokens.push(RenderToken {
				node_id: node,
				kind,
				text: display_text,
				physical_byte_len: m.byte_length,
				absolute_start_line: token_start_line,
				absolute_start_byte: token_start_byte,
				is_virtual,
				is_folded,
			});

			if tokens.len() > 200 {
				break;
			}

			if is_folded {
				line_accumulator = line_accumulator.saturating_add(m.newlines);
				collected_lines += 1;
			} else if text.is_empty() {
				line_accumulator = line_accumulator.saturating_add(m.newlines);
				collected_lines += m.newlines;
			} else {
				let lines_shown = tokens
					.last()
					.unwrap()
					.text
					.iter()
					.filter(|&&b| b == b'\n')
					.count() as u32;
				line_accumulator = line_accumulator.saturating_add(lines_shown);
				collected_lines += lines_shown;
			}

			byte_accumulator = byte_accumulator.saturating_add(m.byte_length as u64);

			if collected_lines >= line_count {
				break;
			}

			visit = self.next_node_skipping_children(node);
		}

		tokens
	}
}

#[cfg(test)]
mod tests {
	use super::UastProjection;
	use crate::core::{DocByte, DocLine, TAB_SIZE, VisualCol};
	use crate::ecs::UastRegistry;
	use crate::svp::pointer::SvpPointer;
	use crate::uast::kind::SemanticKind;
	use crate::uast::metrics::SpanMetrics;
	use std::sync::atomic::Ordering;
	use std::time::{SystemTime, UNIX_EPOCH};

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

	fn build_document_bytes(bytes: &[u8]) -> (UastRegistry, crate::ecs::NodeId) {
		let registry = UastRegistry::new(8);
		let mut chunk = registry.reserve_chunk(2).expect("OOM");
		let newlines = bytes.iter().filter(|&&b| b == b'\n').count() as u32;
		let root = chunk.spawn_node(
			SemanticKind::RelationalTable,
			None,
			SpanMetrics {
				byte_length: bytes.len() as u32,
				newlines,
			},
		);
		let leaf = chunk.spawn_node(
			SemanticKind::Token,
			None,
			SpanMetrics {
				byte_length: bytes.len() as u32,
				newlines,
			},
		);
		chunk.append_local_child(root, leaf);
		unsafe {
			*registry.virtual_data[leaf.index()].get() = Some(bytes.to_vec());
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

	fn build_foldable_document() -> (UastRegistry, crate::ecs::NodeId, crate::ecs::NodeId) {
		let registry = UastRegistry::new(8);
		let mut chunk = registry.reserve_chunk(6).expect("OOM");
		let root = chunk.spawn_node(
			SemanticKind::RelationalTable,
			None,
			SpanMetrics {
				byte_length: 17,
				newlines: 3,
			},
		);
		let head = chunk.spawn_node(
			SemanticKind::Token,
			None,
			SpanMetrics {
				byte_length: 5,
				newlines: 1,
			},
		);
		let folded = chunk.spawn_node(
			SemanticKind::RelationalTable,
			None,
			SpanMetrics {
				byte_length: 8,
				newlines: 2,
			},
		);
		let folded_a = chunk.spawn_node(
			SemanticKind::Token,
			None,
			SpanMetrics {
				byte_length: 4,
				newlines: 1,
			},
		);
		let folded_b = chunk.spawn_node(
			SemanticKind::Token,
			None,
			SpanMetrics {
				byte_length: 4,
				newlines: 1,
			},
		);
		let tail = chunk.spawn_node(
			SemanticKind::Token,
			None,
			SpanMetrics {
				byte_length: 4,
				newlines: 0,
			},
		);
		chunk.append_local_child(root, head);
		chunk.append_local_child(root, folded);
		chunk.append_local_child(root, tail);
		chunk.append_local_child(folded, folded_a);
		chunk.append_local_child(folded, folded_b);
		unsafe {
			*registry.virtual_data[head.index()].get() = Some(b"head\n".to_vec());
			*registry.virtual_data[folded_a.index()].get() = Some(b"one\n".to_vec());
			*registry.virtual_data[folded_b.index()].get() = Some(b"two\n".to_vec());
			*registry.virtual_data[tail.index()].get() = Some(b"tail".to_vec());
		}
		(registry, root, folded)
	}

	fn temp_test_path(name: &str) -> std::path::PathBuf {
		let nanos = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.expect("time should move forward")
			.as_nanos();
		std::env::temp_dir().join(format!("baryon-{}-{}-{}", name, std::process::id(), nanos))
	}

	fn build_uninflated_physical_document(chunks: &[&str]) -> (UastRegistry, crate::ecs::NodeId) {
		let registry = UastRegistry::new((chunks.len() + 1) as u32 + 4);
		let mut chunk = registry
			.reserve_chunk((chunks.len() + 1) as u32)
			.expect("OOM");
		let total_len = chunks.iter().map(|part| part.len() as u32).sum();
		let root = chunk.spawn_node(
			SemanticKind::RelationalTable,
			None,
			SpanMetrics {
				byte_length: total_len,
				newlines: 0,
			},
		);

		let mut byte_offset = 0u64;
		for part in chunks {
			let leaf = chunk.spawn_node(
				SemanticKind::Token,
				Some(SvpPointer {
					lba: byte_offset / 512,
					byte_length: part.len() as u32,
					device_id: 77,
					head_trim: (byte_offset % 512) as u16,
				}),
				SpanMetrics {
					byte_length: part.len() as u32,
					newlines: 0,
				},
			);
			chunk.append_local_child(root, leaf);
			byte_offset += part.len() as u64;
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

	#[test]
	fn doc_byte_for_node_offset_reconstructs_absolute_positions() {
		let (registry, root) = build_document_with_leaves(&["alpha", "beta", "gamma"]);

		let second = registry.find_node_at_doc_byte(root, DocByte::new(6));
		assert_eq!(
			registry.doc_byte_for_node_offset(root, second.node_id, second.node_byte),
			DocByte::new(6)
		);

		let third = registry.find_node_at_doc_byte(root, DocByte::new(11));
		assert_eq!(
			registry.doc_byte_for_node_offset(root, third.node_id, third.node_byte),
			DocByte::new(11)
		);
	}

	#[test]
	fn find_node_at_line_col_prefers_next_leaf_at_trailing_newline_boundary() {
		let (registry, root) = build_document_with_leaves(&["head\n", "tail"]);
		let second = registry
			.get_next_sibling(registry.get_first_child(root).unwrap())
			.unwrap();

		let target = registry.find_node_at_line_col(root, DocLine::new(1), VisualCol::ZERO);

		assert_eq!(target.node_id, second);
		assert_eq!(target.node_byte, crate::core::NodeByteOffset::ZERO);
	}

	#[test]
	fn query_viewport_starts_with_next_leaf_after_trailing_newline_boundary() {
		let (registry, root) = build_document_with_leaves(&["head\n", "tail"]);
		let second = registry
			.get_next_sibling(registry.get_first_child(root).unwrap())
			.unwrap();

		let tokens = registry.query_viewport(root, DocLine::new(1), 1);

		assert_eq!(tokens.len(), 1);
		assert_eq!(tokens[0].node_id, second);
		assert_eq!(tokens[0].text, b"tail".to_vec());
	}

	#[test]
	fn for_each_loaded_slice_fragment_yields_disjoint_leaf_slices_in_order() {
		let (registry, root) = build_document_with_leaves(&["alpha", "beta", "gamma"]);
		let mut fragments = Vec::new();

		registry
			.for_each_loaded_slice_fragment(root, DocByte::new(3), DocByte::new(11), |fragment| {
				fragments.push(String::from_utf8_lossy(fragment).into_owned());
			})
			.expect("slice fragments");

		assert_eq!(
			fragments,
			vec!["ha".to_string(), "beta".to_string(), "ga".to_string()]
		);
	}

	#[test]
	fn read_loaded_slice_collects_fragments_from_the_fragment_visitor() {
		let (registry, root) = build_document_with_leaves(&["alpha", "beta", "gamma"]);
		let slice = registry
			.read_loaded_slice(root, DocByte::new(3), DocByte::new(11))
			.expect("loaded slice");

		assert_eq!(String::from_utf8(slice).expect("utf8"), "habetaga");
	}

	#[test]
	fn query_viewport_preserves_invalid_utf8_bytes_when_skipping_lines() {
		let (registry, root) = build_document_bytes(b"aa\n\xffbb\ncc");
		let tokens = registry.query_viewport(root, DocLine::new(1), 2);

		assert_eq!(tokens.len(), 1);
		assert_eq!(tokens[0].absolute_start_byte, DocByte::new(3));
		assert_eq!(tokens[0].text, b"\xffbb\ncc".to_vec());
	}

	#[test]
	fn root_line_indexed_query_viewport_matches_plain_query_near_eof() {
		let parts = ["head\n", "alpha\nbeta\n", "gamma\ndelta\n", "tail"];
		let (registry, root) = build_document_with_leaves(&parts);
		let line_index = registry.build_root_child_line_index(root);

		let plain = registry.query_viewport(root, DocLine::new(4), 2);
		let indexed = registry.query_viewport_with_root_line_index(
			root,
			DocLine::new(4),
			2,
			Some(&line_index),
		);

		assert_eq!(indexed.len(), plain.len());
		for (left, right) in indexed.iter().zip(plain.iter()) {
			assert_eq!(left.node_id, right.node_id);
			assert_eq!(left.text, right.text);
			assert_eq!(left.absolute_start_line, right.absolute_start_line);
			assert_eq!(left.absolute_start_byte, right.absolute_start_byte);
		}
	}

	#[test]
	fn root_line_indexed_cursor_lookup_matches_plain_lookup_near_eof() {
		let parts = ["head\n", "alpha\nbeta\n", "gamma\ndelta\n", "tail"];
		let (registry, root) = build_document_with_leaves(&parts);
		let line_index = registry.build_root_child_line_index(root);

		let plain = registry.find_node_at_line_col(root, DocLine::new(4), VisualCol::new(2));
		let indexed = registry.find_node_at_line_col_with_root_line_index(
			root,
			DocLine::new(4),
			VisualCol::new(2),
			&line_index,
		);

		assert_eq!(indexed.node_id, plain.node_id);
		assert_eq!(indexed.node_byte, plain.node_byte);
		assert_eq!(indexed.visual_col, plain.visual_col);
	}

	#[test]
	fn query_viewport_emits_fold_placeholder_and_skips_descendants() {
		let (registry, root, folded) = build_foldable_document();
		registry.is_folded[folded.index()].store(true, Ordering::Release);

		let tokens = registry.query_viewport(root, DocLine::ZERO, 4);

		assert_eq!(tokens.len(), 3);
		assert_eq!(tokens[0].text, b"head\n".to_vec());
		assert!(tokens[1].is_folded);
		assert_eq!(tokens[1].node_id, folded);
		assert_eq!(tokens[1].text, "❯❯❯".as_bytes().to_vec());
		assert_eq!(tokens[1].absolute_start_line, DocLine::new(1));
		assert_eq!(tokens[1].absolute_start_byte, DocByte::new(5));
		assert_eq!(tokens[1].physical_byte_len, 8);
		assert_eq!(tokens[2].text, b"tail".to_vec());
		assert_eq!(tokens[2].absolute_start_line, DocLine::new(3));
		assert_eq!(tokens[2].absolute_start_byte, DocByte::new(13));
	}

	#[test]
	fn query_viewport_starts_on_fold_placeholder_when_target_line_is_hidden() {
		let (registry, root, folded) = build_foldable_document();
		registry.is_folded[folded.index()].store(true, Ordering::Release);

		let tokens = registry.query_viewport(root, DocLine::new(2), 2);

		assert_eq!(tokens.len(), 2);
		assert_eq!(tokens[0].node_id, folded);
		assert!(tokens[0].is_folded);
		assert_eq!(tokens[0].absolute_start_line, DocLine::new(1));
		assert_eq!(tokens[1].text, b"tail".to_vec());
	}

	#[test]
	fn find_node_at_line_col_can_reach_the_line_after_a_folded_block() {
		let (registry, root, folded) = build_foldable_document();
		registry.is_folded[folded.index()].store(true, Ordering::Release);

		let target = registry.find_node_at_line_col(root, DocLine::new(3), VisualCol::ZERO);

		assert_ne!(target.node_id, folded);
		assert_eq!(target.node_byte, crate::core::NodeByteOffset::ZERO);
	}

	#[test]
	fn find_node_at_line_col_returns_fold_boundary_for_hidden_lines() {
		let (registry, root, folded) = build_foldable_document();
		registry.is_folded[folded.index()].store(true, Ordering::Release);

		let target = registry.find_node_at_line_col(root, DocLine::new(2), VisualCol::new(3));
		let raw = registry.find_node_at_line_col_raw(root, DocLine::new(2), VisualCol::new(3));

		assert_eq!(target.node_id, folded);
		assert_eq!(target.node_byte, crate::core::NodeByteOffset::ZERO);
		assert_eq!(target.visual_col, VisualCol::ZERO);
		assert_ne!(raw.node_id, folded);
	}

	#[test]
	fn find_node_at_doc_byte_returns_fold_boundary_for_hidden_bytes() {
		let (registry, root, folded) = build_foldable_document();
		registry.is_folded[folded.index()].store(true, Ordering::Release);

		let target = registry.find_node_at_doc_byte(root, DocByte::new(8));
		let raw = registry.find_node_at_doc_byte_raw(root, DocByte::new(8));

		assert_eq!(target.node_id, folded);
		assert_eq!(target.node_start_byte, DocByte::new(5));
		assert_eq!(target.node_end_byte, DocByte::new(13));
		assert_eq!(target.node_byte, crate::core::NodeByteOffset::ZERO);
		assert_ne!(raw.node_id, folded);
	}

	#[test]
	fn find_node_at_line_col_inflates_unloaded_physical_metrics_on_demand() {
		let path = temp_test_path("projection-uninflated");
		std::fs::write(&path, "aa\nbb\ncc\ndd").expect("write temp file");

		let (registry, root) = build_uninflated_physical_document(&["aa\nbb\n", "cc\ndd"]);
		registry.register_device_path(77, path.to_str().expect("utf8 path"));

		let target = registry.find_node_at_line_col(root, DocLine::new(3), VisualCol::ZERO);
		let first_leaf = registry.get_first_child(root).expect("first leaf");
		let second_leaf = registry.get_next_sibling(first_leaf).expect("second leaf");

		assert_eq!(target.node_id, second_leaf);
		assert_eq!(target.node_byte, crate::core::NodeByteOffset::new(3));
		assert_eq!(target.visual_col, VisualCol::ZERO);
		assert_eq!(
			unsafe { (*registry.metrics[root.index()].get()).newlines },
			3
		);
		assert!(registry.metrics_inflated[first_leaf.index()].load(Ordering::Acquire));
		assert!(registry.metrics_inflated[second_leaf.index()].load(Ordering::Acquire));

		let _ = std::fs::remove_file(path);
	}
}
