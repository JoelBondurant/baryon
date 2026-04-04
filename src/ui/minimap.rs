use crate::engine::{
	EMPTY_PREVIEW_BIN, MinimapOverlay, MinimapSnapshot, OverviewSnapshot, PREVIEW_BIN_COLUMNS,
	PreviewRow, PreviewSnapshot,
};
use crate::svp::highlight::{CATEGORY_COUNT, TokenCategory};
use image::{DynamicImage, Rgba, RgbaImage};
use ratatui::{
	Frame,
	buffer::Buffer,
	layout::Rect,
	style::{Color, Style},
};
use ratatui_image::{Image, Resize, picker::Picker, protocol::Protocol};
use std::{
	collections::hash_map::DefaultHasher,
	hash::{Hash, Hasher},
	sync::mpsc,
	thread,
};

#[derive(Clone)]
struct PreviewRequest {
	fingerprint: u64,
	snapshot: PreviewSnapshot,
	area: Rect,
	theme_colors: [Option<Color>; CATEGORY_COUNT],
}

struct PreviewResponse {
	fingerprint: u64,
	area: Rect,
	protocol: Option<Protocol>,
}

pub(super) struct MinimapController {
	tx_request: mpsc::Sender<PreviewRequest>,
	rx_ready: mpsc::Receiver<PreviewResponse>,
	requested_fingerprint: Option<u64>,
	pending_fingerprint: Option<u64>,
	current_fingerprint: Option<u64>,
	current_area: Option<Rect>,
	current_protocol: Option<Protocol>,
}

impl MinimapController {
	pub fn new() -> Self {
		let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
		let (tx_request, rx_request) = mpsc::channel::<PreviewRequest>();
		let (tx_ready, rx_ready) = mpsc::channel::<PreviewResponse>();

		thread::spawn(move || {
			while let Ok(mut request) = rx_request.recv() {
				while let Ok(next_request) = rx_request.try_recv() {
					request = next_request;
				}

				let protocol = render_preview_protocol(
					&picker,
					&request.snapshot,
					request.area,
					&request.theme_colors,
				);
				if tx_ready
					.send(PreviewResponse {
						fingerprint: request.fingerprint,
						area: request.area,
						protocol,
					})
					.is_err()
				{
					break;
				}
			}
		});

		Self {
			tx_request,
			rx_ready,
			requested_fingerprint: None,
			pending_fingerprint: None,
			current_fingerprint: None,
			current_area: None,
			current_protocol: None,
		}
	}

	pub fn request_preview(
		&mut self,
		preview: &PreviewSnapshot,
		area: Rect,
		theme_colors: [Option<Color>; CATEGORY_COUNT],
	) {
		if area.width == 0 || area.height == 0 {
			return;
		}

		let fingerprint = preview_request_fingerprint(preview, area, &theme_colors);
		if self.requested_fingerprint == Some(fingerprint)
			|| self.pending_fingerprint == Some(fingerprint)
			|| self.current_fingerprint == Some(fingerprint)
		{
			self.requested_fingerprint = Some(fingerprint);
			return;
		}

		self.requested_fingerprint = Some(fingerprint);
		self.pending_fingerprint = Some(fingerprint);
		let _ = self.tx_request.send(PreviewRequest {
			fingerprint,
			snapshot: preview.clone(),
			area,
			theme_colors,
		});
	}

	pub fn poll(&mut self) -> bool {
		let mut updated = false;
		while let Ok(response) = self.rx_ready.try_recv() {
			if self.requested_fingerprint == Some(response.fingerprint) {
				self.current_fingerprint = Some(response.fingerprint);
				self.current_area = Some(response.area);
				self.current_protocol = response.protocol;
				self.pending_fingerprint = None;
				updated = true;
			}
		}
		updated
	}

	pub fn render(&self, frame: &mut Frame<'_>, area: Rect) -> bool {
		if area.width == 0 || area.height == 0 {
			return false;
		}
		if self.current_area != Some(area) {
			return false;
		}
		let Some(protocol) = &self.current_protocol else {
			return false;
		};
		frame.render_widget(Image::new(protocol), area);
		true
	}
}

fn render_preview_protocol(
	picker: &Picker,
	snapshot: &PreviewSnapshot,
	area: Rect,
	theme_colors: &[Option<Color>; CATEGORY_COUNT],
) -> Option<Protocol> {
	let image = render_preview_image(snapshot, area, theme_colors, picker.font_size());
	picker.new_protocol(image, area, Resize::Fit(None)).ok()
}

fn preview_request_fingerprint(
	snapshot: &PreviewSnapshot,
	area: Rect,
	theme_colors: &[Option<Color>; CATEGORY_COUNT],
) -> u64 {
	let mut hasher = DefaultHasher::new();
	area.x.hash(&mut hasher);
	area.y.hash(&mut hasher);
	area.width.hash(&mut hasher);
	area.height.hash(&mut hasher);
	snapshot.overlay.total_lines.hash(&mut hasher);
	snapshot.overlay.viewport_start_line.get().hash(&mut hasher);
	snapshot.overlay.viewport_end_line.get().hash(&mut hasher);
	snapshot.overlay.viewport_line_count.hash(&mut hasher);
	snapshot.overlay.cursor_line.get().hash(&mut hasher);
	snapshot.overlay.search_bands.hash(&mut hasher);
	snapshot.overlay.active_search_band.hash(&mut hasher);
	snapshot.max_columns.hash(&mut hasher);
	snapshot.rows.len().hash(&mut hasher);
	for row in &snapshot.rows {
		row.logical_width.hash(&mut hasher);
		hasher.write(&row.bins);
	}
	for color in theme_colors {
		match color {
			Some(color) => {
				1u8.hash(&mut hasher);
				let (r, g, b) = color_components(*color);
				r.hash(&mut hasher);
				g.hash(&mut hasher);
				b.hash(&mut hasher);
			}
			None => 0u8.hash(&mut hasher),
		}
	}
	hasher.finish()
}

fn rgba(color: [u8; 4]) -> Rgba<u8> {
	Rgba(color)
}

