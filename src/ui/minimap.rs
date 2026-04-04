use crate::uast::{MinimapMode, MinimapSnapshot};
use image::{DynamicImage, Rgba, RgbaImage};
use ratatui::{
	Frame,
	buffer::Buffer,
	layout::Rect,
	style::{Color, Style},
};
use ratatui_image::{Image, Resize, picker::Picker, protocol::Protocol};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

#[derive(Clone, Debug, PartialEq, Eq)]
struct MinimapRequest {
	snapshot: MinimapSnapshot,
	area: Rect,
}

struct MinimapFrame {
	area: Rect,
	protocol: Protocol,
}

pub(super) struct MinimapController {
	tx: Sender<MinimapRequest>,
	rx: Receiver<MinimapFrame>,
	last_requested: Option<MinimapRequest>,
	latest: Option<MinimapFrame>,
}

impl MinimapController {
	pub(super) fn new() -> Self {
		let (tx_req, rx_req) = mpsc::channel();
		let (tx_frame, rx_frame) = mpsc::channel();
		let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
		thread::spawn(move || minimap_worker_loop(picker, rx_req, tx_frame));

		Self {
			tx: tx_req,
			rx: rx_frame,
			last_requested: None,
			latest: None,
		}
	}

	pub(super) fn request(&mut self, snapshot: &MinimapSnapshot, area: Rect) {
		if area.width == 0 || area.height == 0 {
			return;
		}

		let request = MinimapRequest {
			snapshot: snapshot.clone(),
			area,
		};
		if self.last_requested.as_ref() == Some(&request) {
			return;
		}
		self.last_requested = Some(request.clone());
		let _ = self.tx.send(request);
	}

	pub(super) fn poll(&mut self) {
		while let Ok(frame) = self.rx.try_recv() {
			self.latest = Some(frame);
		}
	}

	pub(super) fn render(&self, f: &mut Frame<'_>, area: Rect) {
		if let Some(frame) = &self.latest {
			if frame.area == area {
				f.render_widget(Image::new(&frame.protocol), area);
			}
		}
	}
}

fn minimap_worker_loop(picker: Picker, rx: Receiver<MinimapRequest>, tx: Sender<MinimapFrame>) {
	while let Ok(mut request) = rx.recv() {
		while let Ok(newer) = rx.try_recv() {
			request = newer;
		}

		let image = render_snapshot_image(&request.snapshot, request.area);
		if let Ok(protocol) = picker.new_protocol(image, request.area, Resize::Fit(None)) {
			let _ = tx.send(MinimapFrame {
				area: request.area,
				protocol,
			});
		}
	}
}

fn rgb(color: [u8; 4]) -> Color {
	Color::Rgb(color[0], color[1], color[2])
}

fn set_minimap_cell(buf: &mut Buffer, x: u16, y: u16, color: Color) {
	if let Some(cell) = buf.cell_mut((x, y)) {
		cell.set_char(' ')
			.set_style(Style::default().bg(color).fg(color));
	}
}

