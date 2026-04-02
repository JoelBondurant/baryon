use crate::uast::{MinimapMode, MinimapSnapshot};
use image::{DynamicImage, Rgba, RgbaImage};
use ratatui::{Frame, layout::Rect};
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

fn render_snapshot_image(snapshot: &MinimapSnapshot, area: Rect) -> DynamicImage {
	let width = (area.width as u32).saturating_mul(8).max(32);
	let height = (snapshot.bands.len() as u32).max((area.height as u32).saturating_mul(8));
	let mut image = RgbaImage::from_pixel(width, height, Rgba([18, 18, 18, 255]));
	let content_left = 0u32;
	let marker_gutter = 2u32;
	let content_right = width.saturating_sub(marker_gutter).max(content_left + 1);
	let content_width = content_right.saturating_sub(content_left).max(1);

	for y in 0..height {
		let band_idx = ((y as usize) * snapshot.bands.len()) / height.max(1) as usize;
		let density = snapshot.bands[band_idx.min(snapshot.bands.len().saturating_sub(1))];
		let line_width = ((density as u32).saturating_mul(content_width) / 255).max(1);
		let band_color = match snapshot.mode {
			MinimapMode::TextDensity => Rgba([110, 110, 110, 255]),
			MinimapMode::ByteFallback => Rgba([76, 76, 76, 255]),
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
			Rgba([224, 224, 224, 255])
		} else {
			Rgba([160, 160, 160, 255])
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
	let viewport_end = viewport_start
		.saturating_add(snapshot.viewport_line_count.max(1))
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
			Rgba([186, 186, 186, 255]),
		);
		image.put_pixel(
			x.min(width.saturating_sub(1)),
			frame_bottom.min(height.saturating_sub(1)),
			Rgba([186, 186, 186, 255]),
		);
	}
	for y in frame_top..=frame_bottom.min(height.saturating_sub(1)) {
		image.put_pixel(content_left, y, Rgba([162, 162, 162, 255]));
		image.put_pixel(
			content_right.min(width.saturating_sub(1)),
			y,
			Rgba([162, 162, 162, 255]),
		);
	}

	let cursor_y = ((snapshot.cursor_line.get().min(total_lines - 1) as u64) * height as u64
		/ total_lines as u64) as u32;
	for x in 0..width {
		image.put_pixel(
			x,
			cursor_y.min(height.saturating_sub(1)),
			Rgba([218, 218, 218, 255]),
		);
	}

	DynamicImage::ImageRgba8(image)
}
