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
	Search,
	Confirm,
}

pub enum ConfirmAction {
	Yes,
	No,
	All,
	Quit,
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
	SearchStart(String),
	SearchNext,
	SearchPrev,
	SubstituteAll { pattern: String, replacement: String },
	SubstituteConfirm { pattern: String, replacement: String },
	ConfirmResponse(ConfirmAction),
	InternalRefresh,
	Quit,
}

struct ConfirmState {
	replacement: String,
	replacements_done: u32,
	total_matches: u32,
}

fn advance_col(b: u8, line: &mut u32, col: &mut u32) {
	if b == b'\n' { *line += 1; *col = 0; }
	else if b == b'\t' { *col += 4 - (*col % 4); }
	else { *col += 1; }
}

fn find_all_matches(doc_bytes: &[u8], pattern: &[u8]) -> Vec<(u32, u32)> {
	let mut matches = Vec::new();
	if pattern.is_empty() { return matches; }
	let mut line: u32 = 0;
	let mut col: u32 = 0;
	let mut i = 0;
	while i + pattern.len() <= doc_bytes.len() {
		if &doc_bytes[i..i + pattern.len()] == pattern {
			matches.push((line, col));
			for &b in &doc_bytes[i..i + pattern.len()] {
				advance_col(b, &mut line, &mut col);
			}
			i += pattern.len();
		} else {
			advance_col(doc_bytes[i], &mut line, &mut col);
			i += 1;
		}
	}
	matches
}

fn rebuild_document_from_bytes(registry: &UastRegistry, bytes: &[u8]) -> (NodeId, NodeId) {
	use crate::uast::kind::SemanticKind;
	use crate::uast::metrics::SpanMetrics;
	let newlines = bytes.iter().filter(|&&b| b == b'\n').count() as u32;
	let byte_len = bytes.len() as u32;
	let mut chunk = registry.reserve_chunk(2).expect("OOM");
	let root = chunk.spawn_node(
		SemanticKind::RelationalTable,
		None,
		SpanMetrics { byte_length: byte_len, newlines },
	);
	let leaf = chunk.spawn_node(
		SemanticKind::Token,
		None,
		SpanMetrics { byte_length: byte_len, newlines },
	);
	chunk.append_local_child(root, leaf);
	unsafe { *registry.virtual_data[leaf.index()].get() = Some(bytes.to_vec()); }
	(root, leaf)
}