pub(super) fn render_byte_fallback_snapshot(
	buf: &mut Buffer,
	area: Rect,
	snapshot: &MinimapSnapshot,
) {
	use crate::ui::*;

	if area.width == 0 || area.height == 0 || snapshot.bands.is_empty() {
		return;
	}

	let marker_gutter = if area.width > 2 { 1 } else { 0 };
	let content_width = area.width.saturating_sub(marker_gutter).max(1);
	let base_bg = rgb(MINIMAP_RGBA_BG);
	let fallback_bg = rgb(MINIMAP_RGBA_FALLBACK);
	let viewport_bg = Color::Rgb(30, 30, 30);
	let viewport_fallback_bg = Color::Rgb(88, 88, 88);
	let frame_bg = rgb(MINIMAP_RGBA_FRAME);
	let frame_side_bg = rgb(MINIMAP_RGBA_FRAME_SIDE);
	let cursor_bg = rgb(MINIMAP_RGBA_CURSOR);
	let search_active_bg = rgb(MINIMAP_RGBA_SEARCH_ACTIVE);
	let search_inactive_bg = rgb(MINIMAP_RGBA_SEARCH_INACTIVE);

	for row in 0..area.height {
		let band_idx = ((row as usize) * snapshot.bands.len()) / (area.height.max(1) as usize);
		let density = snapshot.bands[band_idx.min(snapshot.bands.len().saturating_sub(1))];
		let line_width =
			((density as u32).saturating_mul(content_width as u32) / 255).max(1) as u16;

		for col in 0..area.width {
			set_minimap_cell(buf, area.x + col, area.y + row, base_bg);
		}
		for col in 0..line_width.min(content_width) {
			set_minimap_cell(buf, area.x + col, area.y + row, fallback_bg);
		}
	}

	for (band_idx, intensity) in snapshot.search_bands.iter().copied().enumerate() {
		if intensity == 0 || marker_gutter == 0 {
			continue;
		}
		let y0 =
			((band_idx as u32) * area.height as u32) / snapshot.search_bands.len().max(1) as u32;
		let y1 = (((band_idx as u32) + 1) * area.height as u32)
			/ snapshot.search_bands.len().max(1) as u32;
		let color = if snapshot.active_search_band == Some(band_idx) {
			search_active_bg
		} else {
			search_inactive_bg
		};
		let marker_width = if snapshot.active_search_band == Some(band_idx) && marker_gutter > 1 {
			2
		} else {
			1
		}
		.min(area.width);
		let marker_x = area.x + area.width.saturating_sub(marker_width);
		for row in
			y0.min(area.height.saturating_sub(1) as u32)..y1.max(y0 + 1).min(area.height as u32)
		{
			for x in 0..marker_width {
				set_minimap_cell(buf, marker_x + x, area.y + row as u16, color);
			}
		}
	}

	let total_lines = snapshot.total_lines.max(1);
	let viewport_start = snapshot.viewport_start_line.get().min(total_lines - 1);
	let viewport_end = snapshot
		.viewport_end_line
		.get()
		.max(viewport_start.saturating_add(1))
		.min(total_lines);
	let viewport_top = ((viewport_start as u64) * area.height as u64 / total_lines as u64) as u16;
	let viewport_bottom = ((viewport_end as u64) * area.height as u64 / total_lines as u64) as u16;
	let viewport_bottom = viewport_bottom.max(viewport_top + 1).min(area.height);
	let frame_right = area.x + content_width.saturating_sub(1);

	for row in viewport_top.min(area.height.saturating_sub(1))..viewport_bottom {
		for col in 0..content_width {
			let x = area.x + col;
			let is_fallback = buf[(x, area.y + row)].bg == fallback_bg;
			let color = if is_fallback {
				viewport_fallback_bg
			} else {
				viewport_bg
			};
			set_minimap_cell(buf, x, area.y + row, color);
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

	let cursor_row = ((snapshot.cursor_line.get().min(total_lines - 1) as u64) * area.height as u64
		/ total_lines as u64) as u16;
	for col in 0..area.width {
		set_minimap_cell(
			buf,
			area.x + col,
			area.y + cursor_row.min(area.height - 1),
			cursor_bg,
		);
	}
}

fn render_snapshot_image(snapshot: &MinimapSnapshot, area: Rect) -> DynamicImage {
	use crate::ui::*;
	let width = (area.width as u32).saturating_mul(8).max(32);
	let height = (snapshot.bands.len() as u32).max((area.height as u32).saturating_mul(8));
	let mut image = RgbaImage::from_pixel(width, height, Rgba(MINIMAP_RGBA_BG));
	let content_left = 0u32;
	let marker_gutter = 2u32;
	let content_right = width.saturating_sub(marker_gutter).max(content_left + 1);
	let content_width = content_right.saturating_sub(content_left).max(1);

	for y in 0..height {
		let band_idx = ((y as usize) * snapshot.bands.len()) / height.max(1) as usize;
		let density = snapshot.bands[band_idx.min(snapshot.bands.len().saturating_sub(1))];
		let line_width = ((density as u32).saturating_mul(content_width) / 255).max(1);
		let band_color = match snapshot.mode {
			MinimapMode::TextDensity => Rgba(MINIMAP_RGBA_DENSITY),
			MinimapMode::ByteFallback => Rgba(MINIMAP_RGBA_FALLBACK),
		};
		for x in content_left..(content_left + line_width.min(content_width)) {
			image.put_pixel(x, y, band_color);
		}
	}

	for (band_idx, intensity) in snapshot.search_bands.iter().copied().enumerate() {
		if intensity == 0 {
			continue;
		}
		let y0 = ((band_idx as u32) * height) / snapshot.search_bands.len().max(1) as u32;
		let y1 = (((band_idx as u32) + 1) * height) / snapshot.search_bands.len().max(1) as u32;
		let marker_width = if snapshot.active_search_band == Some(band_idx) {
			2
		} else {
			1
		};
		let color = if snapshot.active_search_band == Some(band_idx) {
			Rgba(MINIMAP_RGBA_SEARCH_ACTIVE)
		} else {
			Rgba(MINIMAP_RGBA_SEARCH_INACTIVE)
		};
		let x_start = width.saturating_sub(marker_width);
		for y in y0.min(height.saturating_sub(1))..y1.max(y0 + 1).min(height) {
			for x in x_start..width {
				image.put_pixel(x, y, color);
			}
		}
	}

	let total_lines = snapshot.total_lines.max(1);
	let viewport_start = snapshot.viewport_start_line.get().min(total_lines - 1);
	let viewport_end = snapshot
		.viewport_end_line
		.get()
		.max(viewport_start.saturating_add(1))
		.min(total_lines);
	let viewport_top = ((viewport_start as u64) * height as u64 / total_lines as u64) as u32;
	let viewport_bottom = ((viewport_end as u64) * height as u64 / total_lines as u64) as u32;
	for y in viewport_top.min(height.saturating_sub(1))
		..viewport_bottom.max(viewport_top + 1).min(height)
	{
		for x in content_left..content_right {
			let px = image.get_pixel_mut(x, y);
			let [r, g, b, _] = px.0;
			*px = Rgba([
				r.saturating_add(12),
				g.saturating_add(12),
				b.saturating_add(12),
				255,
			]);
		}
	}
	let frame_top = viewport_top.min(height.saturating_sub(1));
	let frame_bottom = viewport_bottom
		.max(viewport_top + 1)
		.min(height)
		.saturating_sub(1);
	for x in content_left..=content_right {
		image.put_pixel(
			x.min(width.saturating_sub(1)),
			frame_top,
			Rgba(MINIMAP_RGBA_FRAME),
		);
		image.put_pixel(
			x.min(width.saturating_sub(1)),
			frame_bottom.min(height.saturating_sub(1)),
			Rgba(MINIMAP_RGBA_FRAME),
		);
	}
	for y in frame_top..=frame_bottom.min(height.saturating_sub(1)) {
		image.put_pixel(content_left, y, Rgba(MINIMAP_RGBA_FRAME_SIDE));
		image.put_pixel(
			content_right.min(width.saturating_sub(1)),
			y,
			Rgba(MINIMAP_RGBA_FRAME_SIDE),
		);
	}

	let cursor_y = ((snapshot.cursor_line.get().min(total_lines - 1) as u64) * height as u64
		/ total_lines as u64) as u32;
	for x in 0..width {
		image.put_pixel(
			x,
			cursor_y.min(height.saturating_sub(1)),
			Rgba(MINIMAP_RGBA_CURSOR),
		);
	}

	DynamicImage::ImageRgba8(image)
}

#[cfg(test)]
mod tests {
	use super::render_byte_fallback_snapshot;
	use crate::core::DocLine;
	use crate::uast::{MinimapMode, MinimapSnapshot};
	use ratatui::{buffer::Buffer, layout::Rect, style::Color};

	#[test]
	fn byte_fallback_cell_renderer_keeps_uniform_width_for_uniform_bands() {
		let area = Rect::new(0, 0, 14, 20);
		let mut buf = Buffer::empty(area);
		let snapshot = MinimapSnapshot {
			mode: MinimapMode::ByteFallback,
			bands: vec![127; 256],
			search_bands: vec![0; 256],
			active_search_band: None,
			total_lines: 1_000,
			viewport_start_line: DocLine::new(500),
			viewport_end_line: DocLine::new(501),
			viewport_line_count: 1,
			cursor_line: DocLine::new(500),
		};

		render_byte_fallback_snapshot(&mut buf, area, &snapshot);

		let fallback_bg = Color::Rgb(76, 76, 76);
		let expected_width = 6usize;
		for row in 0..area.height {
			if row == 10 {
				continue;
			}
			let width = (0..area.width)
				.take_while(|&col| buf[(col, row)].bg == fallback_bg)
				.count();
			assert_eq!(
				width, expected_width,
				"row {row} rendered uneven fallback width"
			);
		}
	}
}
