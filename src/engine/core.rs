use crate::core::{CursorPosition, DocByte, DocLine, TAB_SIZE, VisualCol};
use crate::ecs::{NodeId, UastRegistry};
use crate::engine::clipboard::ClipboardHandle;
use crate::engine::undo::{
	TextDelta, UndoLedger, byte_offset_from_line_col, line_col_from_byte_offset,
};
use crate::svp::highlight::HighlightSpan;
use crate::svp::semantic::{SemanticReactor, SemanticRequest};
use crate::svp::{RequestPriority, SvpResolver, ingest_svp_file};
use crate::uast::{NodeCursorTarget, UastMutation, UastProjection, Viewport};
use regex_automata::meta::Regex;
use regex_automata::util::syntax;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc;

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

#[derive(Debug, Clone)]
pub enum SubstituteRange {
	WholeFile,
	CurrentLine,
	SingleLine(u32),
	LineRange(u32, u32),
}

#[derive(Debug, Clone, Default)]
pub struct SubstituteFlags {
	pub global: bool,
	pub confirm: bool,
	pub case_insensitive: bool,
}

pub enum EditorCommand {
	InsertChar(char),
	Backspace,
	Scroll(i32),
	MoveCursor(MoveDirection),
	ClickCursor(u32, u32),
	GotoLine(u32),
	LoadFile(String),
	WriteFile,
	WriteFileAs(String),
	WriteAndQuit,
	SearchStart(String),
	SearchNext,
	SearchPrev,
	SubstituteAll {
		pattern: String,
		replacement: String,
		range: SubstituteRange,
		flags: SubstituteFlags,
	},
	SubstituteConfirm {
		pattern: String,
		replacement: String,
		range: SubstituteRange,
		flags: SubstituteFlags,
	},
	ConfirmResponse(ConfirmAction),
	YankLine {
		register: char,
	},
	Put {
		register: char,
	},
	Undo,
	Redo,
	ClearFlash,
	InternalRefresh,
	Quit,
}

struct ConfirmState {
	replacement: String,
	replacements_done: u32,
	total_matches: u32,
	flags: SubstituteFlags,
	range: SubstituteRange,
}

fn advance_col(b: u8, line: &mut DocLine, col: &mut VisualCol) {
	if b == b'\n' {
		*line += 1;
		*col = VisualCol::ZERO;
	} else if b == b'\t' {
		*col += TAB_SIZE - (col.get() % TAB_SIZE);
	} else {
		*col += 1;
	}
}

fn line_byte_range(doc: &[u8], start_line: u32, end_line: u32) -> (usize, usize) {
	let mut current_line = 0u32;
	let mut byte_start = 0usize;
	let mut found_start = start_line == 0;

	for (i, &b) in doc.iter().enumerate() {
		if b == b'\n' {
			current_line += 1;
			if !found_start && current_line == start_line {
				byte_start = i + 1;
				found_start = true;
			}
			if current_line > end_line {
				return (byte_start, i + 1);
			}
		}
	}
	if !found_start {
		byte_start = doc.len();
	}
	(byte_start, doc.len())
}

fn resolve_byte_range(
	range: &SubstituteRange,
	doc: &[u8],
	cursor_line: DocLine,
) -> Option<(usize, usize)> {
	match range {
		SubstituteRange::WholeFile => None,
		SubstituteRange::CurrentLine => {
			Some(line_byte_range(doc, cursor_line.get(), cursor_line.get()))
		}
		SubstituteRange::SingleLine(n) => Some(line_byte_range(doc, *n, *n)),
		SubstituteRange::LineRange(a, b) => Some(line_byte_range(doc, *a, *b)),
	}
}

fn build_regex(pattern: &str, case_insensitive: bool) -> Result<Regex, String> {
	Regex::builder()
		.syntax(syntax::Config::new().case_insensitive(case_insensitive))
		.build(pattern)
		.map_err(|e| format!("Invalid regex: {}", e))
}

/// Returns Vec of (line, col, match_byte_len) for each match.
fn find_all_matches(doc_bytes: &[u8], re: &Regex) -> Vec<(DocLine, VisualCol, usize)> {
	let mut matches = Vec::new();
	let mut line = DocLine::ZERO;
	let mut col = VisualCol::ZERO;
	let mut prev_pos = 0usize;

	for m in re.find_iter(doc_bytes) {
		for &b in &doc_bytes[prev_pos..m.start()] {
			advance_col(b, &mut line, &mut col);
		}
		let match_len = m.end() - m.start();
		matches.push((line, col, match_len));
		for &b in &doc_bytes[m.start()..m.end()] {
			advance_col(b, &mut line, &mut col);
		}
		prev_pos = m.end();
	}
	matches
}

fn substitute_bytes(
	doc: &[u8],
	re: &Regex,
	replacement: &[u8],
	byte_range: Option<(usize, usize)>,
) -> (Vec<u8>, u32) {
	let (start, end) = byte_range.unwrap_or((0, doc.len()));
	let mut result = Vec::with_capacity(doc.len());
	let mut count = 0u32;

	result.extend_from_slice(&doc[..start]);

	let region = &doc[start..end];
	let mut last = 0usize;
	for m in re.find_iter(region) {
		result.extend_from_slice(&region[last..m.start()]);
		result.extend_from_slice(replacement);
		count += 1;
		last = m.end();
	}
	result.extend_from_slice(&region[last..]);

	result.extend_from_slice(&doc[end..]);
	(result, count)
}

fn create_document_from_bytes(registry: &UastRegistry, bytes: &[u8]) -> (NodeId, NodeId) {
	use crate::uast::kind::SemanticKind;
	use crate::uast::metrics::SpanMetrics;
	let newlines = bytes.iter().filter(|&&b| b == b'\n').count() as u32;
	let byte_len = bytes.len() as u32;
	let mut chunk = registry.reserve_chunk(2).expect("OOM");
	let root = chunk.spawn_node(
		SemanticKind::RelationalTable,
		None,
		SpanMetrics {
			byte_length: byte_len,
			newlines,
		},
	);
	let leaf = chunk.spawn_node(
		SemanticKind::Token,
		None,
		SpanMetrics {
			byte_length: byte_len,
			newlines,
		},
	);
	chunk.append_local_child(root, leaf);
	unsafe {
		*registry.virtual_data[leaf.index()].get() = Some(bytes.to_vec());
	}
	(root, leaf)
}

