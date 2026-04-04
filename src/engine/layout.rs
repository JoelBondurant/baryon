use super::folding::{
	doc_line_for_visual_index, snap_line_to_visible_boundary, visual_line_index_for_doc_line,
};
use super::support::{line_end_visual_col, read_line_bytes_sparse_with_root_line_index};
use crate::core::{DocLine, VisualCol};
use crate::ecs::{NodeId, UastRegistry};
use crate::uast::{RootChildLineIndex, UastProjection};
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

pub fn viewport_max_line_on_screen(
	viewport_start_line: DocLine,
	viewport_line_count: u32,
	total_lines: u32,
) -> u32 {
	viewport_start_line
		.get()
		.saturating_add(viewport_line_count.saturating_sub(1))
		.min(total_lines)
}

pub fn gutter_width_for_max_line(max_line_on_screen: u32) -> u16 {
	let digits = max_line_on_screen.max(1).ilog10() + 1;
	digits as u16 + 1
}

pub fn viewport_geometry(
	max_line_on_screen: u32,
	screen_width: u16,
	show_minimap: bool,
) -> ViewportGeometry {
	let minimap_width = if show_minimap && screen_width > MINIMAP_MIN_SCREEN_WIDTH {
		MINIMAP_MAX_WIDTH.min(screen_width.saturating_sub(MINIMAP_RESERVED_COLUMNS))
	} else {
		0
	};
	let separator_width = if minimap_width > 0 { 1 } else { 0 };
	let gutter_width = gutter_width_for_max_line(max_line_on_screen);
	let text_width = screen_width.saturating_sub(gutter_width + minimap_width + separator_width);

	ViewportGeometry {
		gutter_width,
		text_width,
		minimap_width,
		separator_width,
	}
}