fn color_components(color: Color) -> (u8, u8, u8) {
	match color {
		Color::Rgb(r, g, b) => (r, g, b),
		Color::Black => (0, 0, 0),
		Color::White => (255, 255, 255),
		Color::DarkGray => (96, 96, 96),
		Color::Gray => (160, 160, 160),
		Color::LightRed => (255, 128, 128),
		Color::LightGreen => (128, 255, 128),
		Color::LightYellow => (255, 255, 128),
		Color::LightBlue => (128, 128, 255),
		Color::LightMagenta => (255, 128, 255),
		Color::LightCyan => (128, 255, 255),
		Color::Red => (224, 92, 92),
		Color::Green => (92, 224, 92),
		Color::Yellow => (224, 224, 92),
		Color::Blue => (92, 92, 224),
		Color::Magenta => (224, 92, 224),
		Color::Cyan => (92, 224, 224),
		Color::Indexed(value) => (value, value, value),
		Color::Reset => (18, 18, 18),
	}
}

fn blend_colors(base: Color, accent: Color, accent_weight: u16, total_weight: u16) -> Color {
	let (br, bg, bb) = color_components(base);
	let (ar, ag, ab) = color_components(accent);
	let base_weight = total_weight.saturating_sub(accent_weight);
	Color::Rgb(
		((br as u16 * base_weight + ar as u16 * accent_weight) / total_weight) as u8,
		((bg as u16 * base_weight + ag as u16 * accent_weight) / total_weight) as u8,
		((bb as u16 * base_weight + ab as u16 * accent_weight) / total_weight) as u8,
	)
}

fn blend_rgba(base: Rgba<u8>, accent: Rgba<u8>, accent_weight: u16, total_weight: u16) -> Rgba<u8> {
	let base_weight = total_weight.saturating_sub(accent_weight);
	Rgba([
		((base[0] as u16 * base_weight + accent[0] as u16 * accent_weight) / total_weight) as u8,
		((base[1] as u16 * base_weight + accent[1] as u16 * accent_weight) / total_weight) as u8,
		((base[2] as u16 * base_weight + accent[2] as u16 * accent_weight) / total_weight) as u8,
		255,
	])
}

fn set_minimap_cell(buf: &mut Buffer, x: u16, y: u16, color: Color) {
	if let Some(cell) = buf.cell_mut((x, y)) {
		cell.set_char(' ')
			.set_style(Style::default().bg(color).fg(color));
	}
}

fn set_minimap_glyph(buf: &mut Buffer, x: u16, y: u16, glyph: char, fg: Color, bg: Color) {
	if let Some(cell) = buf.cell_mut((x, y)) {
		cell.set_char(glyph)
			.set_style(Style::default().fg(fg).bg(bg));
	}
}

fn tint_minimap_cell_bg(buf: &mut Buffer, x: u16, y: u16, color: Color) {
	if let Some(cell) = buf.cell_mut((x, y)) {
		cell.bg = color;
	}
}

fn fill_minimap_area(buf: &mut Buffer, area: Rect, color: Color) {
	for row in 0..area.height {
		for col in 0..area.width {
			set_minimap_cell(buf, area.x + col, area.y + row, color);
		}
	}
}

fn fill_image_rect(image: &mut RgbaImage, x0: u32, y0: u32, x1: u32, y1: u32, color: Rgba<u8>) {
	let max_x = image.width();
	let max_y = image.height();
	for y in y0.min(max_y)..y1.min(max_y) {
		for x in x0.min(max_x)..x1.min(max_x) {
			image.put_pixel(x, y, color);
		}
	}
}

fn tint_image_rect(
	image: &mut RgbaImage,
	x0: u32,
	y0: u32,
	x1: u32,
	y1: u32,
	tint: Rgba<u8>,
	accent_weight: u16,
	total_weight: u16,
) {
	let max_x = image.width();
	let max_y = image.height();
	for y in y0.min(max_y)..y1.min(max_y) {
		for x in x0.min(max_x)..x1.min(max_x) {
			let current = *image.get_pixel(x, y);
			image.put_pixel(x, y, blend_rgba(current, tint, accent_weight, total_weight));
		}
	}
}

fn marker_gutter_width(area: Rect) -> u16 {
	if area.width > 2 { 1 } else { 0 }
}

fn render_search_markers(
	buf: &mut Buffer,
	area: Rect,
	content_height: u16,
	overlay: &MinimapOverlay,
) {
	use crate::ui::*;

	let marker_gutter = marker_gutter_width(area);
	if marker_gutter == 0 || content_height == 0 {
		return;
	}

	let search_active_bg = Color::Rgb(
		MINIMAP_RGBA_SEARCH_ACTIVE[0],
		MINIMAP_RGBA_SEARCH_ACTIVE[1],
		MINIMAP_RGBA_SEARCH_ACTIVE[2],
	);
	let search_inactive_bg = Color::Rgb(
		MINIMAP_RGBA_SEARCH_INACTIVE[0],
		MINIMAP_RGBA_SEARCH_INACTIVE[1],
		MINIMAP_RGBA_SEARCH_INACTIVE[2],
	);
	let marker_width = marker_gutter.min(area.width);
	let marker_x = area.x + area.width.saturating_sub(marker_width);
	for (band_idx, intensity) in overlay.search_bands.iter().copied().enumerate() {
		if intensity == 0 {
			continue;
		}
		let y0 =
			((band_idx as u32) * content_height as u32) / overlay.search_bands.len().max(1) as u32;
		let y1 = (((band_idx as u32) + 1) * content_height as u32)
			/ overlay.search_bands.len().max(1) as u32;
		let color = if overlay.active_search_band == Some(band_idx) {
			search_active_bg
		} else {
			search_inactive_bg
		};
		for row in y0.min(content_height.saturating_sub(1) as u32)
			..y1.max(y0 + 1).min(content_height as u32)
		{
			for x in 0..marker_width {
				set_minimap_cell(buf, marker_x + x, area.y + row as u16, color);
			}
		}
	}
}