fn common_char_prefix_len(left: &str, right: &str) -> usize {
	let mut prefix = 0usize;
	let mut left_chars = left.chars();
	let mut right_chars = right.chars();

	loop {
		match (left_chars.next(), right_chars.next()) {
			(Some(l), Some(r)) if l == r => prefix += l.len_utf8(),
			_ => break,
		}
	}

	prefix
}

fn common_char_suffix_len(left: &str, right: &str) -> usize {
	let mut suffix = 0usize;
	let mut left_chars = left.chars().rev();
	let mut right_chars = right.chars().rev();

	loop {
		match (left_chars.next(), right_chars.next()) {
			(Some(l), Some(r)) if l == r => suffix += l.len_utf8(),
			_ => break,
		}
	}

	suffix
}

fn document_rewrite_delta(before: &str, after: &str) -> Option<(u64, String, String)> {
	if before == after {
		return None;
	}

	let prefix = common_char_prefix_len(before, after);
	let suffix = common_char_suffix_len(&before[prefix..], &after[prefix..]);
	let delete_end = before.len().saturating_sub(suffix);
	let insert_end = after.len().saturating_sub(suffix);

	Some((
		prefix as u64,
		before[prefix..delete_end].to_string(),
		after[prefix..insert_end].to_string(),
	))
}

fn shift_byte_offset(byte_offset: DocByte, shift: i64) -> DocByte {
	if shift >= 0 {
		byte_offset.saturating_add(shift as u64)
	} else {
		byte_offset.saturating_sub((-shift) as u64)
	}
}

fn rebase_semantic_highlights_after_delta(
	semantic_highlights: &mut Vec<HighlightSpan>,
	delta: &TextDelta,
) {
	if semantic_highlights.is_empty() {
		return;
	}

	let edit_start = delta.global_byte_offset;
	let edit_end_before = edit_start.saturating_add(delta.deleted_text.len() as u64);
	let shift = delta.inserted_text.len() as i64 - delta.deleted_text.len() as i64;

	semantic_highlights.retain_mut(|span| {
		if span.end <= edit_start {
			return true;
		}

		if span.start >= edit_end_before {
			span.start = shift_byte_offset(span.start, shift);
			span.end = shift_byte_offset(span.end, shift);
			return true;
		}

		false
	});
}

fn push_delta_and_rebase(
	ledger: &mut UndoLedger,
	semantic_highlights: &mut Vec<HighlightSpan>,
	delta: TextDelta,
) {
	rebase_semantic_highlights_after_delta(semantic_highlights, &delta);
	ledger.push(delta);
}

fn invert_text_delta(delta: &TextDelta) -> TextDelta {
	TextDelta {
		global_byte_offset: delta.global_byte_offset,
		deleted_text: delta.inserted_text.clone(),
		inserted_text: delta.deleted_text.clone(),
		state_before: 0,
		state_after: 0,
	}
}

fn push_document_rewrite_delta(
	ledger: &mut UndoLedger,
	semantic_highlights: &mut Vec<HighlightSpan>,
	before: &[u8],
	after: &[u8],
) {
	if before == after {
		return;
	}

	if let (Ok(before_text), Ok(after_text)) =
		(std::str::from_utf8(before), std::str::from_utf8(after))
	{
		if let Some((global_byte_offset, deleted_text, inserted_text)) =
			document_rewrite_delta(before_text, after_text)
		{
			push_delta_and_rebase(
				ledger,
				semantic_highlights,
				TextDelta {
					global_byte_offset: DocByte::new(global_byte_offset),
					deleted_text,
					inserted_text,
					state_before: 0,
					state_after: 0,
				},
			);
		}
		return;
	}

	push_delta_and_rebase(
		ledger,
		semantic_highlights,
		TextDelta {
			global_byte_offset: DocByte::ZERO,
			deleted_text: String::from_utf8_lossy(before).into_owned(),
			inserted_text: String::from_utf8_lossy(after).into_owned(),
			state_before: 0,
			state_after: 0,
		},
	);
}

fn linewise_put_insertion(
	doc: &[u8],
	cursor_line: DocLine,
	text: &str,
) -> (usize, String, DocLine) {
	let global_offset = byte_offset_from_line_col(doc, cursor_line, VisualCol::ZERO);
	let off = global_offset.get() as usize;
	let line_end = doc[off..]
		.iter()
		.position(|&b| b == b'\n')
		.map(|p| off + p + 1)
		.unwrap_or(doc.len());

	let needs_line_break = line_end == doc.len() && !doc.is_empty() && !doc.ends_with(b"\n");
	let mut inserted_text = String::with_capacity(text.len() + usize::from(needs_line_break));
	if needs_line_break {
		inserted_text.push('\n');
	}
	inserted_text.push_str(text);

	let target_cursor_line = if doc.is_empty() {
		DocLine::ZERO
	} else {
		cursor_line + 1
	};

	(line_end, inserted_text, target_cursor_line)
}

fn apply_cursor_target(
	target: NodeCursorTarget,
	cursor_node: &mut NodeId,
	cursor_offset: &mut u32,
	cursor_abs_col: &mut VisualCol,
) {
	*cursor_node = target.node_id;
	*cursor_offset = target.node_byte.get();
	*cursor_abs_col = target.visual_col;
}

pub struct Engine {
	registry: Arc<UastRegistry>,
	resolver: Arc<SvpResolver>,
	rx_cmd: mpsc::Receiver<EditorCommand>,
	tx_cmd: mpsc::Sender<EditorCommand>,
	tx_view: mpsc::Sender<Viewport>,
	reactor: SemanticReactor,
}

impl Engine {
	pub fn new(
		registry: Arc<UastRegistry>,
		resolver: Arc<SvpResolver>,
		rx_cmd: mpsc::Receiver<EditorCommand>,
		tx_cmd: mpsc::Sender<EditorCommand>,
		tx_view: mpsc::Sender<Viewport>,
	) -> Self {
		let reactor = SemanticReactor::new(tx_cmd.clone());
		Self {
			registry,
			resolver,
			rx_cmd,
			tx_cmd,
			tx_view,
			reactor,
		}
	}