fn substitute_bytes(doc: &[u8], pattern: &[u8], replacement: &[u8]) -> (Vec<u8>, u32) {
	let mut result = Vec::with_capacity(doc.len());
	let mut count = 0u32;
	let mut i = 0;
	while i < doc.len() {
		if i + pattern.len() <= doc.len() && &doc[i..i + pattern.len()] == pattern {
			result.extend_from_slice(replacement);
			count += 1;
			i += pattern.len();
		} else {
			result.push(doc[i]);
			i += 1;
		}
	}
	(result, count)
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
		let mut file_size: u64 = 0;
		let mut is_dirty = false;
		let mut status_message: Option<String> = None;
		let mut pending_quit = false;
		let mut mode_override: Option<EditorMode> = None;

		// Search state
		let mut search_pattern: Option<String> = None;
		let mut search_matches: Vec<(u32, u32)> = Vec::new(); // (line, col) pairs
		let mut search_match_index: Option<usize> = None;

		// Interactive replace state
		let mut confirm_state: Option<ConfirmState> = None;

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
						is_dirty = true;
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
						is_dirty = true;
						needs_render = true;
					}
				}
				EditorCommand::Scroll(delta) => {
					cursor_abs_line = (cursor_abs_line as i32 + delta).max(0) as u32;
					needs_render = true;
				}
				EditorCommand::LoadFile(path) => {
					if let Ok(metadata) = std::fs::metadata(&path) {
						let fsize = metadata.len();
						let device_id = 0x42;

						let rid = ingest_svp_file(
							&resolver,
							&registry,
							fsize,
							device_id,
							path.clone(),
						);
						file_path = Some(path);
						file_size = fsize;
						is_dirty = false;
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
						file_size = 0;
						is_dirty = false;
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
									let len = bytes.len();
									match std::fs::write(path, &bytes) {
										Ok(_) => {
											file_size = len as u64;
											is_dirty = false;
											status_message = Some(format!("\"{}\" {}B written", path, len));
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
				EditorCommand::WriteFileAs(path) => {
					if let Some(rid) = root_id {
						match registry.collect_document_bytes(rid) {
							Ok(bytes) => {
								let len = bytes.len();
								match std::fs::write(&path, &bytes) {
									Ok(_) => {
										file_size = len as u64;
										is_dirty = false;
										status_message = Some(format!("\"{}\" {}B written", path, len));
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
									let len = bytes.len();
									match std::fs::write(path, &bytes) {
										Ok(_) => {
											file_size = len as u64;
											is_dirty = false;
											status_message = Some(format!("\"{}\" {}B written", path, len));
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
				EditorCommand::SearchStart(pattern) => {
					if let Some(rid) = root_id {
						match registry.collect_document_bytes(rid) {
							Ok(bytes) => {
								let matches = find_all_matches(&bytes, pattern.as_bytes());
								if matches.is_empty() {
									status_message = Some("Pattern not found".to_string());
									search_pattern = Some(pattern);
									search_matches.clear();
									search_match_index = None;
								} else {
									let count = matches.len();
									// Find first match at or after cursor
									let idx = matches.iter()
										.position(|&(l, c)| l > cursor_abs_line || (l == cursor_abs_line && c >= cursor_abs_col))
										.unwrap_or(0);
									search_match_index = Some(idx);
									let (ml, mc) = matches[idx];
									cursor_abs_line = ml;
									let (node, offset, _) = registry.find_node_at_line_col(rid, ml, mc);
									cursor_node = node;
									cursor_offset = offset;
									cursor_abs_col = mc;
									search_pattern = Some(pattern);
									search_matches = matches;
									status_message = Some(format!("[{}/{}]", idx + 1, count));
								}
							}
							Err(msg) => { status_message = Some(msg.to_string()); }
						}
					}
					needs_render = true;
				}
				EditorCommand::SearchNext => {
					if let Some(rid) = root_id {
						if search_matches.is_empty() {
							status_message = Some("No previous search".to_string());
						} else {
							let idx = match search_match_index {
								Some(i) => if i + 1 >= search_matches.len() { 0 } else { i + 1 },
								None => 0,
							};
							let wrapped = search_match_index.is_some_and(|i| i + 1 >= search_matches.len());
							search_match_index = Some(idx);
							let (ml, mc) = search_matches[idx];
							cursor_abs_line = ml;
							let (node, offset, _) = registry.find_node_at_line_col(rid, ml, mc);
							cursor_node = node;
							cursor_offset = offset;
							cursor_abs_col = mc;
							let msg = format!("[{}/{}]", idx + 1, search_matches.len());
							status_message = Some(if wrapped { format!("{} (wrapped)", msg) } else { msg });
						}
					}
					needs_render = true;
				}
				EditorCommand::SearchPrev => {
					if let Some(rid) = root_id {
						if search_matches.is_empty() {
							status_message = Some("No previous search".to_string());
						} else {
							let idx = match search_match_index {
								Some(0) | None => search_matches.len() - 1,
								Some(i) => i - 1,
							};
							let wrapped = search_match_index.is_some_and(|i| i == 0);
							search_match_index = Some(idx);
							let (ml, mc) = search_matches[idx];
							cursor_abs_line = ml;
							let (node, offset, _) = registry.find_node_at_line_col(rid, ml, mc);
							cursor_node = node;
							cursor_offset = offset;
							cursor_abs_col = mc;
							let msg = format!("[{}/{}]", idx + 1, search_matches.len());
							status_message = Some(if wrapped { format!("{} (wrapped)", msg) } else { msg });
						}
					}
					needs_render = true;
				}
				EditorCommand::SubstituteAll { pattern, replacement } => {
					if let Some(rid) = root_id {
						match registry.collect_document_bytes(rid) {
							Ok(bytes) => {
								let (new_bytes, count) = substitute_bytes(&bytes, pattern.as_bytes(), replacement.as_bytes());
								if count == 0 {
									status_message = Some("Pattern not found".to_string());
								} else {
									let (new_root, new_leaf) = rebuild_document_from_bytes(&registry, &new_bytes);
									root_id = Some(new_root);
									cursor_node = new_leaf;
									cursor_offset = 0;
									cursor_abs_line = cursor_abs_line.min(new_bytes.iter().filter(|&&b| b == b'\n').count() as u32);
									cursor_abs_col = 0;
									let (node, offset, clamped_col) = registry.find_node_at_line_col(new_root, cursor_abs_line, 0);
									cursor_node = node;
									cursor_offset = offset;
									cursor_abs_col = clamped_col;
									file_size = new_bytes.len() as u64;
									is_dirty = true;
									search_matches.clear();
									search_match_index = None;
									status_message = Some(format!("{} substitution(s)", count));
								}
							}
							Err(msg) => { status_message = Some(msg.to_string()); }
						}
					}
					needs_render = true;
				}
				EditorCommand::SubstituteConfirm { pattern, replacement } => {
					if let Some(rid) = root_id {
						match registry.collect_document_bytes(rid) {
							Ok(bytes) => {
								let matches = find_all_matches(&bytes, pattern.as_bytes());
								if matches.is_empty() {
									status_message = Some("Pattern not found".to_string());
								} else {
									let total = matches.len() as u32;
									search_pattern = Some(pattern);
									search_matches = matches;
									search_match_index = Some(0);
									confirm_state = Some(ConfirmState {
										replacement,
										replacements_done: 0,
										total_matches: total,
									});
									let (ml, mc) = search_matches[0];
									cursor_abs_line = ml;
									let (node, offset, _) = registry.find_node_at_line_col(rid, ml, mc);
									cursor_node = node;
									cursor_offset = offset;
									cursor_abs_col = mc;
									mode_override = Some(EditorMode::Confirm);
									status_message = Some(format!("Replace? [y/n/a/q] (1/{})", total));
								}
							}
							Err(msg) => { status_message = Some(msg.to_string()); }
						}
					}
					needs_render = true;
				}
				EditorCommand::ConfirmResponse(action) => {
					if let (Some(rid), Some(cs)) = (root_id, &mut confirm_state) {
						let pat = search_pattern.clone().unwrap_or_default();
						match action {
							ConfirmAction::Yes => {
								// Replace current match, rebuild doc, re-scan from next position
								if let Ok(bytes) = registry.collect_document_bytes(rid) {
									let idx = search_match_index.unwrap_or(0);
									let (ml, mc) = search_matches[idx];
									// Find byte offset of this match (mc is visual column)
									let mut byte_off = 0usize;
									let mut line = 0u32;
									let mut col = 0u32;
									for &b in bytes.iter() {
										if line == ml && col == mc { break; }
										advance_col(b, &mut line, &mut col);
										byte_off += 1;
									}
									let rep = cs.replacement.as_bytes();
									let mut new_bytes = Vec::with_capacity(bytes.len());
									new_bytes.extend_from_slice(&bytes[..byte_off]);
									new_bytes.extend_from_slice(rep);
									new_bytes.extend_from_slice(&bytes[byte_off + pat.len()..]);
									cs.replacements_done += 1;

									let (new_root, _) = rebuild_document_from_bytes(&registry, &new_bytes);
									root_id = Some(new_root);
									let new_rid = new_root;

									// Re-scan for remaining matches after replacement point
									let new_matches = find_all_matches(&new_bytes, pat.as_bytes());
									// Find next match at or after the replacement point
									let rep_end_line;
									let rep_end_col;
									{
										let mut l = ml;
										let mut c = mc;
										for &b in rep {
											advance_col(b, &mut l, &mut c);
										}
										rep_end_line = l;
										rep_end_col = c;
									}
									let next_idx = new_matches.iter()
										.position(|&(l, c)| l > rep_end_line || (l == rep_end_line && c >= rep_end_col));

									search_matches = new_matches;
									is_dirty = true;
									file_size = new_bytes.len() as u64;

									if let Some(ni) = next_idx {
										search_match_index = Some(ni);
										let (ml2, mc2) = search_matches[ni];
										cursor_abs_line = ml2;
										let (node, offset, _) = registry.find_node_at_line_col(new_rid, ml2, mc2);
										cursor_node = node;
										cursor_offset = offset;
										cursor_abs_col = mc2;
										status_message = Some(format!("Replace? [y/n/a/q] ({}/{})", cs.replacements_done + 1, cs.total_matches));
									} else {
										status_message = Some(format!("{} substitution(s)", cs.replacements_done));
										mode_override = Some(EditorMode::Normal);
										confirm_state = None;
										let (node, offset, _) = registry.find_node_at_line_col(new_rid, cursor_abs_line, cursor_abs_col);
										cursor_node = node;
										cursor_offset = offset;
									}
								}
							}
							ConfirmAction::No => {
								let idx = search_match_index.unwrap_or(0);
								if idx + 1 < search_matches.len() {
									search_match_index = Some(idx + 1);
									let (ml, mc) = search_matches[idx + 1];
									cursor_abs_line = ml;
									let (node, offset, _) = registry.find_node_at_line_col(rid, ml, mc);
									cursor_node = node;
									cursor_offset = offset;
									cursor_abs_col = mc;
									status_message = Some(format!("Replace? [y/n/a/q] ({}/{})", idx + 2, cs.total_matches));
								} else {
									status_message = Some(format!("{} substitution(s)", cs.replacements_done));
									mode_override = Some(EditorMode::Normal);
									confirm_state = None;
								}
							}
							ConfirmAction::All => {
								if let Ok(bytes) = registry.collect_document_bytes(rid) {
									// Replace all remaining from current position
									let idx = search_match_index.unwrap_or(0);
									let (ml, mc) = search_matches[idx];
									let mut byte_off = 0usize;
									let mut line = 0u32;
									let mut col = 0u32;
									for &b in bytes.iter() {
										if line == ml && col == mc { break; }
										advance_col(b, &mut line, &mut col);
										byte_off += 1;
									}
									let remaining = &bytes[byte_off..];
									let (replaced, count) = substitute_bytes(remaining, pat.as_bytes(), cs.replacement.as_bytes());
									let mut new_bytes = Vec::with_capacity(bytes.len());
									new_bytes.extend_from_slice(&bytes[..byte_off]);
									new_bytes.extend_from_slice(&replaced);
									cs.replacements_done += count;

									let (new_root, _new_leaf) = rebuild_document_from_bytes(&registry, &new_bytes);
									root_id = Some(new_root);
									file_size = new_bytes.len() as u64;
									is_dirty = true;
									let (node, offset, clamped_col) = registry.find_node_at_line_col(new_root, cursor_abs_line, cursor_abs_col);
									cursor_node = node;
									cursor_offset = offset;
									cursor_abs_col = clamped_col;
									status_message = Some(format!("{} substitution(s)", cs.replacements_done));
								}
								mode_override = Some(EditorMode::Normal);
								search_matches.clear();
								search_match_index = None;
								confirm_state = None;
							}
							ConfirmAction::Quit => {
								let done = cs.replacements_done;
								status_message = Some(format!("{} substitution(s)", done));
								mode_override = Some(EditorMode::Normal);
								confirm_state = None;
								search_matches.clear();
								search_match_index = None;
							}
						}
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

				let confirm_prompt = confirm_state.as_ref().map(|_| "Replace? [y/n/a/q]".to_string());
				let _ = tx_view.send(Viewport {
					tokens,
					cursor_abs_pos: (cursor_abs_line, cursor_abs_col),
					total_lines,
					status_message: status_message.take(),
					should_quit: pending_quit,
					file_name: file_path.clone(),
					file_size,
					is_dirty,
					search_pattern: search_pattern.clone(),
					confirm_prompt,
					mode_override: mode_override.take(),
				});

				if pending_quit {
					break;
				}
			}
		}
	}
}