fn render_search_markers_image(
	image: &mut RgbaImage,
	content_width_px: u32,
	content_height_px: u32,
	overlay: &MinimapOverlay,
) {
	use crate::ui::*;

	if content_height_px == 0 || content_width_px >= image.width() {
		return;
	}

	let marker_x_px = content_width_px;
	let search_active_bg = rgba(MINIMAP_RGBA_SEARCH_ACTIVE);
	let search_inactive_bg = rgba(MINIMAP_RGBA_SEARCH_INACTIVE);
	for (band_idx, intensity) in overlay.search_bands.iter().copied().enumerate() {
		if intensity == 0 {
			continue;
		}
		let y0 = (band_idx as u32 * content_height_px) / overlay.search_bands.len().max(1) as u32;
		let y1 =
			((band_idx as u32 + 1) * content_height_px) / overlay.search_bands.len().max(1) as u32;
		let color = if overlay.active_search_band == Some(band_idx) {
			search_active_bg
		} else {
			search_inactive_bg
		};
		fill_image_rect(
			image,
			marker_x_px,
			y0.min(content_height_px.saturating_sub(1)),
			image.width(),
			y1.max(y0 + 1).min(content_height_px),
			color,
		);
	}
}

fn overlay_bounds(overlay: &MinimapOverlay, content_height: u16) -> Option<(u16, u16, u16)> {
	if content_height == 0 {
		return None;
	}

	let total_lines = overlay.total_lines.max(1);
	let viewport_start = overlay.viewport_start_line.get().min(total_lines - 1);
	let viewport_end = overlay
		.viewport_end_line
		.get()
		.max(viewport_start.saturating_add(1))
		.min(total_lines);
	let viewport_top =
		((viewport_start as u64) * content_height as u64 / total_lines as u64) as u16;
	let viewport_bottom =
		((viewport_end as u64) * content_height as u64 / total_lines as u64) as u16;
	let viewport_bottom = viewport_bottom.max(viewport_top + 1).min(content_height);
	let cursor_row = ((overlay.cursor_line.get().min(total_lines - 1) as u64)
		* content_height as u64
		/ total_lines as u64) as u16;
	Some((
		viewport_top,
		viewport_bottom,
		cursor_row.min(content_height - 1),
	))
}

fn render_overview_overlay(
	buf: &mut Buffer,
	area: Rect,
	content_width: u16,
	base_color: Color,
	overlay: &MinimapOverlay,
) {
	use crate::ui::*;

	let Some((viewport_top, viewport_bottom, cursor_row)) = overlay_bounds(overlay, area.height)
	else {
		return;
	};
	let viewport_bg = Color::Rgb(30, 30, 30);
	let viewport_content_bg = blend_colors(base_color, viewport_bg, 2, 3);
	let frame_bg = MINIMAP_VIEWPORT_FRAME;
	let frame_side_bg = MINIMAP_VIEWPORT_FRAME;
	let cursor_bg = MINIMAP_CURSOR_LINE;
	let frame_right = area.x + content_width.saturating_sub(1);

	for row in viewport_top.min(area.height.saturating_sub(1))..viewport_bottom {
		for col in 0..content_width {
			set_minimap_cell(buf, area.x + col, area.y + row, viewport_content_bg);
		}
	}

	let frame_top = viewport_top.min(area.height.saturating_sub(1));
	let frame_bottom = viewport_bottom
		.saturating_sub(1)
		.min(area.height.saturating_sub(1));
	for col in 0..content_width {
		let x = area.x + col;
		set_minimap_cell(buf, x, area.y + frame_top, frame_bg);
		set_minimap_cell(buf, x, area.y + frame_bottom, frame_bg);
	}
	for row in frame_top..=frame_bottom {
		set_minimap_cell(buf, area.x, area.y + row, frame_side_bg);
		set_minimap_cell(buf, frame_right, area.y + row, frame_side_bg);
	}

	for col in 0..content_width {
		set_minimap_cell(
			buf,
			area.x + col,
			area.y + cursor_row.min(area.height - 1),
			cursor_bg,
		);
	}
}

fn render_preview_overlay(
	buf: &mut Buffer,
	area: Rect,
	content_width: u16,
	content_height: u16,
	overlay: &MinimapOverlay,
) {
	use crate::ui::*;

	let Some((viewport_top, viewport_bottom, cursor_row)) = overlay_bounds(overlay, content_height)
	else {
		return;
	};
	let frame_bg = blend_colors(
		Color::Rgb(MINIMAP_RGBA_BG[0], MINIMAP_RGBA_BG[1], MINIMAP_RGBA_BG[2]),
		MINIMAP_VIEWPORT_FRAME,
		3,
		4,
	);
	let frame_side_bg = frame_bg;
	let cursor_bg = MINIMAP_CURSOR_LINE;
	let frame_right = area.x + content_width.saturating_sub(1);
	let frame_top = viewport_top.min(content_height.saturating_sub(1));
	let frame_bottom = viewport_bottom
		.saturating_sub(1)
		.min(content_height.saturating_sub(1));

	for col in 0..content_width {
		let x = area.x + col;
		tint_minimap_cell_bg(buf, x, area.y + frame_top, frame_bg);
		tint_minimap_cell_bg(buf, x, area.y + frame_bottom, frame_bg);
	}
	for row in frame_top..=frame_bottom {
		tint_minimap_cell_bg(buf, area.x, area.y + row, frame_side_bg);
		tint_minimap_cell_bg(buf, frame_right, area.y + row, frame_side_bg);
	}
	for col in 0..content_width {
		tint_minimap_cell_bg(
			buf,
			area.x + col,
			area.y + cursor_row.min(content_height - 1),
			cursor_bg,
		);
	}
}