	pub fn run(self) {
		let registry = self.registry;
		let resolver = self.resolver;
		let rx_cmd = self.rx_cmd;
		let tx_cmd = self.tx_cmd;
		let tx_view = self.tx_view;
		let reactor = self.reactor;
		let mut semantic_highlights: Vec<HighlightSpan> = Vec::new();
		let mut last_semantic_state: u64 = u64::MAX;
		let mut last_semantic_len: usize = usize::MAX;
		let mut last_semantic_path: Option<String> = None;
		let mut next_semantic_request_id: u64 = 1;
		let mut pending_semantic_request_id: Option<u64> = None;

		let mut cursor_abs_line = DocLine::ZERO;
		let mut cursor_abs_col = VisualCol::ZERO;
		let viewport_lines = 50;
		let mut root_id: Option<NodeId> = None;
		let mut file_path: Option<String> = None;

		let mut status_message: Option<String> = None;
		let mut pending_quit = false;
		let mut mode_override: Option<EditorMode> = None;

		// Search state
		let mut search_pattern: Option<String> = None;
		let mut search_case_insensitive = false;
		let mut search_matches: Vec<(DocLine, VisualCol, usize)> = Vec::new();
		let mut search_match_index: Option<usize> = None;
		let mut search_match_info: Option<String> = None;

		// Interactive replace state
		let mut confirm_state: Option<ConfirmState> = None;

		// Undo/Redo ledger
		let mut ledger = UndoLedger::new();

		// Registers ('"' = unnamed default, '+' = system clipboard)
		let mut registers: HashMap<char, String> = HashMap::new();

		// OS clipboard handle (lazy, survives display server failures)
		let mut clipboard = ClipboardHandle::new();

		// Yank flash: absolute byte range that should be gold-highlighted
		let mut yank_flash: Option<(DocByte, DocByte)> = None;

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
								if cursor_abs_line.get() < total {
									cursor_abs_line += 1;
								}
							}
							MoveDirection::Left => {
								cursor_abs_col = cursor_abs_col.saturating_sub(1)
							}
							MoveDirection::Right => cursor_abs_col += 1,
							MoveDirection::Top => {
								cursor_abs_line = DocLine::ZERO;
								cursor_abs_col = VisualCol::ZERO;
							}
							MoveDirection::Bottom => {
								cursor_abs_line = DocLine::new(registry.get_total_newlines(rid));
								cursor_abs_col = VisualCol::ZERO;
							}
						}
						let target =
							registry.find_node_at_line_col(rid, cursor_abs_line, cursor_abs_col);
						apply_cursor_target(
							target,
							&mut cursor_node,
							&mut cursor_offset,
							&mut cursor_abs_col,
						);
						needs_render = true;
					}
				}
				EditorCommand::GotoLine(target) => {
					if let Some(rid) = root_id {
						let total = registry.get_total_newlines(rid);
						cursor_abs_line = DocLine::new(target.min(total));
						cursor_abs_col = VisualCol::ZERO;
						let target =
							registry.find_node_at_line_col(rid, cursor_abs_line, cursor_abs_col);
						apply_cursor_target(
							target,
							&mut cursor_node,
							&mut cursor_offset,
							&mut cursor_abs_col,
						);
						needs_render = true;
					}
				}
				EditorCommand::InsertChar(c) => {
					if let Some(rid) = root_id {
						// Compute global byte offset at cursor for the delta.
						let global_offset = if let Ok(bytes) = registry.collect_document_bytes(rid)
						{
							byte_offset_from_line_col(&bytes, cursor_abs_line, cursor_abs_col)
						} else {
							DocByte::ZERO
						};

						let mut buf = [0; 4];
						let s = c.encode_utf8(&mut buf);
						let (new_node, new_offset) =
							registry.insert_text(cursor_node, cursor_offset, s.as_bytes());
						cursor_node = new_node;
						cursor_offset = new_offset;

						push_delta_and_rebase(
							&mut ledger,
							&mut semantic_highlights,
							TextDelta {
								global_byte_offset: global_offset,
								deleted_text: String::new(),
								inserted_text: s.to_string(),
								state_before: 0,
								state_after: 0,
							},
						);
						if c == '\n' {
							cursor_abs_line += 1;
							cursor_abs_col = VisualCol::ZERO;
						} else {
							cursor_abs_col += 1;
						}
						needs_render = true;
					}
				}
				EditorCommand::Backspace => {
					if let Some(rid) = root_id {
						// Compute global byte offset and peek at the char to be deleted.
						let global_offset = if let Ok(bytes) = registry.collect_document_bytes(rid)
						{
							byte_offset_from_line_col(&bytes, cursor_abs_line, cursor_abs_col)
						} else {
							DocByte::ZERO
						};

						if global_offset > DocByte::ZERO {
							// Extract the character about to be deleted.
							let (deleted_text, delete_start) =
								if let Ok(bytes) = registry.collect_document_bytes(rid) {
									let off = global_offset.get() as usize;
									let mut start = off - 1;
									while start > 0 && (bytes[start] & 0xC0) == 0x80 {
										start -= 1;
									}
									(
										String::from_utf8_lossy(&bytes[start..off]).into_owned(),
										DocByte::new(start as u64),
									)
								} else {
									(String::new(), global_offset.saturating_sub(1))
								};

							let (new_node, new_offset) =
								registry.delete_backwards(cursor_node, cursor_offset);
							cursor_node = new_node;
							cursor_offset = new_offset;

							if !deleted_text.is_empty() {
								push_delta_and_rebase(
									&mut ledger,
									&mut semantic_highlights,
									TextDelta {
										global_byte_offset: delete_start,
										deleted_text: deleted_text.clone(),
										inserted_text: String::new(),
										state_before: 0,
										state_after: 0,
									},
								);
							}

							// Update cursor position after backspace.
							if deleted_text == "\n" {
								if cursor_abs_line > DocLine::ZERO {
									cursor_abs_line -= 1;
									// Recompute col: walk to the new byte offset.
									if let Ok(bytes) =
										registry.collect_document_bytes(root_id.unwrap_or(rid))
									{
										let (_, col) =
											line_col_from_byte_offset(&bytes, delete_start);
										cursor_abs_col = col;
									}
								}
							} else {
								cursor_abs_col = cursor_abs_col.saturating_sub(1);
							}
							needs_render = true;
						}
					}
				}
				EditorCommand::Scroll(delta) => {
					if let Some(rid) = root_id {
						let total = registry.get_total_newlines(rid);
						let new_line = (cursor_abs_line.get() as i64 + delta as i64)
							.max(0)
							.min(total as i64) as u32;
						cursor_abs_line = DocLine::new(new_line);
						let target =
							registry.find_node_at_line_col(rid, cursor_abs_line, cursor_abs_col);
						apply_cursor_target(
							target,
							&mut cursor_node,
							&mut cursor_offset,
							&mut cursor_abs_col,
						);
					}
					needs_render = true;
				}
				EditorCommand::ClickCursor(line, col) => {
					if let Some(rid) = root_id {
						let total = registry.get_total_newlines(rid);
						cursor_abs_line = DocLine::new(line.min(total));
						cursor_abs_col = VisualCol::new(col);
						let target =
							registry.find_node_at_line_col(rid, cursor_abs_line, cursor_abs_col);
						apply_cursor_target(
							target,
							&mut cursor_node,
							&mut cursor_offset,
							&mut cursor_abs_col,
						);
					}
					needs_render = true;
				}
				EditorCommand::LoadFile(path) => {
					if let Ok(metadata) = std::fs::metadata(&path) {
						let fsize = metadata.len();
						let device_id = 0x42;

						let rid =
							ingest_svp_file(&resolver, &registry, fsize, device_id, path.clone());
						file_path = Some(path);

						root_id = Some(rid);
						cursor_node = registry
							.get_first_child(rid)
							.expect("Failed to load new file root");
						cursor_offset = 0;
						cursor_abs_line = DocLine::ZERO;
						cursor_abs_col = VisualCol::ZERO;
						ledger.clear();
						semantic_highlights.clear();
						last_semantic_state = u64::MAX;
						last_semantic_len = usize::MAX;
						last_semantic_path = None;
						pending_semantic_request_id = None;
						needs_render = true;
					} else {
						// New file — create empty document
						use crate::uast::kind::SemanticKind;
						use crate::uast::metrics::SpanMetrics;
						let mut chunk = registry.reserve_chunk(2).expect("OOM");
						let rid = chunk.spawn_node(
							SemanticKind::RelationalTable,
							None,
							SpanMetrics {
								byte_length: 0,
								newlines: 0,
							},
						);
						let leaf = chunk.spawn_node(
							SemanticKind::Token,
							None,
							SpanMetrics {
								byte_length: 0,
								newlines: 0,
							},
						);
						chunk.append_local_child(rid, leaf);
						unsafe {
							*registry.virtual_data[leaf.index()].get() = Some(Vec::new());
						}
						file_path = Some(path);

						root_id = Some(rid);
						cursor_node = leaf;
						cursor_offset = 0;
						cursor_abs_line = DocLine::ZERO;
						cursor_abs_col = VisualCol::ZERO;
						ledger.clear();
						semantic_highlights.clear();
						last_semantic_state = u64::MAX;
						last_semantic_len = usize::MAX;
						last_semantic_path = None;
						pending_semantic_request_id = None;
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
											ledger.mark_saved();
											status_message =
												Some(format!("\"{}\" {}B written", path, len));
										}
										Err(e) => {
											status_message = Some(format!("Write error: {}", e))
										}
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
										ledger.mark_saved();
										status_message =
											Some(format!("\"{}\" {}B written", path, len));
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
											ledger.mark_saved();
											status_message =
												Some(format!("\"{}\" {}B written", path, len));
											pending_quit = true;
										}
										Err(e) => {
											status_message = Some(format!("Write error: {}", e))
										}
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
						match build_regex(&pattern, false) {
							Ok(re) => match registry.collect_document_bytes(rid) {
								Ok(bytes) => {
									let matches = find_all_matches(&bytes, &re);
									if matches.is_empty() {
										status_message = Some("Pattern not found".to_string());
										search_pattern = Some(pattern);
										search_case_insensitive = false;
										search_matches.clear();
										search_match_index = None;
										search_match_info = None;
									} else {
										let count = matches.len();
										let idx = matches
											.iter()
											.position(|&(l, c, _)| {
												l > cursor_abs_line
													|| (l == cursor_abs_line && c >= cursor_abs_col)
											})
											.unwrap_or(0);
										search_match_index = Some(idx);
										let (ml, mc, _) = matches[idx];
										cursor_abs_line = ml;
										let target = registry.find_node_at_line_col(rid, ml, mc);
										apply_cursor_target(
											target,
											&mut cursor_node,
											&mut cursor_offset,
											&mut cursor_abs_col,
										);
										cursor_abs_col = mc;
										search_pattern = Some(pattern);
										search_case_insensitive = false;
										search_matches = matches;
										search_match_info =
											Some(format!("[{}/{}]", idx + 1, count));
									}
								}
								Err(msg) => {
									status_message = Some(msg.to_string());
								}
							},
							Err(msg) => {
								status_message = Some(msg);
							}
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
								Some(i) => {
									if i + 1 >= search_matches.len() {
										0
									} else {
										i + 1
									}
								}
								None => 0,
							};
							let wrapped =
								search_match_index.is_some_and(|i| i + 1 >= search_matches.len());
							search_match_index = Some(idx);
							let (ml, mc, _) = search_matches[idx];
							cursor_abs_line = ml;
							let target = registry.find_node_at_line_col(rid, ml, mc);
							apply_cursor_target(
								target,
								&mut cursor_node,
								&mut cursor_offset,
								&mut cursor_abs_col,
							);
							cursor_abs_col = mc;
							let info = format!("[{}/{}]", idx + 1, search_matches.len());
							search_match_info = Some(if wrapped {
								format!("{} (wrapped)", info)
							} else {
								info
							});
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
							let (ml, mc, _) = search_matches[idx];
							cursor_abs_line = ml;
							let target = registry.find_node_at_line_col(rid, ml, mc);
							apply_cursor_target(
								target,
								&mut cursor_node,
								&mut cursor_offset,
								&mut cursor_abs_col,
							);
							cursor_abs_col = mc;
							let info = format!("[{}/{}]", idx + 1, search_matches.len());
							search_match_info = Some(if wrapped {
								format!("{} (wrapped)", info)
							} else {
								info
							});
						}
					}
					needs_render = true;
				}
				EditorCommand::SubstituteAll {
					pattern,
					replacement,
					range,
					flags,
				} => {
					if let Some(rid) = root_id {
						match build_regex(&pattern, flags.case_insensitive) {
							Ok(re) => match registry.collect_document_bytes(rid) {
								Ok(bytes) => {
									let byte_range =
										resolve_byte_range(&range, &bytes, cursor_abs_line);
									let (new_bytes, count) = substitute_bytes(
										&bytes,
										&re,
										replacement.as_bytes(),
										byte_range,
									);
									if count == 0 {
										status_message = Some("Pattern not found".to_string());
									} else {
										push_document_rewrite_delta(
											&mut ledger,
											&mut semantic_highlights,
											&bytes,
											&new_bytes,
										);
										let (new_root, _) =
											create_document_from_bytes(&registry, &new_bytes);
										root_id = Some(new_root);
										cursor_abs_line = DocLine::new(cursor_abs_line.get().min(
											new_bytes.iter().filter(|&&b| b == b'\n').count()
												as u32,
										));
										let target = registry.find_node_at_line_col(
											new_root,
											cursor_abs_line,
											VisualCol::ZERO,
										);
										apply_cursor_target(
											target,
											&mut cursor_node,
											&mut cursor_offset,
											&mut cursor_abs_col,
										);

										search_matches.clear();
										search_match_index = None;
										search_match_info = None;
										status_message = Some(format!("{} substitution(s)", count));
									}
								}
								Err(msg) => {
									status_message = Some(msg.to_string());
								}
							},
							Err(msg) => {
								status_message = Some(msg);
							}
						}
					}
					needs_render = true;
				}
				EditorCommand::SubstituteConfirm {
					pattern,
					replacement,
					range,
					flags,
				} => {
					if let Some(rid) = root_id {
						match build_regex(&pattern, flags.case_insensitive) {
							Ok(re) => match registry.collect_document_bytes(rid) {
								Ok(bytes) => {
									let all_matches = find_all_matches(&bytes, &re);
									let matches: Vec<(DocLine, VisualCol, usize)> = match &range {
										SubstituteRange::WholeFile => all_matches,
										SubstituteRange::CurrentLine => all_matches
											.into_iter()
											.filter(|&(l, _, _)| l == cursor_abs_line)
											.collect(),
										SubstituteRange::SingleLine(n) => {
											let n = *n;
											all_matches
												.into_iter()
												.filter(move |&(l, _, _)| l.get() == n)
												.collect()
										}
										SubstituteRange::LineRange(a, b) => {
											let (a, b) = (*a, *b);
											all_matches
												.into_iter()
												.filter(move |&(l, _, _)| {
													l.get() >= a && l.get() <= b
												})
												.collect()
										}
									};
									if matches.is_empty() {
										status_message = Some("Pattern not found".to_string());
									} else {
										let total = matches.len() as u32;
										search_pattern = Some(pattern);
										search_case_insensitive = flags.case_insensitive;
										search_matches = matches;
										search_match_index = Some(0);
										confirm_state = Some(ConfirmState {
											replacement,
											replacements_done: 0,
											total_matches: total,
											flags,
											range,
										});
										let (ml, mc, _) = search_matches[0];
										cursor_abs_line = ml;
										let target = registry.find_node_at_line_col(rid, ml, mc);
										apply_cursor_target(
											target,
											&mut cursor_node,
											&mut cursor_offset,
											&mut cursor_abs_col,
										);
										cursor_abs_col = mc;
										mode_override = Some(EditorMode::Confirm);
										status_message =
											Some(format!("Replace? [y/n/a/q] (1/{})", total));
									}
								}
								Err(msg) => {
									status_message = Some(msg.to_string());
								}
							},
							Err(msg) => {
								status_message = Some(msg);
							}
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
									let (ml, mc, _) = search_matches[idx];
									// Find byte offset of this match (mc is visual column)
									let mut byte_off = 0usize;
									let mut line = DocLine::ZERO;
									let mut col = VisualCol::ZERO;
									for &b in bytes.iter() {
										if line == ml && col == mc {
											break;
										}
										advance_col(b, &mut line, &mut col);
										byte_off += 1;
									}
									let rep = cs.replacement.as_bytes();
									let mut new_bytes = Vec::with_capacity(bytes.len());
									new_bytes.extend_from_slice(&bytes[..byte_off]);
									new_bytes.extend_from_slice(rep);
									let (_, _, match_len) = search_matches[idx];
									new_bytes.extend_from_slice(&bytes[byte_off + match_len..]);
									cs.replacements_done += 1;
									push_document_rewrite_delta(
										&mut ledger,
										&mut semantic_highlights,
										&bytes,
										&new_bytes,
									);

									let (new_root, _) =
										create_document_from_bytes(&registry, &new_bytes);
									root_id = Some(new_root);
									let new_rid = new_root;

									// Re-scan for remaining matches after replacement point, filtered by range
									let re = build_regex(&pat, cs.flags.case_insensitive).unwrap();
									let all_new = find_all_matches(&new_bytes, &re);
									let new_matches: Vec<(DocLine, VisualCol, usize)> =
										match &cs.range {
											SubstituteRange::WholeFile => all_new,
											SubstituteRange::CurrentLine => all_new
												.into_iter()
												.filter(|&(l, _, _)| l == ml)
												.collect(),
											SubstituteRange::SingleLine(n) => {
												let n = *n;
												all_new
													.into_iter()
													.filter(move |&(l, _, _)| l.get() == n)
													.collect()
											}
											SubstituteRange::LineRange(a, b) => {
												let (a, b) = (*a, *b);
												all_new
													.into_iter()
													.filter(move |&(l, _, _)| {
														l.get() >= a && l.get() <= b
													})
													.collect()
											}
										};
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
									let next_idx = new_matches.iter().position(|&(l, c, _)| {
										l > rep_end_line || (l == rep_end_line && c >= rep_end_col)
									});

									search_matches = new_matches;

									if let Some(ni) = next_idx {
										search_match_index = Some(ni);
										let (ml2, mc2, _) = search_matches[ni];
										cursor_abs_line = ml2;
										let target =
											registry.find_node_at_line_col(new_rid, ml2, mc2);
										apply_cursor_target(
											target,
											&mut cursor_node,
											&mut cursor_offset,
											&mut cursor_abs_col,
										);
										cursor_abs_col = mc2;
										status_message = Some(format!(
											"Replace? [y/n/a/q] ({}/{})",
											cs.replacements_done + 1,
											cs.total_matches
										));
									} else {
										status_message = Some(format!(
											"{} substitution(s)",
											cs.replacements_done
										));
										mode_override = Some(EditorMode::Normal);
										confirm_state = None;
										let target = registry.find_node_at_line_col(
											new_rid,
											cursor_abs_line,
											cursor_abs_col,
										);
										apply_cursor_target(
											target,
											&mut cursor_node,
											&mut cursor_offset,
											&mut cursor_abs_col,
										);
									}
								}
							}
							ConfirmAction::No => {
								let idx = search_match_index.unwrap_or(0);
								if idx + 1 < search_matches.len() {
									search_match_index = Some(idx + 1);
									let (ml, mc, _) = search_matches[idx + 1];
									cursor_abs_line = ml;
									let target = registry.find_node_at_line_col(rid, ml, mc);
									apply_cursor_target(
										target,
										&mut cursor_node,
										&mut cursor_offset,
										&mut cursor_abs_col,
									);
									cursor_abs_col = mc;
									status_message = Some(format!(
										"Replace? [y/n/a/q] ({}/{})",
										idx + 2,
										cs.total_matches
									));
								} else {
									status_message =
										Some(format!("{} substitution(s)", cs.replacements_done));
									mode_override = Some(EditorMode::Normal);
									confirm_state = None;
								}
							}
							ConfirmAction::All => {
								if let Ok(bytes) = registry.collect_document_bytes(rid) {
									// Replace all remaining from current position
									let idx = search_match_index.unwrap_or(0);
									let (ml, mc, _) = search_matches[idx];
									let mut byte_off = 0usize;
									let mut line = DocLine::ZERO;
									let mut col = VisualCol::ZERO;
									for &b in bytes.iter() {
										if line == ml && col == mc {
											break;
										}
										advance_col(b, &mut line, &mut col);
										byte_off += 1;
									}
									let remaining = &bytes[byte_off..];
									let re = build_regex(&pat, cs.flags.case_insensitive).unwrap();
									let (replaced, count) = substitute_bytes(
										remaining,
										&re,
										cs.replacement.as_bytes(),
										None,
									);
									let mut new_bytes = Vec::with_capacity(bytes.len());
									new_bytes.extend_from_slice(&bytes[..byte_off]);
									new_bytes.extend_from_slice(&replaced);
									cs.replacements_done += count;
									push_document_rewrite_delta(
										&mut ledger,
										&mut semantic_highlights,
										&bytes,
										&new_bytes,
									);

									let (new_root, _new_leaf) =
										create_document_from_bytes(&registry, &new_bytes);
									root_id = Some(new_root);

									let target = registry.find_node_at_line_col(
										new_root,
										cursor_abs_line,
										cursor_abs_col,
									);
									apply_cursor_target(
										target,
										&mut cursor_node,
										&mut cursor_offset,
										&mut cursor_abs_col,
									);
									status_message =
										Some(format!("{} substitution(s)", cs.replacements_done));
								}
								mode_override = Some(EditorMode::Normal);
								search_matches.clear();
								search_match_index = None;
								search_match_info = None;
								confirm_state = None;
							}
							ConfirmAction::Quit => {
								let done = cs.replacements_done;
								status_message = Some(format!("{} substitution(s)", done));
								mode_override = Some(EditorMode::Normal);
								confirm_state = None;
								search_matches.clear();
								search_match_index = None;
								search_match_info = None;
							}
						}
					}
					needs_render = true;
				}
				EditorCommand::YankLine { register } => {
					if let Some(rid) = root_id {
						if let Ok(bytes) = registry.collect_document_bytes(rid) {
							let global_offset =
								byte_offset_from_line_col(&bytes, cursor_abs_line, VisualCol::ZERO);
							let off = global_offset.get() as usize;
							// Find the end of the current line (including the \n).
							let line_end = bytes[off..]
								.iter()
								.position(|&b| b == b'\n')
								.map(|p| off + p + 1)
								.unwrap_or(bytes.len());
							let line_text =
								String::from_utf8_lossy(&bytes[off..line_end]).into_owned();

							// Always store in the unnamed register.
							registers.insert('"', line_text.clone());
							if register != '"' && register != '+' {
								registers.insert(register, line_text.clone());
							}

							// If '+' register was requested, also push to OS clipboard.
							if register == '+' {
								clipboard.set_text(&line_text);
							}

							// Yank flash: highlight the yanked line for 200ms.
							yank_flash =
								Some((DocByte::new(off as u64), DocByte::new(line_end as u64)));
							let tx_flash = tx_cmd.clone();
							std::thread::spawn(move || {
								std::thread::sleep(std::time::Duration::from_millis(200));
								let _ = tx_flash.send(EditorCommand::ClearFlash);
							});

							let line_len = line_end - off;
							status_message = Some(format!("{} bytes yanked", line_len));
							needs_render = true;
						}
					}
				}
				EditorCommand::Put { register } => {
					if let Some(rid) = root_id {
						// Resolve the text to paste.
						let paste_text: Option<String> = if register == '+' {
							// System clipboard — read from OS, fall back to unnamed register.
							clipboard
								.get_text()
								.or_else(|| registers.get(&'"').cloned())
						} else {
							registers
								.get(&register)
								.cloned()
								.or_else(|| registers.get(&'"').cloned())
						};

						if let Some(text) = paste_text {
							if !text.is_empty() {
								if let Ok(bytes) = registry.collect_document_bytes(rid) {
									// Insert after current line (Vim 'p' for line-wise yank).
									let (insert_offset, inserted_text, target_cursor_line) =
										linewise_put_insertion(&bytes, cursor_abs_line, &text);

									// Build new document through delta + ledger.
									let mut new_bytes =
										Vec::with_capacity(bytes.len() + inserted_text.len());
									new_bytes.extend_from_slice(&bytes[..insert_offset]);
									new_bytes.extend_from_slice(inserted_text.as_bytes());
									new_bytes.extend_from_slice(&bytes[insert_offset..]);

									push_delta_and_rebase(
										&mut ledger,
										&mut semantic_highlights,
										TextDelta {
											global_byte_offset: DocByte::new(insert_offset as u64),
											deleted_text: String::new(),
											inserted_text,
											state_before: 0,
											state_after: 0,
										},
									);

									let (new_root, _) =
										create_document_from_bytes(&registry, &new_bytes);
									root_id = Some(new_root);

									// Place cursor on the first line of pasted text.
									cursor_abs_line = target_cursor_line;
									cursor_abs_col = VisualCol::ZERO;
									let target = registry.find_node_at_line_col(
										new_root,
										cursor_abs_line,
										cursor_abs_col,
									);
									apply_cursor_target(
										target,
										&mut cursor_node,
										&mut cursor_offset,
										&mut cursor_abs_col,
									);
									needs_render = true;
								}
							}
						} else {
							status_message = Some("Register empty".to_string());
							needs_render = true;
						}
					}
				}
				EditorCommand::Undo => {
					if let Some(rid) = root_id {
						if let Some((new_root, new_leaf, cursor_byte, _, delta)) =
							ledger.undo(&registry, rid)
						{
							rebase_semantic_highlights_after_delta(
								&mut semantic_highlights,
								&invert_text_delta(&delta),
							);
							root_id = Some(new_root);
							cursor_node = new_leaf;
							cursor_offset = 0;
							if let Ok(bytes) = registry.collect_document_bytes(new_root) {
								let (line, col) = line_col_from_byte_offset(&bytes, cursor_byte);
								cursor_abs_line = line;
								let target = registry.find_node_at_line_col(new_root, line, col);
								apply_cursor_target(
									target,
									&mut cursor_node,
									&mut cursor_offset,
									&mut cursor_abs_col,
								);
							}
							needs_render = true;
						} else {
							status_message = Some("Already at oldest change".to_string());
							needs_render = true;
						}
					}
				}
				EditorCommand::Redo => {
					if let Some(rid) = root_id {
						if let Some((new_root, new_leaf, cursor_byte, _, delta)) =
							ledger.redo(&registry, rid)
						{
							rebase_semantic_highlights_after_delta(
								&mut semantic_highlights,
								&delta,
							);
							root_id = Some(new_root);
							cursor_node = new_leaf;
							cursor_offset = 0;
							if let Ok(bytes) = registry.collect_document_bytes(new_root) {
								let (line, col) = line_col_from_byte_offset(&bytes, cursor_byte);
								cursor_abs_line = line;
								let target = registry.find_node_at_line_col(new_root, line, col);
								apply_cursor_target(
									target,
									&mut cursor_node,
									&mut cursor_offset,
									&mut cursor_abs_col,
								);
							}
							needs_render = true;
						} else {
							status_message = Some("Already at newest change".to_string());
							needs_render = true;
						}
					}
				}
				EditorCommand::ClearFlash => {
					yank_flash = None;
					needs_render = true;
				}
				EditorCommand::InternalRefresh => {
					needs_render = true;
				}
				EditorCommand::Quit => break,
			}

			if needs_render {
				let (virtual_tokens, tokens, total_lines) = if let Some(rid) = root_id {
					let scroll_y = cursor_abs_line.saturating_sub(20);

					// 1. Virtual Query (Look-behind 60 lines, Look-ahead 120 lines total)
					let virtual_start_line = scroll_y.saturating_sub(60);
					let virtual_line_count = viewport_lines + 120;
					let virtual_tokens =
						registry.query_viewport(rid, virtual_start_line, virtual_line_count);

					// 2. Visible Query (The actual UI screen)
					let visible_tokens = registry.query_viewport(rid, scroll_y, viewport_lines);

					for token in &visible_tokens {
						if !token.is_virtual && token.text.is_empty() {
							let idx = token.node_id.index();
							if !registry.dma_in_flight[idx].swap(true, Ordering::Relaxed) {
								if let Some(svp) = unsafe { *registry.spans[idx].get() } {
									resolver.request_dma(token.node_id, svp, RequestPriority::High);
								}
							}
						}
					}

					(
						virtual_tokens,
						visible_tokens,
						registry.get_total_newlines(rid),
					)
				} else {
					(Vec::new(), Vec::new(), 0)
				};

				let confirm_prompt = confirm_state
					.as_ref()
					.map(|_| "Replace? [y/n/a/q]".to_string());

				let mut global_start_byte = DocByte::ZERO;
				let mut highlights = Vec::new();

				if let Some(path) = &file_path {
					if path.ends_with(".rs") {
						if let Some(first_v_token) = virtual_tokens.first() {
							let v_global_start = first_v_token.absolute_start_byte;
							let mut virtual_buffer = Vec::new();
							for token in &virtual_tokens {
								virtual_buffer.extend_from_slice(token.text.as_bytes());
							}
							highlights = crate::svp::pipeline::SvpPipeline::process_viewport(
								v_global_start,
								&virtual_buffer,
							);

							// Dual-Lock Gate: triggers on user mutation OR silent DMA load.
							if let Some(rid) = root_id {
								if let Ok(live_bytes) = registry.collect_document_bytes(rid) {
									let path_changed =
										last_semantic_path.as_deref() != Some(path.as_str());
									if path_changed
										|| ledger.current_state_id != last_semantic_state
										|| live_bytes.len() != last_semantic_len
									{
										// Skip incomplete 0-byte ghost reads on startup.
										if !live_bytes.is_empty() && live_bytes.len() < 1_048_576 {
											let text =
												String::from_utf8_lossy(&live_bytes).into_owned();
											let request_id = next_semantic_request_id;
											next_semantic_request_id += 1;
											reactor.send(SemanticRequest {
												content: text,
												global_offset: DocByte::ZERO,
												file_path: file_path.clone().unwrap_or_default(),
												state_id: ledger.current_state_id,
												request_id,
											});
											// Lock only after a successful, non-empty send.
											last_semantic_state = ledger.current_state_id;
											last_semantic_len = live_bytes.len();
											last_semantic_path = Some(path.clone());
											pending_semantic_request_id = Some(request_id);
										}
									}
								}
							}
						} else {
							semantic_highlights.clear();
							last_semantic_state = ledger.current_state_id;
							last_semantic_len = 0;
							last_semantic_path = Some(path.clone());
							pending_semantic_request_id = None;
						}
					} else {
						semantic_highlights.clear();
						last_semantic_state = u64::MAX;
						last_semantic_len = usize::MAX;
						last_semantic_path = None;
						pending_semantic_request_id = None;
					}
				} else {
					semantic_highlights.clear();
					last_semantic_state = u64::MAX;
					last_semantic_len = usize::MAX;
					last_semantic_path = None;
					pending_semantic_request_id = None;
				}

				// Non-blocking poll: grab latest semantic highlights if ready.
				while let Some(response) = reactor.try_recv() {
					if Some(response.request_id) == pending_semantic_request_id
						&& response.state_id == ledger.current_state_id
					{
						semantic_highlights = response.highlights;
						pending_semantic_request_id = None;
					}
				}

				if !semantic_highlights.is_empty() {
					highlights = merge_highlights(&highlights, &semantic_highlights);
				}

				if let Some(first_visible) = tokens.first() {
					global_start_byte = first_visible.absolute_start_byte;
				}

				let _ = tx_view.send(Viewport {
					tokens,
					cursor_abs_pos: CursorPosition::new(cursor_abs_line, cursor_abs_col),
					total_lines,
					status_message: status_message.take(),
					should_quit: pending_quit,
					file_name: file_path.clone(),
					file_size: root_id
						.and_then(|rid| registry.collect_document_bytes(rid).ok())
						.map(|b| b.len() as u64)
						.unwrap_or(0),
					is_dirty: ledger.is_dirty(),
					search_pattern: search_pattern.clone(),
					search_case_insensitive,
					search_match_info: search_match_info.clone(),
					confirm_prompt,
					mode_override: mode_override.take(),
					global_start_byte,
					highlights,
					yank_flash,
				});

				if pending_quit {
					break;
				}
			}
		}
	}
}

