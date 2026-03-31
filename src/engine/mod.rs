use crate::ecs::{NodeId, UastRegistry};
use crate::uast::{Viewport, UastProjection, UastMutation};
use crate::svp::{SvpResolver, RequestPriority, ingest_svp_file};
use std::sync::Arc;
use std::sync::mpsc;
use std::sync::atomic::Ordering;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorMode {
	Normal,
	Insert,
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
	WriteFile,
	WriteFileAs(String),
	WriteAndQuit,
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
		let mut file_path: Option<String> = None;
		let mut status_message: Option<String> = None;
		let mut pending_quit = false;

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
						let (node, offset, clamped_col) = registry.find_node_at_line_col(
							rid,
							cursor_abs_line,
							cursor_abs_col,
						);
						cursor_node = node;
						cursor_offset = offset;
						cursor_abs_col = clamped_col;
						needs_render = true;
					}
				}
				EditorCommand::GotoLine(target) => {
					if let Some(rid) = root_id {
						let total = registry.get_total_newlines(rid);
						cursor_abs_line = target.min(total);
						cursor_abs_col = 0;
						let (node, offset, clamped_col) = registry.find_node_at_line_col(
							rid,
							cursor_abs_line,
							cursor_abs_col,
						);
						cursor_node = node;
						cursor_offset = offset;
						cursor_abs_col = clamped_col;
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
						if c == '\n' {
							cursor_abs_line += 1;
							cursor_abs_col = 0;
						} else {
							cursor_abs_col += 1;
						}
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
							path.clone(),
						);
						file_path = Some(path);
						root_id = Some(rid);
						cursor_node = registry
							.get_first_child(rid)
							.expect("Failed to load new file root");
						cursor_offset = 0;
						cursor_abs_line = 0;
						cursor_abs_col = 0;
						needs_render = true;
					} else {
						// New file — create empty document
						use crate::uast::kind::SemanticKind;
						use crate::uast::metrics::SpanMetrics;
						let mut chunk = registry.reserve_chunk(2).expect("OOM");
						let rid = chunk.spawn_node(
							SemanticKind::RelationalTable,
							None,
							SpanMetrics { byte_length: 0, newlines: 0 },
						);
						let leaf = chunk.spawn_node(
							SemanticKind::Token,
							None,
							SpanMetrics { byte_length: 0, newlines: 0 },
						);
						chunk.append_local_child(rid, leaf);
						unsafe {
							*registry.virtual_data[leaf.index()].get() = Some(Vec::new());
						}
						file_path = Some(path);
						root_id = Some(rid);
						cursor_node = leaf;
						cursor_offset = 0;
						cursor_abs_line = 0;
						cursor_abs_col = 0;
						needs_render = true;
					}
				}
				EditorCommand::WriteFile => {
					if let Some(rid) = root_id {
						if let Some(ref path) = file_path {
							match registry.collect_document_bytes(rid) {
								Ok(bytes) => {
									match std::fs::write(path, &bytes) {
										Ok(_) => status_message = Some(format!("\"{}\" {}B written", path, bytes.len())),
										Err(e) => status_message = Some(format!("Write error: {}", e)),
									}
								}
								Err(msg) => status_message = Some(msg.to_string()),
							}
						} else {
							status_message = Some("No file name".to_string());
						}
					} else {
						status_message = Some("No file to write".to_string());
					}
					needs_render = true;
				}
				EditorCommand::WriteFileAs(path) => {
					if let Some(rid) = root_id {
						match registry.collect_document_bytes(rid) {
							Ok(bytes) => {
								match std::fs::write(&path, &bytes) {
									Ok(_) => {
										status_message = Some(format!("\"{}\" {}B written", path, bytes.len()));
										file_path = Some(path);
									}
									Err(e) => status_message = Some(format!("Write error: {}", e)),
								}
							}
							Err(msg) => status_message = Some(msg.to_string()),
						}
					} else {
						status_message = Some("No file to write".to_string());
					}
					needs_render = true;
				}
				EditorCommand::WriteAndQuit => {
					if let Some(rid) = root_id {
						if let Some(ref path) = file_path {
							match registry.collect_document_bytes(rid) {
								Ok(bytes) => {
									match std::fs::write(path, &bytes) {
										Ok(_) => {
											status_message = Some(format!("\"{}\" {}B written", path, bytes.len()));
											pending_quit = true;
										}
										Err(e) => status_message = Some(format!("Write error: {}", e)),
									}
								}
								Err(msg) => status_message = Some(msg.to_string()),
							}
						} else {
							status_message = Some("No file name".to_string());
						}
					} else {
						status_message = Some("No file to write".to_string());
					}
					needs_render = true;
				}
				EditorCommand::InternalRefresh => {
					needs_render = true;
				}
				EditorCommand::Quit => break,
			}

			if needs_render {
				let (tokens, total_lines) = if let Some(rid) = root_id {
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

					(tokens, registry.get_total_newlines(rid))
				} else {
					(Vec::new(), 0)
				};

				let _ = tx_view.send(Viewport {
					tokens,
					cursor_abs_pos: (cursor_abs_line, cursor_abs_col),
					total_lines,
					status_message: status_message.take(),
					should_quit: pending_quit,
				});

				if pending_quit {
					break;
				}
			}
		}
	}
}