fn render_preview_overlay_image(
	image: &mut RgbaImage,
	content_width_px: u32,
	content_height_px: u32,
	overlay: &MinimapOverlay,
) {
	use crate::ui::*;

	if content_width_px == 0 || content_height_px == 0 {
		return;
	}
	let frame_bg = rgba(MINIMAP_RGBA_VIEWPORT_FRAME);
	let cursor_bg = rgba(MINIMAP_RGBA_CURSOR_LINE);
	let total_lines = overlay.total_lines.max(1);
	let viewport_start = overlay.viewport_start_line.get().min(total_lines - 1);
	let viewport_end = overlay
		.viewport_end_line
		.get()
		.max(viewport_start.saturating_add(1))
		.min(total_lines);
	let cursor_line = overlay.cursor_line.get().min(total_lines - 1);
	let frame_top_px =
		((viewport_start as u64) * content_height_px as u64 / total_lines as u64) as u32;
	let frame_bottom_px =
		((viewport_end as u64) * content_height_px as u64 / total_lines as u64) as u32;
	let frame_bottom_px = frame_bottom_px.max(frame_top_px.saturating_add(1));
	let cursor_top_px =
		((cursor_line as u64) * content_height_px as u64 / total_lines as u64) as u32;
	let cursor_bottom_px =
		(((cursor_line as u64 + 1) * content_height_px as u64) / total_lines as u64) as u32;
	let cursor_bottom_px = cursor_bottom_px.max(cursor_top_px.saturating_add(1));
	let horizontal_thickness = if content_height_px >= 180 { 2 } else { 1 };
	let vertical_thickness = if content_width_px >= 80 { 2 } else { 1 };
	let cursor_line_top = cursor_top_px
		.saturating_add(
			cursor_bottom_px
				.saturating_sub(cursor_top_px)
				.saturating_sub(horizontal_thickness)
				/ 2,
		)
		.min(content_height_px.saturating_sub(horizontal_thickness));
	let cursor_line_bottom = (cursor_line_top + horizontal_thickness).min(content_height_px);
	tint_image_rect(
		image,
		0,
		frame_top_px,
		content_width_px,
		(frame_top_px + horizontal_thickness).min(content_height_px),
		frame_bg,
		1,
		1,
	);
	tint_image_rect(
		image,
		0,
		frame_bottom_px
			.saturating_sub(horizontal_thickness)
			.min(content_height_px.saturating_sub(1)),
		content_width_px,
		frame_bottom_px.min(content_height_px),
		frame_bg,
		1,
		1,
	);
	tint_image_rect(
		image,
		0,
		frame_top_px,
		vertical_thickness,
		frame_bottom_px.min(content_height_px),
		frame_bg,
		1,
		1,
	);
	tint_image_rect(
		image,
		content_width_px.saturating_sub(vertical_thickness),
		frame_top_px,
		content_width_px,
		frame_bottom_px.min(content_height_px),
		frame_bg,
		1,
		1,
	);
	fill_image_rect(
		image,
		0,
		cursor_line_top,
		content_width_px,
		cursor_line_bottom,
		cursor_bg,
	);
}

fn sample_preview_bin(row: &PreviewRow, cell_col: u16, content_width: u16) -> u8 {
	sample_preview_bin_range(
		row,
		cell_col as usize,
		(cell_col as usize).saturating_add(1),
		content_width.max(1) as usize,
	)
}

fn sample_preview_half_bin(row: &PreviewRow, cell_col: u16, half: usize, content_width: u16) -> u8 {
	if half > 1 {
		return EMPTY_PREVIEW_BIN;
	}

	let total_half_columns = content_width.max(1) as usize * 2;
	let start_half = (cell_col as usize).saturating_mul(2).saturating_add(half);
	sample_preview_bin_range(
		row,
		start_half,
		start_half.saturating_add(1),
		total_half_columns,
	)
}

fn sample_preview_bin_range(
	row: &PreviewRow,
	range_start: usize,
	range_end: usize,
	total_columns: usize,
) -> u8 {
	if total_columns == 0 || row.logical_width == 0 {
		return EMPTY_PREVIEW_BIN;
	}

	let start_bin = (range_start * PREVIEW_BIN_COLUMNS) / total_columns;
	let end_bin = ((range_end * PREVIEW_BIN_COLUMNS) + total_columns - 1) / total_columns;
	let start_bin = start_bin.min(PREVIEW_BIN_COLUMNS - 1);
	let end_bin = end_bin.max(start_bin + 1).min(PREVIEW_BIN_COLUMNS);
	let mut counts = [0u16; CATEGORY_COUNT];
	let mut saw_unclassified = false;

	for &bin in &row.bins[start_bin..end_bin] {
		if bin == EMPTY_PREVIEW_BIN {
			continue;
		}
		if bin as usize >= CATEGORY_COUNT {
			continue;
		}
		counts[bin as usize] = counts[bin as usize].saturating_add(1);
		if bin as usize == TokenCategory::Unclassified as usize {
			saw_unclassified = true;
		}
	}

	let mut best: Option<(usize, u16)> = None;
	for (idx, &count) in counts.iter().enumerate() {
		if count == 0 {
			continue;
		}
		match best {
			Some((_, best_count)) if best_count >= count => {}
			_ => best = Some((idx, count)),
		}
	}

	if let Some((idx, _)) = best {
		idx as u8
	} else if saw_unclassified {
		TokenCategory::Unclassified as u8
	} else {
		EMPTY_PREVIEW_BIN
	}
}

fn preview_braille_glyph(left_active: bool, right_active: bool) -> Option<char> {
	let mut mask = 0u8;
	if left_active {
		mask |= 0x02;
	}
	if right_active {
		mask |= 0x10;
	}
	(mask != 0).then(|| char::from_u32(0x2800 + mask as u32).unwrap_or(' '))
}

fn preview_bin_color(
	bin: u8,
	theme_colors: &[Option<Color>; CATEGORY_COUNT],
	base_bg: Color,
) -> Option<Color> {
	if bin == EMPTY_PREVIEW_BIN {
		return None;
	}

	let accent = theme_colors
		.get(bin as usize)
		.and_then(|color| *color)
		.unwrap_or(crate::ui::TEXT_FG);
	Some(blend_colors(base_bg, accent, 5, 6))
}

fn preview_bin_rgba(
	bin: u8,
	theme_colors: &[Option<Color>; CATEGORY_COUNT],
	base_bg: Rgba<u8>,
) -> Option<Rgba<u8>> {
	if bin == EMPTY_PREVIEW_BIN {
		return None;
	}

	let accent = theme_colors
		.get(bin as usize)
		.and_then(|color| *color)
		.unwrap_or(crate::ui::TEXT_FG);
	let (r, g, b) = color_components(accent);
	Some(blend_rgba(base_bg, Rgba([r, g, b, 255]), 5, 6))
}

fn dominant_preview_bin_for_rows(
	rows: &[PreviewRow],
	x_start: usize,
	x_end: usize,
	total_columns: usize,
) -> Option<(u8, u16, u16)> {
	if rows.is_empty() || total_columns == 0 {
		return None;
	}

	let mut counts = [0u16; CATEGORY_COUNT];
	let mut non_empty = 0u16;
	for row in rows {
		let bin = sample_preview_bin_range(row, x_start, x_end, total_columns);
		if bin == EMPTY_PREVIEW_BIN {
			continue;
		}
		let idx = bin as usize;
		if idx >= CATEGORY_COUNT {
			continue;
		}
		counts[idx] = counts[idx].saturating_add(1);
		non_empty = non_empty.saturating_add(1);
	}

	if non_empty == 0 {
		return None;
	}

	let mut best_idx = TokenCategory::Unclassified as usize;
	let mut best_count = 0u16;
	for (idx, &count) in counts.iter().enumerate() {
		if count > best_count {
			best_idx = idx;
			best_count = count;
		}
	}

	Some((
		best_idx as u8,
		non_empty,
		rows.len().min(u16::MAX as usize) as u16,
	))
}