/// Composite semantic highlights over lexical highlights.
///
/// Semantic tags (struct, trait, variable resolution) overwrite lexical tags
/// wherever their byte ranges overlap. Lexical spans that fall outside any
/// semantic span are preserved unchanged.
fn merge_highlights(lexical: &[HighlightSpan], semantic: &[HighlightSpan]) -> Vec<HighlightSpan> {
	let mut merged = Vec::with_capacity(lexical.len() + semantic.len());

	for &lex_span in lexical {
		// Binary search for the first semantic span that could overlap.
		let search = semantic.partition_point(|span| span.end <= lex_span.start);
		let mut overwritten = false;
		for sem_span in &semantic[search..] {
			if sem_span.start >= lex_span.end {
				break;
			}
			// Semantic span overlaps this lexical span — replace it.
			overwritten = true;
			break;
		}
		if !overwritten {
			merged.push(lex_span);
		}
	}

	// Append all semantic spans (they are authoritative where they exist).
	merged.extend_from_slice(semantic);

	// Sort by start byte for the projector's binary search.
	merged.sort_unstable_by_key(|span| span.start);
	merged
}

#[cfg(test)]
mod tests {
	use super::{
		document_rewrite_delta, linewise_put_insertion, rebase_semantic_highlights_after_delta,
	};
	use crate::core::{DocByte, DocLine};
	use crate::engine::undo::TextDelta;
	use crate::svp::highlight::{HighlightSpan, TokenCategory};