pub fn viewport_geometry_for_viewport(
	viewport_start_line: DocLine,
	viewport_line_count: u32,
	total_lines: u32,
	screen_width: u16,
	show_minimap: bool,
) -> ViewportGeometry {
	viewport_geometry(
		viewport_max_line_on_screen(viewport_start_line, viewport_line_count, total_lines),
		screen_width,
		show_minimap,
	)
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

fn visible_line_for_layout(
	registry: &UastRegistry,
	root: NodeId,
	line: DocLine,
	total_lines: u32,
	has_closed_folds: bool,
) -> DocLine {
	let line = DocLine::new(line.get().min(total_lines));
	if has_closed_folds {
		snap_line_to_visible_boundary(registry, root, line)
	} else {
		line
	}
}

fn visual_index_for_layout(
	registry: &UastRegistry,
	root: NodeId,
	line: DocLine,
	total_lines: u32,
	has_closed_folds: bool,
) -> u32 {
	if has_closed_folds {
		visual_line_index_for_doc_line(registry, root, line)
	} else {
		line.get().min(total_lines)
	}
}

fn line_for_visual_index_for_layout(
	registry: &UastRegistry,
	root: NodeId,
	target_visual_index: u32,
	total_lines: u32,
	has_closed_folds: bool,
) -> DocLine {
	if has_closed_folds {
		doc_line_for_visual_index(registry, root, target_visual_index, total_lines)
	} else {
		DocLine::new(target_visual_index.min(total_lines))
	}
}

fn is_folded_visible_line_with_root_line_index(
	registry: &UastRegistry,
	root: NodeId,
	line: DocLine,
	line_index: Option<&RootChildLineIndex>,
	has_closed_folds: bool,
) -> bool {
	if !has_closed_folds {
		return false;
	}

	let target = if let Some(line_index) = line_index {
		registry.find_node_at_line_col_with_root_line_index(root, line, VisualCol::ZERO, line_index)
	} else {
		registry.find_node_at_line_col(root, line, VisualCol::ZERO)
	};
	let idx = target.node_id.index();
	let has_children = unsafe { (*registry.edges[idx].get()).first_child.is_some() };
	has_children && registry.is_folded[idx].load(Ordering::Acquire)
}

fn line_row_count_sparse_with_root_line_index(
	registry: &UastRegistry,
	root: NodeId,
	line: DocLine,
	wrap_enabled: bool,
	text_width: u16,
	line_index: Option<&RootChildLineIndex>,
	has_closed_folds: bool,
) -> u32 {
	if !wrap_enabled {
		return 1;
	}
	if is_folded_visible_line_with_root_line_index(
		registry,
		root,
		line,
		line_index,
		has_closed_folds,
	) {
		return row_count_from_visual_width(FOLDED_PLACEHOLDER_COLUMNS, text_width);
	}

	read_line_bytes_sparse_with_root_line_index(registry, root, line, false, line_index)
		.map(|bytes| {
			row_count_from_visual_width(
				line_end_visual_col(&bytes, DocLine::ZERO).get(),
				text_width,
			)
		})
		.unwrap_or(1)
}

fn cursor_row_and_screen_col_with_root_line_index(
	registry: &UastRegistry,
	root: NodeId,
	line: DocLine,
	col: VisualCol,
	wrap_enabled: bool,
	text_width: u16,
	line_index: Option<&RootChildLineIndex>,
	has_closed_folds: bool,
) -> (u32, VisualCol) {
	if !wrap_enabled
		|| text_width == 0
		|| is_folded_visible_line_with_root_line_index(
			registry,
			root,
			line,
			line_index,
			has_closed_folds,
		) {
		return (0, col);
	}

	let width = u32::from(text_width);
	let Ok(bytes) =
		read_line_bytes_sparse_with_root_line_index(registry, root, line, false, line_index)
	else {
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

fn next_visible_line_with_root_line_index(
	registry: &UastRegistry,
	root: NodeId,
	line: DocLine,
	total_lines: u32,
	_line_index: Option<&RootChildLineIndex>,
	has_closed_folds: bool,
) -> Option<DocLine> {
	let line = visible_line_for_layout(registry, root, line, total_lines, has_closed_folds);
	if !has_closed_folds {
		return (line.get() < total_lines).then(|| DocLine::new(line.get() + 1));
	}
	let current_visual =
		visual_index_for_layout(registry, root, line, total_lines, has_closed_folds);
	let next = line_for_visual_index_for_layout(
		registry,
		root,
		current_visual.saturating_add(1),
		total_lines,
		has_closed_folds,
	);
	(next != line).then_some(next)
}

fn previous_visible_line_with_root_line_index(
	registry: &UastRegistry,
	root: NodeId,
	line: DocLine,
	total_lines: u32,
	_line_index: Option<&RootChildLineIndex>,
	has_closed_folds: bool,
) -> Option<DocLine> {
	let line = visible_line_for_layout(registry, root, line, total_lines, has_closed_folds);
	let current_visual =
		visual_index_for_layout(registry, root, line, total_lines, has_closed_folds);
	if current_visual == 0 {
		return None;
	}

	let prev = line_for_visual_index_for_layout(
		registry,
		root,
		current_visual - 1,
		total_lines,
		has_closed_folds,
	);
	(prev != line).then_some(prev)
}

fn normalize_anchor_with_root_line_index(
	registry: &UastRegistry,
	root: NodeId,
	anchor: ViewportAnchor,
	total_lines: u32,
	wrap_enabled: bool,
	text_width: u16,
	line_index: Option<&RootChildLineIndex>,
	has_closed_folds: bool,
) -> ViewportAnchor {
	let line = visible_line_for_layout(registry, root, anchor.line, total_lines, has_closed_folds);
	let max_row_offset = line_row_count_sparse_with_root_line_index(
		registry,
		root,
		line,
		wrap_enabled,
		text_width,
		line_index,
		has_closed_folds,
	)
	.saturating_sub(1);
	ViewportAnchor {
		line,
		row_offset: if wrap_enabled {
			anchor.row_offset.min(max_row_offset)
		} else {
			0
		},
	}
}

fn rows_from_anchor_to_position_with_root_line_index(
	registry: &UastRegistry,
	root: NodeId,
	anchor: ViewportAnchor,
	target_line: DocLine,
	target_row: u32,
	total_lines: u32,
	wrap_enabled: bool,
	text_width: u16,
	line_index: Option<&RootChildLineIndex>,
	has_closed_folds: bool,
) -> u32 {
	if anchor.line == target_line {
		return target_row.saturating_sub(anchor.row_offset);
	}

	let mut rows = line_row_count_sparse_with_root_line_index(
		registry,
		root,
		anchor.line,
		wrap_enabled,
		text_width,
		line_index,
		has_closed_folds,
	)
	.saturating_sub(anchor.row_offset);
	let mut line = anchor.line;
	while let Some(next_line) = next_visible_line_with_root_line_index(
		registry,
		root,
		line,
		total_lines,
		line_index,
		has_closed_folds,
	) {
		line = next_line;
		if line == target_line {
			return rows.saturating_add(target_row);
		}
		rows = rows.saturating_add(line_row_count_sparse_with_root_line_index(
			registry,
			root,
			line,
			wrap_enabled,
			text_width,
			line_index,
			has_closed_folds,
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
	scroll_viewport_anchor_with_root_line_index(
		registry,
		root,
		anchor,
		delta_rows,
		total_lines,
		wrap_enabled,
		text_width,
		None,
		true,
	)
}

pub(crate) fn scroll_viewport_anchor_with_root_line_index(
	registry: &UastRegistry,
	root: NodeId,
	anchor: ViewportAnchor,
	delta_rows: i32,
	total_lines: u32,
	wrap_enabled: bool,
	text_width: u16,
	line_index: Option<&RootChildLineIndex>,
	has_closed_folds: bool,
) -> ViewportAnchor {
	let mut anchor = normalize_anchor_with_root_line_index(
		registry,
		root,
		anchor,
		total_lines,
		wrap_enabled,
		text_width,
		line_index,
		has_closed_folds,
	);
	if !wrap_enabled {
		anchor.row_offset = 0;
	}

	let mut remaining = delta_rows;
	while remaining > 0 {
		let line_rows = line_row_count_sparse_with_root_line_index(
			registry,
			root,
			anchor.line,
			wrap_enabled,
			text_width,
			line_index,
			has_closed_folds,
		);
		let remaining_in_line = line_rows
			.saturating_sub(anchor.row_offset)
			.saturating_sub(1);
		if remaining as u32 <= remaining_in_line {
			anchor.row_offset = anchor.row_offset.saturating_add(remaining as u32);
			return anchor;
		}

		remaining -= remaining_in_line as i32 + 1;
		let Some(next_line) = next_visible_line_with_root_line_index(
			registry,
			root,
			anchor.line,
			total_lines,
			line_index,
			has_closed_folds,
		) else {
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
		let Some(prev_line) = previous_visible_line_with_root_line_index(
			registry,
			root,
			anchor.line,
			total_lines,
			line_index,
			has_closed_folds,
		) else {
			return ViewportAnchor {
				line: DocLine::ZERO,
				row_offset: 0,
			};
		};
		anchor.line = prev_line;
		anchor.row_offset = line_row_count_sparse_with_root_line_index(
			registry,
			root,
			anchor.line,
			wrap_enabled,
			text_width,
			line_index,
			has_closed_folds,
		)
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
	pan_viewport_anchor_to_keep_cursor_visible_with_root_line_index(
		registry,
		root,
		anchor,
		cursor_line,
		cursor_col,
		total_lines,
		viewport_rows,
		wrap_enabled,
		text_width,
		None,
		true,
	)
}

pub(crate) fn pan_viewport_anchor_to_keep_cursor_visible_with_root_line_index(
	registry: &UastRegistry,
	root: NodeId,
	anchor: ViewportAnchor,
	cursor_line: DocLine,
	cursor_col: VisualCol,
	total_lines: u32,
	viewport_rows: u32,
	wrap_enabled: bool,
	text_width: u16,
	line_index: Option<&RootChildLineIndex>,
	has_closed_folds: bool,
) -> ViewportAnchor {
	let viewport_rows = viewport_rows.max(1);
	let anchor = normalize_anchor_with_root_line_index(
		registry,
		root,
		anchor,
		total_lines,
		wrap_enabled,
		text_width,
		line_index,
		has_closed_folds,
	);
	let cursor_line =
		visible_line_for_layout(registry, root, cursor_line, total_lines, has_closed_folds);
	let (cursor_row, _) = cursor_row_and_screen_col_with_root_line_index(
		registry,
		root,
		cursor_line,
		cursor_col,
		wrap_enabled,
		text_width,
		line_index,
		has_closed_folds,
	);
	let anchor_visual =
		visual_index_for_layout(registry, root, anchor.line, total_lines, has_closed_folds);
	let cursor_visual =
		visual_index_for_layout(registry, root, cursor_line, total_lines, has_closed_folds);

	if cursor_visual < anchor_visual
		|| (cursor_visual == anchor_visual && cursor_row < anchor.row_offset)
	{
		return ViewportAnchor {
			line: cursor_line,
			row_offset: if wrap_enabled { cursor_row } else { 0 },
		};
	}

	let cursor_offset = rows_from_anchor_to_position_with_root_line_index(
		registry,
		root,
		anchor,
		cursor_line,
		cursor_row,
		total_lines,
		wrap_enabled,
		text_width,
		line_index,
		has_closed_folds,
	);
	if cursor_offset < viewport_rows {
		anchor
	} else {
		scroll_viewport_anchor_with_root_line_index(
			registry,
			root,
			anchor,
			cursor_offset.saturating_sub(viewport_rows.saturating_sub(1)) as i32,
			total_lines,
			wrap_enabled,
			text_width,
			line_index,
			has_closed_folds,
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
	clamp_cursor_to_viewport_anchor_with_root_line_index(
		registry,
		root,
		anchor,
		cursor_line,
		cursor_col,
		total_lines,
		viewport_rows,
		wrap_enabled,
		text_width,
		None,
		true,
	)
}

pub(crate) fn clamp_cursor_to_viewport_anchor_with_root_line_index(
	registry: &UastRegistry,
	root: NodeId,
	anchor: ViewportAnchor,
	cursor_line: DocLine,
	cursor_col: VisualCol,
	total_lines: u32,
	viewport_rows: u32,
	wrap_enabled: bool,
	text_width: u16,
	line_index: Option<&RootChildLineIndex>,
	has_closed_folds: bool,
) -> (DocLine, VisualCol) {
	let viewport_rows = viewport_rows.max(1);
	let anchor = normalize_anchor_with_root_line_index(
		registry,
		root,
		anchor,
		total_lines,
		wrap_enabled,
		text_width,
		line_index,
		has_closed_folds,
	);
	let cursor_line =
		visible_line_for_layout(registry, root, cursor_line, total_lines, has_closed_folds);
	let (cursor_row, _) = cursor_row_and_screen_col_with_root_line_index(
		registry,
		root,
		cursor_line,
		cursor_col,
		wrap_enabled,
		text_width,
		line_index,
		has_closed_folds,
	);
	let anchor_visual =
		visual_index_for_layout(registry, root, anchor.line, total_lines, has_closed_folds);
	let cursor_visual =
		visual_index_for_layout(registry, root, cursor_line, total_lines, has_closed_folds);

	if cursor_visual < anchor_visual
		|| (cursor_visual == anchor_visual && cursor_row < anchor.row_offset)
	{
		return (
			anchor.line,
			row_start_col(anchor.row_offset, wrap_enabled, text_width),
		);
	}

	let cursor_offset = rows_from_anchor_to_position_with_root_line_index(
		registry,
		root,
		anchor,
		cursor_line,
		cursor_row,
		total_lines,
		wrap_enabled,
		text_width,
		line_index,
		has_closed_folds,
	);
	if cursor_offset < viewport_rows {
		(cursor_line, cursor_col)
	} else {
		let bottom = scroll_viewport_anchor_with_root_line_index(
			registry,
			root,
			anchor,
			viewport_rows.saturating_sub(1) as i32,
			total_lines,
			wrap_enabled,
			text_width,
			line_index,
			has_closed_folds,
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
	cursor_view_metrics_with_root_line_index(
		registry,
		root,
		anchor,
		cursor_line,
		cursor_col,
		total_lines,
		wrap_enabled,
		text_width,
		None,
		true,
	)
}

pub(crate) fn cursor_view_metrics_with_root_line_index(
	registry: &UastRegistry,
	root: NodeId,
	anchor: ViewportAnchor,
	cursor_line: DocLine,
	cursor_col: VisualCol,
	total_lines: u32,
	wrap_enabled: bool,
	text_width: u16,
	line_index: Option<&RootChildLineIndex>,
	has_closed_folds: bool,
) -> (u32, VisualCol) {
	let anchor = normalize_anchor_with_root_line_index(
		registry,
		root,
		anchor,
		total_lines,
		wrap_enabled,
		text_width,
		line_index,
		has_closed_folds,
	);
	let cursor_line =
		visible_line_for_layout(registry, root, cursor_line, total_lines, has_closed_folds);
	let (cursor_row, screen_col) = cursor_row_and_screen_col_with_root_line_index(
		registry,
		root,
		cursor_line,
		cursor_col,
		wrap_enabled,
		text_width,
		line_index,
		has_closed_folds,
	);
	let row = rows_from_anchor_to_position_with_root_line_index(
		registry,
		root,
		anchor,
		cursor_line,
		cursor_row,
		total_lines,
		wrap_enabled,
		text_width,
		line_index,
		has_closed_folds,
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
	collect_visible_rows_with_root_line_index(
		registry,
		root,
		anchor,
		total_lines,
		viewport_rows,
		wrap_enabled,
		text_width,
		None,
		true,
	)
}

pub(crate) fn collect_visible_rows_with_root_line_index(
	registry: &UastRegistry,
	root: NodeId,
	anchor: ViewportAnchor,
	total_lines: u32,
	viewport_rows: u32,
	wrap_enabled: bool,
	text_width: u16,
	line_index: Option<&RootChildLineIndex>,
	has_closed_folds: bool,
) -> Vec<VisibleRow> {
	let mut rows = Vec::new();
	let viewport_rows = viewport_rows.max(1);
	let mut anchor = normalize_anchor_with_root_line_index(
		registry,
		root,
		anchor,
		total_lines,
		wrap_enabled,
		text_width,
		line_index,
		has_closed_folds,
	);
	loop {
		let line_rows = line_row_count_sparse_with_root_line_index(
			registry,
			root,
			anchor.line,
			wrap_enabled,
			text_width,
			line_index,
			has_closed_folds,
		);
		for row_offset in anchor.row_offset..line_rows {
			rows.push(VisibleRow {
				line: anchor.line,
				start_col: row_start_col(row_offset, wrap_enabled, text_width),
			});
			if rows.len() as u32 >= viewport_rows {
				return rows;
			}
		}

		let Some(next_line) = next_visible_line_with_root_line_index(
			registry,
			root,
			anchor.line,
			total_lines,
			line_index,
			has_closed_folds,
		) else {
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
		collect_visible_rows_with_root_line_index, cursor_view_metrics,
		cursor_view_metrics_with_root_line_index, pan_viewport_anchor_to_keep_cursor_visible,
		pan_viewport_anchor_to_keep_cursor_visible_with_root_line_index, viewport_geometry,
		viewport_geometry_for_viewport, viewport_max_line_on_screen,
	};
	use crate::core::{DocLine, VisualCol};
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

	fn temp_test_path(name: &str) -> std::path::PathBuf {
		let nanos = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.expect("time should move forward")
			.as_nanos();
		std::env::temp_dir().join(format!("baryon-{}-{}-{}", name, std::process::id(), nanos))
	}

	fn build_source_backed_line_document(
		line_count: u32,
	) -> (UastRegistry, crate::ecs::NodeId, std::path::PathBuf) {
		let path = temp_test_path("layout-source-backed");
		let total_bytes = line_count
			.saturating_sub(1)
			.saturating_mul(2)
			.saturating_add(1);
		let registry = UastRegistry::new(line_count + 4);
		let mut chunk = registry.reserve_chunk(line_count + 1).expect("OOM");
		let root = chunk.spawn_node(
			SemanticKind::RelationalTable,
			None,
			SpanMetrics {
				byte_length: total_bytes,
				newlines: line_count.saturating_sub(1),
			},
		);

		let mut bytes = Vec::with_capacity(total_bytes as usize);
		let mut byte_offset = 0u64;
		for idx in 0..line_count {
			let part: &[u8] = if idx + 1 == line_count { b"x" } else { b"x\n" };
			bytes.extend_from_slice(part);
			let byte_length = part.len() as u32;
			let leaf = chunk.spawn_node(
				SemanticKind::Token,
				Some(SvpPointer {
					lba: byte_offset / 512,
					byte_length,
					device_id: 77,
					head_trim: (byte_offset % 512) as u16,
				}),
				SpanMetrics {
					byte_length,
					newlines: u32::from(part.ends_with(b"\n")),
				},
			);
			chunk.append_local_child(root, leaf);
			byte_offset += byte_length as u64;
		}

		std::fs::write(&path, bytes).expect("write temp file");
		registry.register_device_path(77, path.to_str().expect("utf8 path"));
		(registry, root, path)
	}

	fn inflated_leaf_count(registry: &UastRegistry, root: crate::ecs::NodeId) -> usize {
		let mut count = 0usize;
		let mut child = registry.get_first_child(root);
		while let Some(node) = child {
			if registry.metrics_inflated[node.index()].load(Ordering::Acquire) {
				count += 1;
			}
			child = registry.get_next_sibling(node);
		}
		count
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
	fn viewport_geometry_shrinks_gutter_when_returning_to_top_of_huge_file() {
		let total_lines = 208_999_999;
		let top_geometry =
			viewport_geometry_for_viewport(DocLine::ZERO, 25, total_lines, 120, true);
		let bottom_geometry = viewport_geometry_for_viewport(
			DocLine::new(total_lines.saturating_sub(24)),
			25,
			total_lines,
			120,
			true,
		);

		assert_eq!(
			viewport_max_line_on_screen(DocLine::ZERO, 25, total_lines),
			24
		);
		assert_eq!(top_geometry.gutter_width, 3);
		assert_eq!(top_geometry.text_width, 102);
		assert_eq!(bottom_geometry.gutter_width, 10);
		assert_eq!(bottom_geometry.text_width, 95);
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

	#[test]
	fn indexed_wrapped_tail_queries_only_inflate_visible_tail_chunks() {
		let viewport_rows = 20u32;
		let (registry, root, path) = build_source_backed_line_document(256);
		let line_index = registry.build_root_child_line_index(root);
		let total_lines = registry.get_total_newlines(root);
		let cursor_line = DocLine::new(total_lines);
		let expected_top =
			DocLine::new(total_lines.saturating_sub(viewport_rows.saturating_sub(1)));
		let geometry =
			viewport_geometry_for_viewport(expected_top, viewport_rows, total_lines, 120, true);
		let anchor = pan_viewport_anchor_to_keep_cursor_visible_with_root_line_index(
			&registry,
			root,
			ViewportAnchor {
				line: expected_top,
				row_offset: 0,
			},
			cursor_line,
			VisualCol::ZERO,
			total_lines,
			viewport_rows,
			true,
			geometry.text_width,
			Some(&line_index),
			false,
		);
		let rows = collect_visible_rows_with_root_line_index(
			&registry,
			root,
			anchor,
			total_lines,
			viewport_rows,
			true,
			geometry.text_width,
			Some(&line_index),
			false,
		);
		let (cursor_row, screen_col) = cursor_view_metrics_with_root_line_index(
			&registry,
			root,
			anchor,
			cursor_line,
			VisualCol::ZERO,
			total_lines,
			true,
			geometry.text_width,
			Some(&line_index),
			false,
		);

		assert_eq!(rows.len(), viewport_rows as usize);
		assert_eq!(rows.first().map(|row| row.line), Some(expected_top));
		assert_eq!(rows.last().map(|row| row.line), Some(cursor_line));
		assert_eq!(cursor_row, viewport_rows.saturating_sub(1));
		assert_eq!(screen_col, VisualCol::ZERO);
		assert!(
			inflated_leaf_count(&registry, root) <= viewport_rows as usize + 2,
			"wrapped tail queries should only inflate a bounded tail window"
		);

		let _ = std::fs::remove_file(path);
	}
}