fn render_preview_image(
	snapshot: &PreviewSnapshot,
	area: Rect,
	theme_colors: &[Option<Color>; CATEGORY_COUNT],
	font_size: (u16, u16),
) -> DynamicImage {
	use crate::ui::*;

	let font_w = font_size.0.max(1) as u32;
	let font_h = font_size.1.max(1) as u32;
	let width_px = (area.width.max(1) as u32).saturating_mul(font_w);
	let height_px = (area.height.max(1) as u32).saturating_mul(font_h);
	let base_bg = rgba(MINIMAP_RGBA_BG);
	let marker_gutter = marker_gutter_width(area);
	let content_width = area.width.saturating_sub(marker_gutter).max(1);
	let mut image = RgbaImage::from_pixel(width_px, height_px, base_bg);
	let total_rows = snapshot.rows.len();
	if total_rows == 0 {
		return DynamicImage::ImageRgba8(image);
	}

	let content_width_px = content_width as u32 * font_w;
	let x_pad = (font_w / 10).max(1).min(2);
	let sparse_line_px = if total_rows <= 64 { 2 } else { 1 };
	let use_sparse_preview = (total_rows as u32).saturating_mul(sparse_line_px) <= height_px;
	let used_height_px = if use_sparse_preview {
		(total_rows as u32).saturating_mul(sparse_line_px)
	} else {
		height_px
	};
	if use_sparse_preview {
		let sparse_x_pad = if sparse_line_px <= 1 { 0 } else { x_pad };
		let ink_top_pad = if sparse_line_px > 1 { 1 } else { 0 };
		let ink_bottom_pad = if sparse_line_px > 2 { 1 } else { 0 };
		let max_columns = snapshot.max_columns.max(1) as u32;

		for (row_idx, row) in snapshot.rows.iter().enumerate() {
			if row.logical_width == 0 {
				continue;
			}

			let row_width_px = ((row.logical_width as u32).saturating_mul(content_width_px))
				.div_ceil(max_columns)
				.max(1)
				.min(content_width_px);

			let row_top = row_idx as u32 * sparse_line_px;
			let row_bottom = (row_top + sparse_line_px).min(height_px);
			let ink_top = (row_top + ink_top_pad).min(row_bottom.saturating_sub(1));
			let ink_bottom = row_bottom.saturating_sub(ink_bottom_pad).max(ink_top + 1);
			let core_top = if ink_bottom.saturating_sub(ink_top) > 2 {
				ink_top + 1
			} else {
				ink_top
			};
			let core_bottom = if ink_bottom.saturating_sub(core_top) > 1 {
				ink_bottom - 1
			} else {
				ink_bottom
			};

			for x in 0..row_width_px {
				let within_cell = x % font_w;
				if within_cell < sparse_x_pad || within_cell >= font_w.saturating_sub(sparse_x_pad)
				{
					continue;
				}

				let bin = sample_preview_bin_range(
					row,
					x.saturating_sub(1) as usize,
					(x + 2).min(row_width_px) as usize,
					row_width_px as usize,
				);
				let Some(color) = preview_bin_rgba(bin, theme_colors, base_bg) else {
					continue;
				};
				let edge = blend_rgba(base_bg, color, 3, 8);
				let core = blend_rgba(base_bg, color, 7, 8);
				fill_image_rect(&mut image, x, ink_top, x + 1, ink_bottom, edge);
				fill_image_rect(&mut image, x, core_top, x + 1, core_bottom, core);
			}
		}
	} else {
		let total_columns = content_width_px as usize;
		let dense_x_pad = 0u32;
		for y in 0..used_height_px {
			let row_start = ((y as usize) * total_rows) / used_height_px.max(1) as usize;
			let row_end = ((((y as usize) + 1) * total_rows) + used_height_px as usize - 1)
				/ used_height_px.max(1) as usize;
			let row_end = row_end.max(row_start.saturating_add(1)).min(total_rows);
			let row_slice = &snapshot.rows[row_start.min(total_rows - 1)..row_end];
			for x in 0..content_width_px {
				let within_cell = x % font_w;
				if within_cell < dense_x_pad || within_cell >= font_w.saturating_sub(dense_x_pad) {
					continue;
				}

				let Some((bin, non_empty, sampled_rows)) = dominant_preview_bin_for_rows(
					row_slice,
					x as usize,
					(x as usize).saturating_add(1),
					total_columns,
				) else {
					continue;
				};
				let Some(base_color) = preview_bin_rgba(bin, theme_colors, base_bg) else {
					continue;
				};
				let coverage_weight =
					(2 + ((non_empty as u32 * 6) / sampled_rows.max(1) as u32) as u16).min(8);
				let color = blend_rgba(base_bg, base_color, coverage_weight, 8);
				image.put_pixel(x, y, color);
			}
		}
	}

	render_search_markers_image(
		&mut image,
		content_width_px,
		used_height_px,
		&snapshot.overlay,
	);
	render_preview_overlay_image(
		&mut image,
		content_width_px,
		used_height_px,
		&snapshot.overlay,
	);
	DynamicImage::ImageRgba8(image)
}

pub(super) fn render_minimap_snapshot(
	buf: &mut Buffer,
	area: Rect,
	snapshot: &MinimapSnapshot,
	theme_colors: &[Option<Color>; CATEGORY_COUNT],
) {
	match snapshot {
		MinimapSnapshot::Preview(snapshot) => {
			render_preview_snapshot(buf, area, snapshot, theme_colors)
		}
		MinimapSnapshot::Overview(snapshot) => render_overview_snapshot(
			buf,
			area,
			snapshot,
			Color::Rgb(
				crate::ui::MINIMAP_RGBA_DENSITY[0],
				crate::ui::MINIMAP_RGBA_DENSITY[1],
				crate::ui::MINIMAP_RGBA_DENSITY[2],
			),
		),
		MinimapSnapshot::ByteFallback(snapshot) => render_overview_snapshot(
			buf,
			area,
			snapshot,
			Color::Rgb(
				crate::ui::MINIMAP_RGBA_FALLBACK[0],
				crate::ui::MINIMAP_RGBA_FALLBACK[1],
				crate::ui::MINIMAP_RGBA_FALLBACK[2],
			),
		),
	}
}

