use crate::ecs::{NodeId, UastRegistry};
use crate::uast::{Viewport, UastProjection, UastMutation};
use crate::svp::{SvpResolver, RequestPriority, ingest_svp_file};
use std::sync::Arc;
use std::sync::mpsc;
use std::sync::atomic::Ordering;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorMode {
	Normal,
	Command,
}

pub enum MoveDirection {
	Up,
	Down,
	Left,
	Right,
	Top,
	Bottom,
}

pub enum EditorCommand {
	InsertChar(char),
	Backspace,
	#[allow(dead_code)]
	Scroll(i32),
	MoveCursor(MoveDirection),
	GotoLine(u32),
	LoadFile(String),
	InternalRefresh,
	Quit,
}

pub struct Engine {
	registry: Arc<UastRegistry>,
	resolver: Arc<SvpResolver>,
	rx_cmd: mpsc::Receiver<EditorCommand>,
	tx_view: mpsc::Sender<Viewport>,
}

impl Engine {
	pub fn new(
		registry: Arc<UastRegistry>,
		resolver: Arc<SvpResolver>,
		rx_cmd: mpsc::Receiver<EditorCommand>,
		tx_view: mpsc::Sender<Viewport>,
	) -> Self {
		Self {
			registry,
			resolver,
			rx_cmd,
			tx_view,
		}
	}

	pub fn run(self) {
		let registry = self.registry;
		let resolver = self.resolver;
		let rx_cmd = self.rx_cmd;
		let tx_view = self.tx_view;

		let mut cursor_abs_line: u32 = 0;
		let mut cursor_abs_col: u32 = 0;
		let viewport_lines = 50;
		let mut root_id: Option<NodeId> = None;

		let mut cursor_node = NodeId(std::num::NonZeroU32::new(1).unwrap());
		let mut cursor_offset = 0;

		while let Ok(cmd) = rx_cmd.recv() {
			let mut needs_render = false;

			match cmd {
				EditorCommand::MoveCursor(dir) => {
					if let Some(rid) = root_id {
						match dir {
							MoveDirection::Up => {
								cursor_abs_line = cursor_abs_line.saturating_sub(1)
							}
							MoveDirection::Down => {
								let total = registry.get_total_newlines(rid);
								if cursor_abs_line < total {
									cursor_abs_line += 1;
								}
							}
							MoveDirection::Left => {
								cursor_abs_col = cursor_abs_col.saturating_sub(1)
							}
							MoveDirection::Right => cursor_abs_col += 1,
							MoveDirection::Top => {
								cursor_abs_line = 0;
								cursor_abs_col = 0;
							}
							MoveDirection::Bottom => {
								cursor_abs_line = registry.get_total_newlines(rid);
								cursor_abs_col = 0;
							}
						}
						let (node, offset) = registry.find_node_at_line_col(
							rid,
							cursor_abs_line,
							cursor_abs_col,
						);
						cursor_node = node;
						cursor_offset = offset;
						needs_render = true;
					}
				}
				EditorCommand::GotoLine(target) => {
					if let Some(rid) = root_id {
						let total = registry.get_total_newlines(rid);
						cursor_abs_line = target.min(total);
						cursor_abs_col = 0;
						let (node, offset) = registry.find_node_at_line_col(
							rid,
							cursor_abs_line,
							cursor_abs_col,
						);
						cursor_node = node;
						cursor_offset = offset;
						needs_render = true;
					}
				}
				EditorCommand::InsertChar(c) => {
					if let Some(_rid) = root_id {
						let mut buf = [0; 4];
						let s = c.encode_utf8(&mut buf);
						let (new_node, new_offset) =
							registry.insert_text(cursor_node, cursor_offset, s.as_bytes());
						cursor_node = new_node;
						cursor_offset = new_offset;
						cursor_abs_col += 1;
						needs_render = true;
					}
				}
				EditorCommand::Backspace => {
					if let Some(_rid) = root_id {
						let (new_node, new_offset) =
							registry.delete_backwards(cursor_node, cursor_offset);
						cursor_node = new_node;
						cursor_offset = new_offset;
						cursor_abs_col = cursor_abs_col.saturating_sub(1);
						needs_render = true;
					}
				}
				EditorCommand::Scroll(delta) => {
					cursor_abs_line = (cursor_abs_line as i32 + delta).max(0) as u32;
					needs_render = true;
				}
				EditorCommand::LoadFile(path) => {
					if let Ok(metadata) = std::fs::metadata(&path) {
						let file_size = metadata.len();
						let device_id = 0x42;

						let rid = ingest_svp_file(
							&resolver,
							&registry,
							file_size,
							device_id,
							path,
						);
						root_id = Some(rid);
						cursor_node = registry
							.get_first_child(rid)
							.expect("Failed to load new file root");
						cursor_offset = 0;
						cursor_abs_line = 0;
						cursor_abs_col = 0;
						needs_render = true;
					}
				}
				EditorCommand::InternalRefresh => {
					needs_render = true;
				}
				EditorCommand::Quit => break,
			}

			if needs_render {
				if let Some(rid) = root_id {
					let scroll_y = cursor_abs_line.saturating_sub(20);
					let tokens = registry.query_viewport(rid, scroll_y, viewport_lines);

					for token in &tokens {
						if !token.is_virtual && token.text.is_empty() {
							let idx = token.node_id.index();
							if !registry.dma_in_flight[idx].swap(true, Ordering::Relaxed) {
								if let Some(svp) = unsafe { *registry.spans[idx].get() } {
									resolver.request_dma(
										token.node_id,
										svp,
										RequestPriority::High,
									);
								}
							}
						}
					}

					let _ = tx_view.send(Viewport {
						tokens,
						cursor_abs_pos: (cursor_abs_line, cursor_abs_col),
						total_lines: registry.get_total_newlines(rid),
					});
				}
			}
		}
	}
}