	#[test]
	fn rewrite_delta_tracks_changed_middle_span() {
		let delta =
			document_rewrite_delta("fn main() {}\n", "fn demo() {}\n").expect("expected a delta");
		assert_eq!(delta.0, 3);
		assert_eq!(delta.1, "main");
		assert_eq!(delta.2, "demo");
	}

	#[test]
	fn linewise_put_adds_newline_at_eof_without_trailing_break() {
		let (insert_offset, inserted_text, target_cursor_line) =
			linewise_put_insertion(b"alpha", DocLine::ZERO, "beta");
		assert_eq!(insert_offset, 5);
		assert_eq!(inserted_text, "\nbeta");
		assert_eq!(target_cursor_line, DocLine::new(1));
	}

	#[test]
	fn linewise_put_stays_on_first_line_for_empty_docs() {
		let (insert_offset, inserted_text, target_cursor_line) =
			linewise_put_insertion(b"", DocLine::ZERO, "beta\n");
		assert_eq!(insert_offset, 0);
		assert_eq!(inserted_text, "beta\n");
		assert_eq!(target_cursor_line, DocLine::ZERO);
	}

	#[test]
	fn semantic_cache_rebases_spans_after_backspace() {
		let mut semantic = vec![
			HighlightSpan::new(DocByte::new(0), DocByte::new(3), TokenCategory::Keyword),
			HighlightSpan::new(DocByte::new(10), DocByte::new(13), TokenCategory::Module),
		];

		rebase_semantic_highlights_after_delta(
			&mut semantic,
			&TextDelta {
				global_byte_offset: DocByte::new(5),
				deleted_text: "x".to_string(),
				inserted_text: String::new(),
				state_before: 0,
				state_after: 0,
			},
		);

		assert_eq!(
			semantic,
			vec![
				HighlightSpan::new(DocByte::new(0), DocByte::new(3), TokenCategory::Keyword),
				HighlightSpan::new(DocByte::new(9), DocByte::new(12), TokenCategory::Module),
			]
		);
	}

	#[test]
	fn semantic_cache_drops_spans_crossing_the_edit_boundary() {
		let mut semantic = vec![
			HighlightSpan::new(DocByte::new(4), DocByte::new(7), TokenCategory::Variable),
			HighlightSpan::new(DocByte::new(10), DocByte::new(14), TokenCategory::Type),
		];

		rebase_semantic_highlights_after_delta(
			&mut semantic,
			&TextDelta {
				global_byte_offset: DocByte::new(5),
				deleted_text: String::new(),
				inserted_text: "x".to_string(),
				state_before: 0,
				state_after: 0,
			},
		);

		assert_eq!(
			semantic,
			vec![HighlightSpan::new(
				DocByte::new(11),
				DocByte::new(15),
				TokenCategory::Type,
			)]
		);
	}
}