fn render_overview_snapshot(
	buf: &mut Buffer,
	area: Rect,
	snapshot: &OverviewSnapshot,
	band_color: Color,
) {
	use crate::ui::*;

	if area.width == 0 || area.height == 0 || snapshot.bands.is_empty() {
		return;
	}

	let base_bg = Color::Rgb(MINIMAP_RGBA_BG[0], MINIMAP_RGBA_BG[1], MINIMAP_RGBA_BG[2]);
	let marker_gutter = marker_gutter_width(area);
	let content_width = area.width.saturating_sub(marker_gutter).max(1);
	fill_minimap_area(buf, area, base_bg);

	for row in 0..area.height {
		let band_idx = ((row as usize) * snapshot.bands.len()) / (area.height.max(1) as usize);
		let density = snapshot.bands[band_idx.min(snapshot.bands.len().saturating_sub(1))];
		let line_width =
			((density as u32).saturating_mul(content_width as u32) / 255).max(1) as u16;
		for col in 0..line_width.min(content_width) {
			set_minimap_cell(buf, area.x + col, area.y + row, band_color);
		}
	}

	render_search_markers(buf, area, area.height, &snapshot.overlay);
	render_overview_overlay(buf, area, content_width, band_color, &snapshot.overlay);
}

fn render_preview_snapshot(
	buf: &mut Buffer,
	area: Rect,
	snapshot: &PreviewSnapshot,
	theme_colors: &[Option<Color>; CATEGORY_COUNT],
) {
	use crate::ui::*;

	if area.width == 0 || area.height == 0 {
		return;
	}

	let base_bg = Color::Rgb(MINIMAP_RGBA_BG[0], MINIMAP_RGBA_BG[1], MINIMAP_RGBA_BG[2]);
	let marker_gutter = marker_gutter_width(area);
	let content_width = area.width.saturating_sub(marker_gutter).max(1);
	fill_minimap_area(buf, area, base_bg);

	let total_rows = snapshot.rows.len();
	if total_rows == 0 {
		render_search_markers(buf, area, area.height, &snapshot.overlay);
		return;
	}

	let content_height = area.height.min(total_rows as u16).max(1);
	for screen_row in 0..content_height {
		let row_idx = if total_rows <= area.height as usize {
			screen_row as usize
		} else {
			((screen_row as usize) * total_rows) / area.height.max(1) as usize
		};
		let row = &snapshot.rows[row_idx.min(total_rows - 1)];
		for col in 0..content_width {
			let left_bin = sample_preview_half_bin(row, col, 0, content_width);
			let right_bin = sample_preview_half_bin(row, col, 1, content_width);
			let cell_bin = sample_preview_bin(row, col, content_width);
			let left_active = left_bin != EMPTY_PREVIEW_BIN;
			let right_active = right_bin != EMPTY_PREVIEW_BIN;
			if let (Some(glyph), Some(color)) = (
				preview_braille_glyph(left_active, right_active),
				preview_bin_color(cell_bin, theme_colors, base_bg),
			) {
				set_minimap_glyph(
					buf,
					area.x + col,
					area.y + screen_row,
					glyph,
					color,
					base_bg,
				);
			}
		}
	}

	render_search_markers(buf, area, content_height, &snapshot.overlay);
	render_preview_overlay(buf, area, content_width, content_height, &snapshot.overlay);
}

#[cfg(test)]
mod tests {
	use super::{PREVIEW_BIN_COLUMNS, render_minimap_snapshot, render_preview_image};
	use crate::core::DocLine;
	use crate::engine::{
		EMPTY_PREVIEW_BIN, MinimapOverlay, MinimapSnapshot, OverviewSnapshot, PreviewRow,
		PreviewSnapshot,
	};
	use crate::svp::highlight::{CATEGORY_COUNT, TokenCategory};
	use ratatui::{buffer::Buffer, layout::Rect, style::Color};

	#[test]
	fn overview_renderer_keeps_uniform_width_for_uniform_bands() {
		let area = Rect::new(0, 0, 14, 20);
		let mut buf = Buffer::empty(area);
		let snapshot = MinimapSnapshot::ByteFallback(OverviewSnapshot {
			overlay: MinimapOverlay {
				total_lines: 1_000,
				viewport_start_line: DocLine::new(500),
				viewport_end_line: DocLine::new(501),
				viewport_line_count: 1,
				cursor_line: DocLine::new(500),
				search_bands: vec![0; 256],
				active_search_band: None,
			},
			bands: vec![127; 256],
		});

		render_minimap_snapshot(&mut buf, area, &snapshot, &[None; CATEGORY_COUNT]);

		let fallback_bg = Color::Rgb(76, 76, 76);
		let expected_width = 6usize;
		for row in 0..area.height {
			if row == 10 {
				continue;
			}
			let width = (0..area.width)
				.take_while(|&col| buf[(col, row)].bg == fallback_bg)
				.count();
			assert_eq!(width, expected_width, "row {row} rendered uneven width");
		}
	}

