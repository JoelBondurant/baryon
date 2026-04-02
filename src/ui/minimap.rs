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
	let mut image = RgbaImage::from_pixel(width, height, Rgba([12, 16, 22, 255]));

	for y in 0..height {
		let band_idx = ((y as usize) * snapshot.bands.len()) / height.max(1) as usize;
		let density = snapshot.bands[band_idx.min(snapshot.bands.len().saturating_sub(1))];
		let line_width = ((density as u32).saturating_mul(width.max(1)) / 255).max(1);
		let band_color = match snapshot.mode {
			MinimapMode::TextDensity => Rgba([94, 129, 172, 255]),
			MinimapMode::ByteFallback => Rgba([76, 86, 106, 255]),
		};
		for x in 0..line_width.min(width) {
			image.put_pixel(x, y, band_color);
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
		for x in 0..width {
			let px = image.get_pixel_mut(x, y);
			let [r, g, b, _] = px.0;
			*px = Rgba([
				r.saturating_add(18),
				g.saturating_add(24),
				b.saturating_add(36),
				255,
			]);
		}
	}

	let cursor_y = ((snapshot.cursor_line.get().min(total_lines - 1) as u64) * height as u64
		/ total_lines as u64) as u32;
	for x in 0..width {
		image.put_pixel(
			x,
			cursor_y.min(height.saturating_sub(1)),
			Rgba([240, 143, 104, 255]),
		);
	}

	DynamicImage::ImageRgba8(image)
}
