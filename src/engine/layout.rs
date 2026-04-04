use super::folding::{
	doc_line_for_visual_index, snap_line_to_visible_boundary, visual_line_index_for_doc_line,
};
use super::support::{line_end_visual_col, read_line_bytes_sparse};
use crate::core::{DocLine, VisualCol};
use crate::ecs::{NodeId, UastRegistry};
use crate::uast::UastProjection;
use std::sync::atomic::Ordering;

const MINIMAP_MIN_SCREEN_WIDTH: u16 = 40;
const MINIMAP_RESERVED_COLUMNS: u16 = 24;
const MINIMAP_MAX_WIDTH: u16 = 14;
const FOLDED_PLACEHOLDER_COLUMNS: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisibleRow {
	pub line: DocLine,
	pub start_col: VisualCol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewportAnchor {
	pub line: DocLine,
	pub row_offset: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewportGeometry {
	pub gutter_width: u16,
	pub text_width: u16,
	pub minimap_width: u16,
	pub separator_width: u16,
}

pub fn viewport_geometry(
	total_lines: u32,
	screen_width: u16,
	show_minimap: bool,
) -> ViewportGeometry {
	let minimap_width = if show_minimap && screen_width > MINIMAP_MIN_SCREEN_WIDTH {
		MINIMAP_MAX_WIDTH.min(screen_width.saturating_sub(MINIMAP_RESERVED_COLUMNS))
	} else {
		0
	};
	let separator_width = if minimap_width > 0 { 1 } else { 0 };
	let digits = total_lines.max(1).ilog10() + 1;
	let gutter_width = digits as u16 + 1;
	let text_width = screen_width.saturating_sub(gutter_width + minimap_width + separator_width);

	ViewportGeometry {
		gutter_width,
		text_width,
		minimap_width,
		separator_width,
	}
}

fn row_count_from_visual_width(visual_width: u32, text_width: u16) -> u32 {
	if text_width == 0 || visual_width == 0 {
		1
	} else {
		visual_width.saturating_sub(1) / u32::from(text_width) + 1
	}
}

fn row_start_col(row_offset: u32, wrap_enabled: bool, text_width: u16) -> VisualCol {
	if wrap_enabled && text_width > 0 {
		VisualCol::new(row_offset.saturating_mul(u32::from(text_width)))
	} else {
		VisualCol::ZERO
	}
}

fn is_folded_visible_line(registry: &UastRegistry, root: NodeId, line: DocLine) -> bool {
	let target = registry.find_node_at_line_col(root, line, VisualCol::ZERO);
	let idx = target.node_id.index();
	let has_children = unsafe { (*registry.edges[idx].get()).first_child.is_some() };
	has_children && registry.is_folded[idx].load(Ordering::Acquire)
}

fn line_row_count_sparse(
	registry: &UastRegistry,
	root: NodeId,
	line: DocLine,
	wrap_enabled: bool,
	text_width: u16,
) -> u32 {
	if !wrap_enabled {
		return 1;
	}
	if is_folded_visible_line(registry, root, line) {
		return row_count_from_visual_width(FOLDED_PLACEHOLDER_COLUMNS, text_width);
	}

	read_line_bytes_sparse(registry, root, line, false)
		.map(|bytes| {
			row_count_from_visual_width(
				line_end_visual_col(&bytes, DocLine::ZERO).get(),
				text_width,
			)
		})
		.unwrap_or(1)
}

fn cursor_row_and_screen_col(
	registry: &UastRegistry,
	root: NodeId,
	line: DocLine,
	col: VisualCol,
	wrap_enabled: bool,
	text_width: u16,
) -> (u32, VisualCol) {
	if !wrap_enabled || text_width == 0 || is_folded_visible_line(registry, root, line) {
		return (0, col);
	}

	let width = u32::from(text_width);
	let Ok(bytes) = read_line_bytes_sparse(registry, root, line, false) else {
		return (0, col);
	};
	let line_end = line_end_visual_col(&bytes, DocLine::ZERO).get();
	let clamped_col = col.get().min(line_end);
	if clamped_col == line_end && line_end > 0 && line_end % width == 0 {
		(
			line_end / width - 1,
			VisualCol::new(width.saturating_sub(1)),
		)
	} else {
		(clamped_col / width, VisualCol::new(clamped_col % width))
	}
}

fn next_visible_line(
	registry: &UastRegistry,
	root: NodeId,
	line: DocLine,
	total_lines: u32,
) -> Option<DocLine> {
	let line = snap_line_to_visible_boundary(registry, root, line);
	let current_visual = visual_line_index_for_doc_line(registry, root, line);
	let next = doc_line_for_visual_index(
		registry,
		root,
		current_visual.saturating_add(1),
		total_lines,
	);
	(next != line).then_some(next)
}

fn previous_visible_line(
	registry: &UastRegistry,
	root: NodeId,
	line: DocLine,
	total_lines: u32,
) -> Option<DocLine> {
	let line = snap_line_to_visible_boundary(registry, root, line);
	let current_visual = visual_line_index_for_doc_line(registry, root, line);
	if current_visual == 0 {
		return None;
	}

	let prev = doc_line_for_visual_index(registry, root, current_visual - 1, total_lines);
	(prev != line).then_some(prev)
}

fn normalize_anchor(
	registry: &UastRegistry,
	root: NodeId,
	anchor: ViewportAnchor,
	total_lines: u32,
	wrap_enabled: bool,
	text_width: u16,
) -> ViewportAnchor {
	let line = snap_line_to_visible_boundary(
		registry,
		root,
		DocLine::new(anchor.line.get().min(total_lines)),
	);
	let max_row_offset =
		line_row_count_sparse(registry, root, line, wrap_enabled, text_width).saturating_sub(1);
	ViewportAnchor {
		line,
		row_offset: if wrap_enabled {
			anchor.row_offset.min(max_row_offset)
		} else {
			0
		},
	}
}

fn rows_from_anchor_to_position(
	registry: &UastRegistry,
	root: NodeId,
	anchor: ViewportAnchor,
	target_line: DocLine,
	target_row: u32,
	total_lines: u32,
	wrap_enabled: bool,
	text_width: u16,
) -> u32 {
	if anchor.line == target_line {
		return target_row.saturating_sub(anchor.row_offset);
	}

	let mut rows = line_row_count_sparse(registry, root, anchor.line, wrap_enabled, text_width)
		.saturating_sub(anchor.row_offset);
	let mut line = anchor.line;
	while let Some(next_line) = next_visible_line(registry, root, line, total_lines) {
		line = next_line;
		if line == target_line {
			return rows.saturating_add(target_row);
		}
		rows = rows.saturating_add(line_row_count_sparse(
			registry,
			root,
			line,
			wrap_enabled,
			text_width,
		));
	}

	rows
}

pub fn scroll_viewport_anchor(
	registry: &UastRegistry,
	root: NodeId,
	anchor: ViewportAnchor,
	delta_rows: i32,
	total_lines: u32,
	wrap_enabled: bool,
	text_width: u16,
) -> ViewportAnchor {
	let mut anchor = normalize_anchor(
		registry,
		root,
		anchor,
		total_lines,
		wrap_enabled,
		text_width,
	);
	if !wrap_enabled {
		anchor.row_offset = 0;
	}

	let mut remaining = delta_rows;
	while remaining > 0 {
		let line_rows =
			line_row_count_sparse(registry, root, anchor.line, wrap_enabled, text_width);
		let remaining_in_line = line_rows
			.saturating_sub(anchor.row_offset)
			.saturating_sub(1);
		if remaining as u32 <= remaining_in_line {
			anchor.row_offset = anchor.row_offset.saturating_add(remaining as u32);
			return anchor;
		}

		remaining -= remaining_in_line as i32 + 1;
		let Some(next_line) = next_visible_line(registry, root, anchor.line, total_lines) else {
			anchor.row_offset = line_rows.saturating_sub(1);
			return anchor;
		};
		anchor.line = next_line;
		anchor.row_offset = 0;
	}

	while remaining < 0 {
		if (-remaining) as u32 <= anchor.row_offset {
			anchor.row_offset = anchor.row_offset.saturating_sub((-remaining) as u32);
			return anchor;
		}

		remaining += anchor.row_offset as i32 + 1;
		let Some(prev_line) = previous_visible_line(registry, root, anchor.line, total_lines)
		else {
			return ViewportAnchor {
				line: DocLine::ZERO,
				row_offset: 0,
			};
		};
		anchor.line = prev_line;
		anchor.row_offset =
			line_row_count_sparse(registry, root, anchor.line, wrap_enabled, text_width)
				.saturating_sub(1);
	}

	anchor
}

pub fn pan_viewport_anchor_to_keep_cursor_visible(
	registry: &UastRegistry,
	root: NodeId,
	anchor: ViewportAnchor,
	cursor_line: DocLine,
	cursor_col: VisualCol,
	total_lines: u32,
	viewport_rows: u32,
	wrap_enabled: bool,
	text_width: u16,
) -> ViewportAnchor {
	let viewport_rows = viewport_rows.max(1);
	let anchor = normalize_anchor(
		registry,
		root,
		anchor,
		total_lines,
		wrap_enabled,
		text_width,
	);
	let cursor_line = snap_line_to_visible_boundary(registry, root, cursor_line);
	let (cursor_row, _) = cursor_row_and_screen_col(
		registry,
		root,
		cursor_line,
		cursor_col,
		wrap_enabled,
		text_width,
	);
	let anchor_visual = visual_line_index_for_doc_line(registry, root, anchor.line);
	let cursor_visual = visual_line_index_for_doc_line(registry, root, cursor_line);

	if cursor_visual < anchor_visual
		|| (cursor_visual == anchor_visual && cursor_row < anchor.row_offset)
	{
		return ViewportAnchor {
			line: cursor_line,
			row_offset: if wrap_enabled { cursor_row } else { 0 },
		};
	}

	let cursor_offset = rows_from_anchor_to_position(
		registry,
		root,
		anchor,
		cursor_line,
		cursor_row,
		total_lines,
		wrap_enabled,
		text_width,
	);
	if cursor_offset < viewport_rows {
		anchor
	} else {
		scroll_viewport_anchor(
			registry,
			root,
			anchor,
			cursor_offset.saturating_sub(viewport_rows.saturating_sub(1)) as i32,
			total_lines,
			wrap_enabled,
			text_width,
		)
	}
}

pub fn clamp_cursor_to_viewport_anchor(
	registry: &UastRegistry,
	root: NodeId,
	anchor: ViewportAnchor,
	cursor_line: DocLine,
	cursor_col: VisualCol,
	total_lines: u32,
	viewport_rows: u32,
	wrap_enabled: bool,
	text_width: u16,
) -> (DocLine, VisualCol) {
	let viewport_rows = viewport_rows.max(1);
	let anchor = normalize_anchor(
		registry,
		root,
		anchor,
		total_lines,
		wrap_enabled,
		text_width,
	);
	let cursor_line = snap_line_to_visible_boundary(registry, root, cursor_line);
	let (cursor_row, _) = cursor_row_and_screen_col(
		registry,
		root,
		cursor_line,
		cursor_col,
		wrap_enabled,
		text_width,
	);
	let anchor_visual = visual_line_index_for_doc_line(registry, root, anchor.line);
	let cursor_visual = visual_line_index_for_doc_line(registry, root, cursor_line);

	if cursor_visual < anchor_visual
		|| (cursor_visual == anchor_visual && cursor_row < anchor.row_offset)
	{
		return (
			anchor.line,
			row_start_col(anchor.row_offset, wrap_enabled, text_width),
		);
	}

	let cursor_offset = rows_from_anchor_to_position(
		registry,
		root,
		anchor,
		cursor_line,
		cursor_row,
		total_lines,
		wrap_enabled,
		text_width,
	);
	if cursor_offset < viewport_rows {
		(cursor_line, cursor_col)
	} else {
		let bottom = scroll_viewport_anchor(
			registry,
			root,
			anchor,
			viewport_rows.saturating_sub(1) as i32,
			total_lines,
			wrap_enabled,
			text_width,
		);
		(
			bottom.line,
			row_start_col(bottom.row_offset, wrap_enabled, text_width),
		)
	}
}

pub fn cursor_view_metrics(
	registry: &UastRegistry,
	root: NodeId,
	anchor: ViewportAnchor,
	cursor_line: DocLine,
	cursor_col: VisualCol,
	total_lines: u32,
	wrap_enabled: bool,
	text_width: u16,
) -> (u32, VisualCol) {
	let anchor = normalize_anchor(
		registry,
		root,
		anchor,
		total_lines,
		wrap_enabled,
		text_width,
	);
	let cursor_line = snap_line_to_visible_boundary(registry, root, cursor_line);
	let (cursor_row, screen_col) = cursor_row_and_screen_col(
		registry,
		root,
		cursor_line,
		cursor_col,
		wrap_enabled,
		text_width,
	);
	let row = rows_from_anchor_to_position(
		registry,
		root,
		anchor,
		cursor_line,
		cursor_row,
		total_lines,
		wrap_enabled,
		text_width,
	);
	(row, screen_col)
}

pub fn collect_visible_rows(
	registry: &UastRegistry,
	root: NodeId,
	anchor: ViewportAnchor,
	total_lines: u32,
	viewport_rows: u32,
	wrap_enabled: bool,
	text_width: u16,
) -> Vec<VisibleRow> {
	let mut rows = Vec::new();
	let viewport_rows = viewport_rows.max(1);
	let mut anchor = normalize_anchor(
		registry,
		root,
		anchor,
		total_lines,
		wrap_enabled,
		text_width,
	);
	loop {
		let line_rows =
			line_row_count_sparse(registry, root, anchor.line, wrap_enabled, text_width);
		for row_offset in anchor.row_offset..line_rows {
			rows.push(VisibleRow {
				line: anchor.line,
				start_col: row_start_col(row_offset, wrap_enabled, text_width),
			});
			if rows.len() as u32 >= viewport_rows {
				return rows;
			}
		}

		let Some(next_line) = next_visible_line(registry, root, anchor.line, total_lines) else {
			return rows;
		};
		anchor.line = next_line;
		anchor.row_offset = 0;
	}
}

#[cfg(test)]
mod tests {
	use super::{
		ViewportAnchor, VisibleRow, clamp_cursor_to_viewport_anchor, collect_visible_rows,
		cursor_view_metrics, pan_viewport_anchor_to_keep_cursor_visible, viewport_geometry,
	};
	use crate::core::{DocLine, VisualCol};
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
	fn viewport_geometry_matches_draw_constraints() {
		let geometry = viewport_geometry(999, 120, true);
		assert_eq!(geometry.gutter_width, 4);
		assert_eq!(geometry.minimap_width, 14);
		assert_eq!(geometry.separator_width, 1);
		assert_eq!(geometry.text_width, 101);
	}

	#[test]
	fn collect_visible_rows_can_start_mid_wrapped_line() {
		let (registry, root) = build_document("abcdefghij\nz\n");
		let rows = collect_visible_rows(
			&registry,
			root,
			ViewportAnchor {
				line: DocLine::ZERO,
				row_offset: 1,
			},
			registry.get_total_newlines(root),
			4,
			true,
			4,
		);

		assert_eq!(
			rows,
			vec![
				VisibleRow {
					line: DocLine::ZERO,
					start_col: VisualCol::new(4),
				},
				VisibleRow {
					line: DocLine::ZERO,
					start_col: VisualCol::new(8),
				},
				VisibleRow {
					line: DocLine::new(1),
					start_col: VisualCol::ZERO,
				},
				VisibleRow {
					line: DocLine::new(2),
					start_col: VisualCol::ZERO,
				},
			]
		);
	}

	#[test]
	fn cursor_metrics_and_clamp_respect_wrapped_offsets() {
		let (registry, root) = build_document("abcdefghij\nz\n");
		let total_lines = registry.get_total_newlines(root);
		let anchor = ViewportAnchor {
			line: DocLine::ZERO,
			row_offset: 1,
		};

		let (cursor_row, screen_col) = cursor_view_metrics(
			&registry,
			root,
			anchor,
			DocLine::ZERO,
			VisualCol::new(9),
			total_lines,
			true,
			4,
		);
		assert_eq!(cursor_row, 1);
		assert_eq!(screen_col, VisualCol::new(1));

		let clamped = clamp_cursor_to_viewport_anchor(
			&registry,
			root,
			anchor,
			DocLine::ZERO,
			VisualCol::ZERO,
			total_lines,
			2,
			true,
			4,
		);
		assert_eq!(clamped, (DocLine::ZERO, VisualCol::new(4)));

		let panned = pan_viewport_anchor_to_keep_cursor_visible(
			&registry,
			root,
			ViewportAnchor {
				line: DocLine::ZERO,
				row_offset: 0,
			},
			DocLine::ZERO,
			VisualCol::new(9),
			total_lines,
			2,
			true,
			4,
		);
		assert_eq!(
			panned,
			ViewportAnchor {
				line: DocLine::ZERO,
				row_offset: 1,
			}
		);
	}
}
