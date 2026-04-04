pub mod clipboard;
mod core;
mod folding;
mod layout;
mod minimap;
mod support;
pub mod undo;

pub use self::core::*;
pub use self::layout::VisibleRow;
pub use self::layout::{
	ViewportAnchor, clamp_cursor_to_viewport_anchor, collect_visible_rows, cursor_view_metrics,
	gutter_width_for_max_line, pan_viewport_anchor_to_keep_cursor_visible, scroll_viewport_anchor,
	viewport_geometry, viewport_geometry_for_viewport, viewport_max_line_on_screen,
};
pub use self::minimap::{
	EMPTY_PREVIEW_BIN, MinimapOverlay, MinimapSnapshot, OverviewSnapshot, PREVIEW_BIN_COLUMNS,
	PreviewRow, PreviewSnapshot,
};