	#[test]
	fn preview_renderer_top_aligns_short_files() {
		let area = Rect::new(0, 0, 12, 10);
		let mut buf = Buffer::empty(area);
		let rows = vec![
			PreviewRow {
				logical_width: 8,
				bins: {
					let mut bins = vec![EMPTY_PREVIEW_BIN; PREVIEW_BIN_COLUMNS];
					bins[0] = TokenCategory::Keyword as u8;
					bins.into_boxed_slice()
				},
			},
			PreviewRow {
				logical_width: 8,
				bins: {
					let mut bins = vec![EMPTY_PREVIEW_BIN; PREVIEW_BIN_COLUMNS];
					bins[PREVIEW_BIN_COLUMNS / 3] = TokenCategory::Function as u8;
					bins.into_boxed_slice()
				},
			},
		];
		let snapshot = MinimapSnapshot::Preview(PreviewSnapshot {
			overlay: MinimapOverlay {
				total_lines: 2,
				viewport_start_line: DocLine::ZERO,
				viewport_end_line: DocLine::new(2),
				viewport_line_count: 2,
				cursor_line: DocLine::ZERO,
				search_bands: vec![0; 256],
				active_search_band: None,
			},
			max_columns: 8,
			rows,
		});
		let mut theme_colors = [None; CATEGORY_COUNT];
		theme_colors[TokenCategory::Keyword as usize] = Some(Color::Rgb(255, 0, 0));
		theme_colors[TokenCategory::Function as usize] = Some(Color::Rgb(0, 255, 0));

		render_minimap_snapshot(&mut buf, area, &snapshot, &theme_colors);

		assert!((0..area.width).any(|x| buf[(x, 0)].symbol() != " "));
		assert!((0..area.width).any(|x| buf[(x, 1)].symbol() != " "));
		assert_eq!(buf[(0, 5)].symbol(), " ");
	}

	#[test]
	fn preview_image_top_aligns_short_files() {
		let area = Rect::new(0, 0, 12, 10);
		let snapshot = PreviewSnapshot {
			overlay: MinimapOverlay {
				total_lines: 2,
				viewport_start_line: DocLine::ZERO,
				viewport_end_line: DocLine::new(2),
				viewport_line_count: 2,
				cursor_line: DocLine::ZERO,
				search_bands: vec![0; 256],
				active_search_band: None,
			},
			max_columns: 8,
			rows: vec![
				PreviewRow {
					logical_width: 8,
					bins: {
						let mut bins = vec![EMPTY_PREVIEW_BIN; PREVIEW_BIN_COLUMNS];
						bins[0] = TokenCategory::Keyword as u8;
						bins.into_boxed_slice()
					},
				},
				PreviewRow {
					logical_width: 8,
					bins: {
						let mut bins = vec![EMPTY_PREVIEW_BIN; PREVIEW_BIN_COLUMNS];
						bins[PREVIEW_BIN_COLUMNS / 2] = TokenCategory::Function as u8;
						bins.into_boxed_slice()
					},
				},
			],
		};
		let mut theme_colors = [None; CATEGORY_COUNT];
		theme_colors[TokenCategory::Keyword as usize] = Some(Color::Rgb(255, 0, 0));
		theme_colors[TokenCategory::Function as usize] = Some(Color::Rgb(0, 255, 0));

		let image = render_preview_image(&snapshot, area, &theme_colors, (10, 20)).to_rgba8();
		let base_bg = image[(0, image.height() - 1)];
		let top_has_ink = (0..image.width()).any(|x| image[(x, 1)] != base_bg);
		let lower_empty = (0..image.width()).all(|x| image[(x, image.height() - 5)] == base_bg);

		assert!(top_has_ink);
		assert!(lower_empty);
	}

	#[test]
	fn preview_image_dense_files_use_full_vertical_extent() {
		let area = Rect::new(0, 0, 12, 10);
		let mut rows = Vec::new();
		for row_idx in 0..400 {
			rows.push(PreviewRow {
				logical_width: 8,
				bins: {
					let mut bins = vec![EMPTY_PREVIEW_BIN; PREVIEW_BIN_COLUMNS];
					for bin in bins.iter_mut().skip(row_idx % 3).step_by(3) {
						*bin = TokenCategory::Keyword as u8;
					}
					bins.into_boxed_slice()
				},
			});
		}
		let snapshot = PreviewSnapshot {
			overlay: MinimapOverlay {
				total_lines: rows.len() as u32,
				viewport_start_line: DocLine::ZERO,
				viewport_end_line: DocLine::new(20),
				viewport_line_count: 20,
				cursor_line: DocLine::new(10),
				search_bands: vec![0; 256],
				active_search_band: None,
			},
			max_columns: 8,
			rows,
		};
		let mut theme_colors = [None; CATEGORY_COUNT];
		theme_colors[TokenCategory::Keyword as usize] = Some(Color::Rgb(255, 0, 0));

		let image = render_preview_image(&snapshot, area, &theme_colors, (10, 20)).to_rgba8();
		let base_bg = image[(0, image.height() - 1)];
		let near_bottom_has_ink =
			(0..image.width()).any(|x| image[(x, image.height().saturating_sub(8))] != base_bg);

		assert!(near_bottom_has_ink);
	}

	#[test]
	fn preview_image_dense_files_do_not_leave_periodic_empty_vertical_stripes() {
		let area = Rect::new(0, 0, 12, 10);
		let rows = (0..400)
			.map(|_| PreviewRow {
				logical_width: 8,
				bins: vec![TokenCategory::Keyword as u8; PREVIEW_BIN_COLUMNS].into_boxed_slice(),
			})
			.collect();
		let snapshot = PreviewSnapshot {
			overlay: MinimapOverlay {
				total_lines: 400,
				viewport_start_line: DocLine::ZERO,
				viewport_end_line: DocLine::new(20),
				viewport_line_count: 20,
				cursor_line: DocLine::new(10),
				search_bands: vec![0; 256],
				active_search_band: None,
			},
			max_columns: 8,
			rows,
		};
		let mut theme_colors = [None; CATEGORY_COUNT];
		theme_colors[TokenCategory::Keyword as usize] = Some(Color::Rgb(255, 0, 0));

		let image = render_preview_image(&snapshot, area, &theme_colors, (10, 20)).to_rgba8();
		let base_bg = image[(0, image.height() - 1)];
		let content_width_px = (area.width.saturating_sub(1).max(1) as u32) * 10;

		for x in 1..content_width_px.saturating_sub(1) {
			let has_ink = (0..image.height()).any(|y| image[(x, y)] != base_bg);
			assert!(has_ink, "column {x} rendered as an empty vertical stripe");
		}
	}

	#[test]
	fn preview_image_sparse_files_do_not_leave_periodic_empty_vertical_stripes() {
		let area = Rect::new(0, 0, 12, 10);
		let rows = (0..100)
			.map(|_| PreviewRow {
				logical_width: 8,
				bins: vec![TokenCategory::Keyword as u8; PREVIEW_BIN_COLUMNS].into_boxed_slice(),
			})
			.collect();
		let snapshot = PreviewSnapshot {
			overlay: MinimapOverlay {
				total_lines: 100,
				viewport_start_line: DocLine::ZERO,
				viewport_end_line: DocLine::new(20),
				viewport_line_count: 20,
				cursor_line: DocLine::new(10),
				search_bands: vec![0; 256],
				active_search_band: None,
			},
			max_columns: 8,
			rows,
		};
		let mut theme_colors = [None; CATEGORY_COUNT];
		theme_colors[TokenCategory::Keyword as usize] = Some(Color::Rgb(255, 0, 0));

		let image = render_preview_image(&snapshot, area, &theme_colors, (10, 20)).to_rgba8();
		let base_bg = image[(0, image.height() - 1)];
		let content_width_px = (area.width.saturating_sub(1).max(1) as u32) * 10;

		for x in 1..content_width_px.saturating_sub(1) {
			let has_ink = (0..100u32).any(|y| image[(x, y)] != base_bg);
			assert!(
				has_ink,
				"column {x} rendered as an empty sparse vertical stripe"
			);
		}
	}

	#[test]
	fn preview_image_sparse_files_respect_relative_line_widths() {
		let area = Rect::new(0, 0, 12, 10);
		let snapshot = PreviewSnapshot {
			overlay: MinimapOverlay {
				total_lines: 100,
				viewport_start_line: DocLine::new(90),
				viewport_end_line: DocLine::new(95),
				viewport_line_count: 5,
				cursor_line: DocLine::new(92),
				search_bands: vec![0; 256],
				active_search_band: None,
			},
			max_columns: 32,
			rows: vec![
				PreviewRow {
					logical_width: 8,
					bins: vec![TokenCategory::Keyword as u8; PREVIEW_BIN_COLUMNS]
						.into_boxed_slice(),
				},
				PreviewRow {
					logical_width: 32,
					bins: vec![TokenCategory::Keyword as u8; PREVIEW_BIN_COLUMNS]
						.into_boxed_slice(),
				},
			],
		};
		let mut theme_colors = [None; CATEGORY_COUNT];
		theme_colors[TokenCategory::Keyword as usize] = Some(Color::Rgb(255, 0, 0));

		let image = render_preview_image(&snapshot, area, &theme_colors, (10, 20)).to_rgba8();
		let base_bg = image[(0, image.height() - 1)];
		let content_width_px = (area.width.saturating_sub(1).max(1) as u32) * 10;
		let probe_x = content_width_px.saturating_sub(6);
		let short_row_y = 1u32;
		let long_row_y = 3u32;

		assert_eq!(image[(probe_x, short_row_y)], base_bg);
		assert_ne!(image[(probe_x, long_row_y)], base_bg);
	}

	#[test]
	fn preview_image_uses_neon_green_viewport_frame() {
		let area = Rect::new(0, 0, 12, 10);
		let snapshot = PreviewSnapshot {
			overlay: MinimapOverlay {
				total_lines: 20,
				viewport_start_line: DocLine::new(4),
				viewport_end_line: DocLine::new(10),
				viewport_line_count: 6,
				cursor_line: DocLine::new(7),
				search_bands: vec![0; 256],
				active_search_band: None,
			},
			max_columns: 8,
			rows: (0..20)
				.map(|_| PreviewRow {
					logical_width: 8,
					bins: vec![TokenCategory::Keyword as u8; PREVIEW_BIN_COLUMNS]
						.into_boxed_slice(),
				})
				.collect(),
		};
		let mut theme_colors = [None; CATEGORY_COUNT];
		theme_colors[TokenCategory::Keyword as usize] = Some(Color::Rgb(255, 0, 0));

		let image = render_preview_image(&snapshot, area, &theme_colors, (10, 20)).to_rgba8();
		let expected = image::Rgba(crate::ui::MINIMAP_RGBA_VIEWPORT_FRAME);
		let sparse_line_px = (20u32 / 6).clamp(2, 4);
		let used_height_px = snapshot.rows.len() as u32 * sparse_line_px;
		let frame_y = ((snapshot.overlay.viewport_start_line.get() as u64 * used_height_px as u64)
			/ snapshot.overlay.total_lines.max(1) as u64) as u32;

		assert_eq!(
			image[(0, frame_y.min(image.height().saturating_sub(1)))],
			expected
		);
	}

	#[test]
	fn preview_image_uses_neon_green_cursor_line() {
		let area = Rect::new(0, 0, 12, 10);
		let snapshot = PreviewSnapshot {
			overlay: MinimapOverlay {
				total_lines: 20,
				viewport_start_line: DocLine::new(4),
				viewport_end_line: DocLine::new(10),
				viewport_line_count: 6,
				cursor_line: DocLine::new(7),
				search_bands: vec![0; 256],
				active_search_band: None,
			},
			max_columns: 8,
			rows: (0..20)
				.map(|_| PreviewRow {
					logical_width: 8,
					bins: vec![TokenCategory::Keyword as u8; PREVIEW_BIN_COLUMNS]
						.into_boxed_slice(),
				})
				.collect(),
		};
		let mut theme_colors = [None; CATEGORY_COUNT];
		theme_colors[TokenCategory::Keyword as usize] = Some(Color::Rgb(255, 0, 0));

		let image = render_preview_image(&snapshot, area, &theme_colors, (10, 20)).to_rgba8();
		let expected = image::Rgba(crate::ui::MINIMAP_RGBA_CURSOR_LINE);
		let sparse_line_px = if snapshot.rows.len() <= 64 { 2 } else { 1 };
		let used_height_px = snapshot.rows.len() as u32 * sparse_line_px;
		let cursor_top = ((snapshot.overlay.cursor_line.get() as u64) * used_height_px as u64
			/ snapshot.overlay.total_lines.max(1) as u64) as u32;
		let cursor_bottom = (((snapshot.overlay.cursor_line.get() as u64 + 1)
			* used_height_px as u64)
			/ snapshot.overlay.total_lines.max(1) as u64) as u32;
		let horizontal_thickness = if image.height() >= 180 { 2 } else { 1 };
		let cursor_y = cursor_top
			.saturating_add(
				cursor_bottom
					.saturating_sub(cursor_top)
					.saturating_sub(horizontal_thickness)
					/ 2,
			)
			.min(image.height().saturating_sub(horizontal_thickness));

		assert_eq!(
			image[(
				image.width() / 2,
				cursor_y.min(image.height().saturating_sub(1))
			)],
			expected
		);
	}
}
