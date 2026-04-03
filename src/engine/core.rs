use crate::core::{
	CursorPosition, DocByte, DocLine, NodeByteOffset, RequestId, StateId, TAB_SIZE, VisualCol,
};
use crate::ecs::{NodeId, UastRegistry};
use crate::engine::clipboard::ClipboardHandle;
use crate::engine::undo::{
	TextDelta, UndoLedger, byte_offset_from_line_col, line_col_from_byte_offset,
};
use crate::svp::highlight::HighlightSpan;
use crate::svp::semantic::{SemanticReactor, SemanticRequest};
use crate::svp::{RequestPriority, SvpPointer, SvpResolver, ingest_svp_file};
use crate::uast::{
	MinimapMode, MinimapSnapshot, NodeByteTarget, NodeCursorTarget, UastMutation, UastProjection,
	Viewport,
};
use crate::ui::Theme;
use ra_ap_syntax::{AstNode, Direction, Edition, SourceFile, SyntaxKind, SyntaxToken, TextSize};
use regex_automata::meta::Regex;
use regex_automata::util::syntax;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc;

const FILE_DEVICE_ID: u16 = 0x42;
const MAX_SEMANTIC_BYTES: u64 = 1_048_576;
const MAX_MINIMAP_TEXT_BYTES: u64 = 1_048_576;
const MINIMAP_BANDS: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorMode {
	Normal,
	Insert,
	Command,
	Search,
	Confirm,
	Visual { anchor: DocByte, kind: VisualKind },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualKind {
	Char,
	Line,
	Block,
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
	NextWord,
	PrevWord,
	NextWordEnd,
	Top,
	Bottom,
}

#[derive(Debug, Clone)]
pub enum SubstituteRange {
	WholeFile,
	CurrentLine,
	SingleLine(DocLine),
	LineRange(DocLine, DocLine),
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
	Delete,
	Resize(u16, u16),
	Scroll(i32),
	ScrollViewport(i32),
	MoveCursor(MoveDirection),
	ClickCursor(CursorPosition),
	GotoLine(DocLine),
	LineStart,
	FirstNonWhitespace,
	LineEnd,
	SmartHome,
	PageUp,
	PageDown,
	DeleteInnerWord,
	ChangeInnerWord,
	DeleteToLineEnd,
	SetVisualSelection {
		anchor: DocByte,
		kind: VisualKind,
	},
	ClearVisualSelection,
	VisualYank {
		anchor: DocByte,
		kind: VisualKind,
	},
	VisualDelete {
		anchor: DocByte,
		kind: VisualKind,
	},
	VisualChange {
		anchor: DocByte,
		kind: VisualKind,
	},
	LoadFile(String),
	WriteFile,
	WriteFileAs(String),
	WriteAndQuit,
	SetTheme(String),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SearchMatch {
	line: DocLine,
	col: VisualCol,
	byte_len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LexClass {
	Whitespace,
	Keyword,
	Punctuation,
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

fn classify_lex_char(ch: char) -> LexClass {
	match ch {
		' ' | '\t' | '\n' => LexClass::Whitespace,
		_ if ch.is_alphanumeric() || ch == '_' => LexClass::Keyword,
		_ => LexClass::Punctuation,
	}
}

fn clamp_to_char_boundary(doc: &str, byte: usize) -> usize {
	let mut byte = byte.min(doc.len());
	while byte > 0 && !doc.is_char_boundary(byte) {
		byte -= 1;
	}
	byte
}

fn char_start_before(doc: &str, byte: usize) -> Option<usize> {
	let byte = clamp_to_char_boundary(doc, byte);
	doc[..byte].char_indices().next_back().map(|(idx, _)| idx)
}

fn char_at(doc: &str, byte: usize) -> Option<char> {
	let byte = clamp_to_char_boundary(doc, byte);
	doc[byte..].char_indices().next().map(|(_, ch)| ch)
}

fn next_char_start(doc: &str, byte: usize) -> Option<usize> {
	let byte = clamp_to_char_boundary(doc, byte);
	let ch = char_at(doc, byte)?;
	Some(byte + ch.len_utf8())
}

fn next_word_start(doc: &str, current_byte: usize) -> usize {
	let mut byte = clamp_to_char_boundary(doc, current_byte);
	if byte >= doc.len() {
		return doc.len();
	}

	let start_class = classify_lex_char(char_at(doc, byte).expect("char at valid boundary"));
	while byte < doc.len() {
		let ch = char_at(doc, byte).expect("char at valid boundary");
		if classify_lex_char(ch) != start_class {
			break;
		}
		byte = match next_char_start(doc, byte) {
			Some(next) => next,
			None => return doc.len(),
		};
	}

	while byte < doc.len() {
		let ch = char_at(doc, byte).expect("char at valid boundary");
		if classify_lex_char(ch) != LexClass::Whitespace {
			break;
		}
		byte = match next_char_start(doc, byte) {
			Some(next) => next,
			None => return doc.len(),
		};
	}

	byte
}

fn prev_word_start(doc: &str, current_byte: usize) -> usize {
	let current_byte = clamp_to_char_boundary(doc, current_byte);
	let Some(mut byte) = char_start_before(doc, current_byte) else {
		return 0;
	};

	while let Some(ch) = char_at(doc, byte) {
		if classify_lex_char(ch) != LexClass::Whitespace {
			break;
		}
		let Some(prev) = char_start_before(doc, byte) else {
			return 0;
		};
		byte = prev;
	}

	let Some(ch) = char_at(doc, byte) else {
		return 0;
	};
	let target_class = classify_lex_char(ch);

	while let Some(prev) = char_start_before(doc, byte) {
		let prev_ch = char_at(doc, prev).expect("char at valid boundary");
		if classify_lex_char(prev_ch) != target_class {
			break;
		}
		byte = prev;
	}

	byte
}

fn next_word_end(doc: &str, current_byte: usize) -> usize {
	let current_byte = clamp_to_char_boundary(doc, current_byte);
	let Some(mut byte) = next_char_start(doc, current_byte) else {
		return current_byte.min(doc.len());
	};

	while byte < doc.len() {
		let ch = char_at(doc, byte).expect("char at valid boundary");
		if classify_lex_char(ch) != LexClass::Whitespace {
			break;
		}
		byte = match next_char_start(doc, byte) {
			Some(next) => next,
			None => return current_byte.min(doc.len()),
		};
	}

	if byte >= doc.len() {
		return current_byte.min(doc.len());
	}

	let target_class = classify_lex_char(char_at(doc, byte).expect("char at valid boundary"));
	let mut last = byte;

	while byte < doc.len() {
		let ch = char_at(doc, byte).expect("char at valid boundary");
		if classify_lex_char(ch) != target_class {
			break;
		}
		last = byte;
		byte = match next_char_start(doc, byte) {
			Some(next) => next,
			None => break,
		};
	}

	last
}

fn line_byte_range(doc: &[u8], start_line: DocLine, end_line: DocLine) -> (usize, usize) {
	let mut current_line = DocLine::ZERO;
	let mut byte_start = 0usize;
	let mut found_start = start_line == DocLine::ZERO;

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

fn line_content_slice(doc: &[u8], target_line: DocLine) -> &[u8] {
	let (start, end) = line_byte_range(doc, target_line, target_line);
	let line = &doc[start..end];
	if line.ends_with(b"\n") {
		&line[..line.len().saturating_sub(1)]
	} else {
		line
	}
}

fn advance_visual_col_only(col: &mut VisualCol, b: u8) {
	if b == b'\t' {
		*col += TAB_SIZE - (col.get() % TAB_SIZE);
	} else if b != b'\n' {
		*col += 1;
	}
}

fn line_end_visual_col(doc: &[u8], target_line: DocLine) -> VisualCol {
	let mut col = VisualCol::ZERO;
	for &b in line_content_slice(doc, target_line) {
		advance_visual_col_only(&mut col, b);
	}
	col
}

fn first_non_whitespace_visual_col(doc: &[u8], target_line: DocLine) -> VisualCol {
	let mut col = VisualCol::ZERO;
	for &b in line_content_slice(doc, target_line) {
		match b {
			b' ' | b'\t' => advance_visual_col_only(&mut col, b),
			_ => return col,
		}
	}

	VisualCol::ZERO
}

fn smart_home_visual_col(doc: &[u8], target_line: DocLine, current_col: VisualCol) -> VisualCol {
	let first_non_whitespace = first_non_whitespace_visual_col(doc, target_line);
	if current_col == first_non_whitespace {
		VisualCol::ZERO
	} else {
		first_non_whitespace
	}
}

fn step_left_visual_col(doc: &[u8], target_line: DocLine, current_col: VisualCol) -> VisualCol {
	let mut col = VisualCol::ZERO;

	for &b in line_content_slice(doc, target_line) {
		let char_start = col;
		advance_visual_col_only(&mut col, b);
		if current_col <= col {
			return char_start;
		}
	}

	VisualCol::ZERO
}

fn step_right_visual_col(doc: &[u8], target_line: DocLine, current_col: VisualCol) -> VisualCol {
	let mut col = VisualCol::ZERO;

	for &b in line_content_slice(doc, target_line) {
		advance_visual_col_only(&mut col, b);
		if current_col < col {
			return col;
		}
	}

	col
}

fn page_motion_step(viewport_lines: u32) -> u32 {
	if viewport_lines > 1 {
		viewport_lines - 1
	} else {
		1
	}
}

fn max_scroll_y(total_lines: u32, viewport_lines: u32) -> u32 {
	total_lines.saturating_sub(viewport_lines.max(1).saturating_sub(1))
}

fn clamp_scroll_y(scroll_y: u32, total_lines: u32, viewport_lines: u32) -> u32 {
	scroll_y.min(max_scroll_y(total_lines, viewport_lines))
}

fn viewport_bottom_line(scroll_y: u32, total_lines: u32, viewport_lines: u32) -> u32 {
	clamp_scroll_y(scroll_y, total_lines, viewport_lines)
		.saturating_add(viewport_lines.max(1).saturating_sub(1))
		.min(total_lines)
}

fn pan_scroll_y_to_keep_cursor_visible(
	scroll_y: u32,
	cursor_line: DocLine,
	total_lines: u32,
	viewport_lines: u32,
) -> u32 {
	let viewport_lines = viewport_lines.max(1);
	let mut scroll_y = clamp_scroll_y(scroll_y, total_lines, viewport_lines);
	if cursor_line.get() < scroll_y {
		scroll_y = cursor_line.get();
	} else if cursor_line.get() >= scroll_y.saturating_add(viewport_lines) {
		scroll_y = cursor_line
			.get()
			.saturating_sub(viewport_lines.saturating_sub(1));
	}
	clamp_scroll_y(scroll_y, total_lines, viewport_lines)
}

fn scroll_viewport(scroll_y: u32, delta: i32, total_lines: u32, viewport_lines: u32) -> u32 {
	let scroll_y = (scroll_y as i64 + delta as i64).max(0) as u32;
	clamp_scroll_y(scroll_y, total_lines, viewport_lines)
}

fn clamp_cursor_line_to_viewport(
	cursor_line: DocLine,
	scroll_y: u32,
	total_lines: u32,
	viewport_lines: u32,
) -> DocLine {
	let top = clamp_scroll_y(scroll_y, total_lines, viewport_lines);
	let bottom = viewport_bottom_line(top, total_lines, viewport_lines);
	DocLine::new(cursor_line.get().clamp(top, bottom))
}

fn resolve_byte_range(
	range: &SubstituteRange,
	doc: &[u8],
	cursor_line: DocLine,
) -> Option<(usize, usize)> {
	match range {
		SubstituteRange::WholeFile => None,
		SubstituteRange::CurrentLine => Some(line_byte_range(doc, cursor_line, cursor_line)),
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

fn find_all_matches(doc_bytes: &[u8], re: &Regex) -> Vec<SearchMatch> {
	let mut matches = Vec::new();
	let mut line = DocLine::ZERO;
	let mut col = VisualCol::ZERO;
	let mut prev_pos = 0usize;

	for m in re.find_iter(doc_bytes) {
		for &b in &doc_bytes[prev_pos..m.start()] {
			advance_col(b, &mut line, &mut col);
		}
		let match_len = m.end() - m.start();
		matches.push(SearchMatch {
			line,
			col,
			byte_len: match_len,
		});
		for &b in &doc_bytes[m.start()..m.end()] {
			advance_col(b, &mut line, &mut col);
		}
		prev_pos = m.end();
	}
	matches
}

fn substitute_text_deltas(
	doc: &[u8],
	re: &Regex,
	replacement: &str,
	byte_range: Option<(usize, usize)>,
) -> Vec<TextDelta> {
	let (start, end) = byte_range.unwrap_or((0, doc.len()));
	let mut deltas = Vec::new();

	for m in re.find_iter(&doc[start..end]) {
		let match_start = start + m.start();
		let match_end = start + m.end();
		deltas.push(TextDelta {
			global_byte_offset: DocByte::new(match_start as u64),
			deleted_text: String::from_utf8_lossy(&doc[match_start..match_end]).into_owned(),
			inserted_text: replacement.to_string(),
			state_before: StateId::ZERO,
			state_after: StateId::ZERO,
		});
	}

	deltas.reverse();
	deltas
}

fn temp_save_path(target_path: &Path) -> Result<PathBuf, String> {
	let file_name = target_path
		.file_name()
		.ok_or_else(|| "Invalid target path".to_string())?;
	let parent = target_path.parent().unwrap_or_else(|| Path::new("."));
	let file_name = file_name.to_string_lossy();
	let pid = std::process::id();

	for attempt in 0..1024u32 {
		let candidate = parent.join(format!(".{}.baryon.{}.{}.tmp", file_name, pid, attempt));
		if !candidate.exists() {
			return Ok(candidate);
		}
	}

	Err("Unable to allocate temporary save path".to_string())
}

fn copy_physical_span_to_writer(
	source_file: &mut File,
	span: SvpPointer,
	writer: &mut impl Write,
	buffer: &mut [u8],
) -> Result<u64, String> {
	let mut remaining = span.byte_length as usize;
	let source_offset = span.lba * 512 + u64::from(span.head_trim);
	source_file
		.seek(SeekFrom::Start(source_offset))
		.map_err(|e| format!("Write error: {}", e))?;

	let mut written = 0u64;
	while remaining > 0 {
		let chunk_len = remaining.min(buffer.len());
		source_file
			.read_exact(&mut buffer[..chunk_len])
			.map_err(|e| format!("Write error: {}", e))?;
		writer
			.write_all(&buffer[..chunk_len])
			.map_err(|e| format!("Write error: {}", e))?;
		remaining -= chunk_len;
		written += chunk_len as u64;
	}

	Ok(written)
}

fn stream_document_to_writer(
	registry: &UastRegistry,
	root: NodeId,
	source_path: Option<&Path>,
	writer: &mut impl Write,
) -> Result<u64, String> {
	let mut visit = registry.get_first_child(root);
	let mut source_file: Option<File> = None;
	let mut buffer = vec![0u8; 256 * 1024];
	let mut written = 0u64;

	while let Some(node) = visit {
		let idx = node.index();
		let has_children = unsafe { (*registry.edges[idx].get()).first_child.is_some() };

		if has_children {
			visit = registry.get_first_child(node);
			continue;
		}

		unsafe {
			if let Some(v_data) = &*registry.virtual_data[idx].get() {
				writer
					.write_all(v_data)
					.map_err(|e| format!("Write error: {}", e))?;
				written += v_data.len() as u64;
			} else if let Some(span) = *registry.spans[idx].get() {
				if source_file.is_none() {
					let Some(path) = source_path else {
						return Err(
							"Cannot stream-save physical spans without a source file".to_string()
						);
					};
					source_file =
						Some(File::open(path).map_err(|e| format!("Write error: {}", e))?);
				}

				written += copy_physical_span_to_writer(
					source_file
						.as_mut()
						.expect("source file must be opened for physical span"),
					span,
					writer,
					&mut buffer,
				)?;
			}
		}

		visit = registry.get_next_node_in_walk(node);
	}

	Ok(written)
}

fn save_document_atomic(
	registry: &UastRegistry,
	root: NodeId,
	source_path: Option<&Path>,
	target_path: &Path,
) -> Result<u64, String> {
	let temp_path = temp_save_path(target_path)?;

	let save_result = (|| -> Result<u64, String> {
		let temp_file = OpenOptions::new()
			.write(true)
			.create_new(true)
			.open(&temp_path)
			.map_err(|e| format!("Write error: {}", e))?;
		let mut writer = BufWriter::with_capacity(1024 * 1024, temp_file);
		let written = stream_document_to_writer(registry, root, source_path, &mut writer)?;
		writer.flush().map_err(|e| format!("Write error: {}", e))?;
		let temp_file = writer
			.into_inner()
			.map_err(|e| format!("Write error: {}", e.into_error()))?;
		temp_file
			.sync_all()
			.map_err(|e| format!("Write error: {}", e))?;
		std::fs::rename(&temp_path, target_path).map_err(|e| format!("Write error: {}", e))?;
		if let Some(parent) = target_path.parent() {
			if let Ok(dir) = File::open(parent) {
				let _ = dir.sync_all();
			}
		}
		Ok(written)
	})();

	if save_result.is_err() {
		let _ = std::fs::remove_file(&temp_path);
	}

	save_result
}

fn rebind_document_spans_to_saved_file(registry: &UastRegistry, root: NodeId, device_id: u16) {
	let mut visit = registry.get_first_child(root);
	let mut byte_offset = 0u64;

	while let Some(node) = visit {
		let idx = node.index();
		let has_children = unsafe { (*registry.edges[idx].get()).first_child.is_some() };

		if has_children {
			visit = registry.get_first_child(node);
			continue;
		}

		let byte_length = unsafe { (*registry.metrics[idx].get()).byte_length };
		unsafe {
			*registry.spans[idx].get() = if byte_length == 0 {
				None
			} else {
				Some(SvpPointer {
					lba: byte_offset / 512,
					byte_length,
					device_id,
					head_trim: (byte_offset % 512) as u16,
				})
			};
			*registry.virtual_data[idx].get() = None;
		}
		registry.dma_in_flight[idx].store(false, Ordering::Relaxed);
		registry.metrics_inflated[idx].store(byte_length != 0, Ordering::Relaxed);

		byte_offset += byte_length as u64;
		visit = registry.get_next_node_in_walk(node);
	}
}

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
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

fn rebase_semantic_highlights_after_deltas(
	semantic_highlights: &mut Vec<HighlightSpan>,
	deltas: &[TextDelta],
) {
	for delta in deltas {
		rebase_semantic_highlights_after_delta(semantic_highlights, delta);
	}
}

#[cfg(test)]
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

fn read_loaded_document(registry: &UastRegistry, root: NodeId) -> Result<Vec<u8>, String> {
	registry
		.read_loaded_slice(
			root,
			DocByte::ZERO,
			DocByte::new(registry.get_total_bytes(root)),
		)
		.map_err(|msg| msg.to_string())
}

fn normalize_line_density(byte_len: usize) -> u8 {
	let capped = byte_len.min(160) as u32;
	((capped * 255) / 160) as u8
}

fn build_text_minimap_snapshot(
	bytes: &[u8],
	viewport_start_line: DocLine,
	viewport_line_count: u32,
	cursor_line: DocLine,
) -> MinimapSnapshot {
	let doc_lines = memchr::memchr_iter(b'\n', bytes).count() as u32 + 1;
	let mut sums = vec![0u32; MINIMAP_BANDS];
	let mut counts = vec![0u32; MINIMAP_BANDS];
	let mut total_lines = 0u32;
	let mut current_len = 0usize;

	for &b in bytes {
		if b == b'\n' {
			let band = ((total_lines as usize) * MINIMAP_BANDS) / doc_lines.max(1) as usize;
			sums[band.min(MINIMAP_BANDS - 1)] += normalize_line_density(current_len) as u32;
			counts[band.min(MINIMAP_BANDS - 1)] += 1;
			total_lines += 1;
			current_len = 0;
		} else {
			current_len += 1;
		}
	}

	let final_band = ((total_lines as usize) * MINIMAP_BANDS) / doc_lines as usize;
	sums[final_band.min(MINIMAP_BANDS - 1)] += normalize_line_density(current_len) as u32;
	counts[final_band.min(MINIMAP_BANDS - 1)] += 1;

	let bands = sums
		.into_iter()
		.zip(counts)
		.map(
			|(sum, count)| {
				if count == 0 { 0 } else { (sum / count) as u8 }
			},
		)
		.collect();

	MinimapSnapshot {
		mode: MinimapMode::TextDensity,
		bands,
		search_bands: vec![0; MINIMAP_BANDS],
		active_search_band: None,
		total_lines: doc_lines,
		viewport_start_line,
		viewport_line_count,
		cursor_line,
	}
}

fn build_byte_fallback_minimap_snapshot(
	registry: &UastRegistry,
	root: NodeId,
	viewport_start_line: DocLine,
	viewport_line_count: u32,
	cursor_line: DocLine,
) -> MinimapSnapshot {
	let file_size = registry.get_total_bytes(root).max(1);
	let doc_lines = registry.get_total_newlines(root).saturating_add(1).max(1);
	let mut bands = vec![0u8; MINIMAP_BANDS];
	let mut visit = registry.get_first_child(root);
	let mut cumulative_bytes = 0u64;

	while let Some(node) = visit {
		let idx = node.index();
		let has_children = unsafe { (*registry.edges[idx].get()).first_child.is_some() };
		if has_children {
			visit = registry.get_first_child(node);
			continue;
		}

		let metrics = unsafe { &*registry.metrics[idx].get() };
		let start_band = ((cumulative_bytes as usize) * MINIMAP_BANDS) / file_size as usize;
		let end_byte = cumulative_bytes.saturating_add(metrics.byte_length as u64);
		let mut end_band = ((end_byte as usize) * MINIMAP_BANDS) / file_size as usize;
		if end_band <= start_band {
			end_band = start_band + 1;
		}
		let avg_line_bytes = if metrics.newlines == 0 {
			metrics.byte_length as usize
		} else {
			(metrics.byte_length as usize) / (metrics.newlines as usize + 1)
		};
		let density = normalize_line_density(avg_line_bytes).max(24);
		for band in start_band.min(MINIMAP_BANDS - 1)..end_band.min(MINIMAP_BANDS) {
			bands[band] = bands[band].max(density);
		}

		cumulative_bytes = end_byte;
		visit = registry.get_next_node_in_walk(node);
	}

	MinimapSnapshot {
		mode: MinimapMode::ByteFallback,
		bands,
		search_bands: vec![0; MINIMAP_BANDS],
		active_search_band: None,
		total_lines: doc_lines,
		viewport_start_line,
		viewport_line_count,
		cursor_line,
	}
}

fn build_search_minimap_bands(
	search_matches: &[SearchMatch],
	active_match_index: Option<usize>,
	total_lines: u32,
) -> (Vec<u8>, Option<usize>) {
	let total_lines = total_lines.max(1);
	let mut bands = vec![0u8; MINIMAP_BANDS];
	if search_matches.is_empty() {
		return (bands, None);
	}

	let stride = (search_matches.len() / 8192).max(1);
	for (idx, m) in search_matches.iter().enumerate().step_by(stride) {
		let band = ((m.line.get() as usize) * MINIMAP_BANDS) / total_lines as usize;
		let band = band.min(MINIMAP_BANDS - 1);
		let intensity = if Some(idx) == active_match_index {
			255
		} else {
			196
		};
		bands[band] = bands[band].max(intensity);
	}

	let active_search_band = active_match_index
		.and_then(|idx| search_matches.get(idx))
		.map(|m| {
			(((m.line.get() as usize) * MINIMAP_BANDS) / total_lines as usize)
				.min(MINIMAP_BANDS - 1)
		});
	(bands, active_search_band)
}

fn insert_text_sparse(
	registry: &UastRegistry,
	root: NodeId,
	byte_offset: DocByte,
	text: &str,
) -> Result<(NodeId, u32), String> {
	let file_size = registry.get_total_bytes(root);
	if byte_offset.get() > file_size {
		return Err("Edit range exceeded document bounds".to_string());
	}
	if text.is_empty() {
		let target = registry.find_node_at_doc_byte(root, byte_offset);
		return Ok((target.node_id, target.node_byte.get()));
	}

	let target = registry.find_node_at_doc_byte(root, byte_offset);
	Ok(registry.insert_text(target.node_id, target.node_byte.get(), text.as_bytes()))
}

fn delete_text_sparse(
	registry: &UastRegistry,
	root: NodeId,
	byte_offset: DocByte,
	expected_deleted: &str,
) -> Result<(NodeId, u32), String> {
	if expected_deleted.is_empty() {
		let target = registry.find_node_at_doc_byte(root, byte_offset);
		return Ok((target.node_id, target.node_byte.get()));
	}

	let delete_len = expected_deleted.len() as u64;
	let delete_end = byte_offset.saturating_add(delete_len);
	let file_size = registry.get_total_bytes(root);
	if delete_end.get() > file_size {
		return Err("Edit range exceeded document bounds".to_string());
	}

	let live_bytes = registry
		.read_loaded_slice(root, byte_offset, delete_end)
		.map_err(|msg| msg.to_string())?;
	if live_bytes != expected_deleted.as_bytes() {
		return Err("Edit range no longer matches live document".to_string());
	}

	let mut target = registry.find_node_at_doc_byte(root, delete_end);
	let mut current_byte =
		registry.doc_byte_for_node_offset(root, target.node_id, target.node_byte);
	while current_byte > byte_offset {
		let previous_byte = current_byte;
		let (new_node, new_offset) =
			registry.delete_backwards(target.node_id, target.node_byte.get());
		target.node_id = new_node;
		target.node_byte = NodeByteOffset::new(new_offset);
		current_byte = registry.doc_byte_for_node_offset(root, new_node, target.node_byte);
		if current_byte >= previous_byte {
			return Err("Sparse delete made no progress".to_string());
		}
	}

	if current_byte != byte_offset {
		return Err("Sparse delete overshot target range".to_string());
	}

	Ok((target.node_id, target.node_byte.get()))
}

fn line_start_byte_sparse(registry: &UastRegistry, root: NodeId, target_line: DocLine) -> DocByte {
	let target = registry.find_node_at_line_col(root, target_line, VisualCol::ZERO);
	registry.doc_byte_for_node_offset(root, target.node_id, target.node_byte)
}

fn line_end_byte_sparse(
	registry: &UastRegistry,
	root: NodeId,
	target_line: DocLine,
	include_newline: bool,
) -> Result<DocByte, String> {
	let line_start_target = registry.find_node_at_line_col(root, target_line, VisualCol::ZERO);
	let mut node = line_start_target.node_id;
	let mut node_offset = line_start_target.node_byte.get() as usize;
	let mut absolute = registry.doc_byte_for_node_offset(
		root,
		line_start_target.node_id,
		line_start_target.node_byte,
	);
	let file_size = registry.get_total_bytes(root);

	loop {
		let text = resolve_loaded_node_text(registry, node)?;
		for &b in &text.as_bytes()[node_offset..] {
			if b == b'\n' {
				return Ok(if include_newline {
					absolute.saturating_add(1)
				} else {
					absolute
				});
			}
			absolute = absolute.saturating_add(1);
		}

		if absolute.get() >= file_size {
			return Ok(DocByte::new(file_size));
		}

		let Some(next) = registry.get_next_sibling(node) else {
			return Ok(DocByte::new(file_size));
		};
		node = next;
		node_offset = 0;
	}
}

fn read_line_bytes_sparse(
	registry: &UastRegistry,
	root: NodeId,
	target_line: DocLine,
	include_newline: bool,
) -> Result<Vec<u8>, String> {
	let start = line_start_byte_sparse(registry, root, target_line);
	let end = line_end_byte_sparse(registry, root, target_line, include_newline)?;
	registry
		.read_loaded_slice(root, start, end)
		.map_err(|msg| msg.to_string())
}

fn linewise_put_insertion_sparse(
	registry: &UastRegistry,
	root: NodeId,
	cursor_line: DocLine,
	text: &str,
) -> Result<(DocByte, String, DocLine), String> {
	let file_size = registry.get_total_bytes(root);
	let insert_offset = line_end_byte_sparse(registry, root, cursor_line, true)?;
	let needs_line_break = if insert_offset.get() == file_size && file_size > 0 {
		let last = registry
			.read_loaded_slice(
				root,
				DocByte::new(file_size.saturating_sub(1)),
				DocByte::new(file_size),
			)
			.map_err(|msg| msg.to_string())?;
		last.first().copied() != Some(b'\n')
	} else {
		false
	};

	let mut inserted_text = String::with_capacity(text.len() + usize::from(needs_line_break));
	if needs_line_break {
		inserted_text.push('\n');
	}
	inserted_text.push_str(text);

	let target_cursor_line = if file_size == 0 {
		DocLine::ZERO
	} else {
		cursor_line + 1
	};

	Ok((insert_offset, inserted_text, target_cursor_line))
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

fn line_col_from_doc_byte_sparse(
	registry: &UastRegistry,
	root: NodeId,
	target_byte: DocByte,
) -> Result<(DocLine, VisualCol), String> {
	let leaf = registry.find_node_at_doc_byte(root, target_byte);
	let mut line = DocLine::ZERO;
	let mut visit = registry.get_first_child(root);

	while let Some(node) = visit {
		if node == leaf.node_id {
			break;
		}
		line = line.saturating_add(unsafe { (*registry.metrics[node.index()].get()).newlines });
		visit = registry.get_next_sibling(node);
	}

	let leaf_text = resolve_loaded_node_text(registry, leaf.node_id)?;
	let local_offset = leaf.node_byte.get() as usize;
	let local_prefix = &leaf_text.as_bytes()[..local_offset.min(leaf_text.len())];
	line = line.saturating_add(local_prefix.iter().filter(|&&b| b == b'\n').count() as u32);

	let mut line_bytes = if let Some(last_newline) = local_prefix.iter().rposition(|&b| b == b'\n')
	{
		local_prefix[last_newline + 1..].to_vec()
	} else {
		local_prefix.to_vec()
	};

	if local_prefix.iter().rposition(|&b| b == b'\n').is_none() {
		let mut prev = registry.get_prev_sibling(leaf.node_id);
		while let Some(node) = prev {
			let text = resolve_loaded_node_text(registry, node)?;
			let bytes = text.as_bytes();
			if let Some(last_newline) = bytes.iter().rposition(|&b| b == b'\n') {
				let mut prefix = bytes[last_newline + 1..].to_vec();
				prefix.extend_from_slice(&line_bytes);
				line_bytes = prefix;
				break;
			}

			let mut prefix = bytes.to_vec();
			prefix.extend_from_slice(&line_bytes);
			line_bytes = prefix;
			prev = registry.get_prev_sibling(node);
		}
	}

	let mut col = VisualCol::ZERO;
	for &b in &line_bytes {
		advance_visual_col_only(&mut col, b);
	}

	Ok((line, col))
}

struct ParseWindow {
	text: String,
	global_start_byte: DocByte,
	cursor_local_byte: u32,
}

fn is_structural_word_token(kind: SyntaxKind) -> bool {
	kind == SyntaxKind::IDENT || kind.is_keyword(Edition::Edition2024) || kind.is_literal()
}

fn seek_structural_token(mut token: SyntaxToken, direction: Direction) -> Option<SyntaxToken> {
	loop {
		if is_structural_word_token(token.kind()) {
			return Some(token);
		}

		if token.kind() == SyntaxKind::WHITESPACE || token.kind().is_punct() {
			token = match direction {
				Direction::Next => token.next_token()?,
				Direction::Prev => token.prev_token()?,
			};
			continue;
		}

		return None;
	}
}

fn select_structural_token_at_offset(
	syntax: &ra_ap_syntax::SyntaxNode,
	offset: TextSize,
) -> Option<SyntaxToken> {
	let left = syntax.token_at_offset(offset).left_biased();
	let right = syntax.token_at_offset(offset).right_biased();

	match (left, right) {
		(None, None) => None,
		(Some(token), None) | (None, Some(token)) => {
			if is_structural_word_token(token.kind()) {
				Some(token)
			} else {
				seek_structural_token(token.clone(), Direction::Next)
					.or_else(|| seek_structural_token(token, Direction::Prev))
			}
		}
		(Some(left), Some(right)) if left == right => {
			if is_structural_word_token(left.kind()) {
				Some(left)
			} else {
				seek_structural_token(left.clone(), Direction::Next)
					.or_else(|| seek_structural_token(left, Direction::Prev))
			}
		}
		(Some(left), Some(right)) => {
			if is_structural_word_token(right.kind()) {
				return Some(right);
			}

			if right.kind() == SyntaxKind::WHITESPACE {
				if let Some(found) = seek_structural_token(right.clone(), Direction::Next) {
					return Some(found);
				}
			}

			if is_structural_word_token(left.kind()) {
				return Some(left);
			}

			if right.kind().is_punct() {
				if let Some(found) = seek_structural_token(right, Direction::Next) {
					return Some(found);
				}
			}

			seek_structural_token(left, Direction::Prev)
		}
	}
}

fn resolve_loaded_node_text(registry: &UastRegistry, node: NodeId) -> Result<String, String> {
	let text = registry.resolve_physical_bytes(node);
	let byte_len = unsafe { (*registry.metrics[node.index()].get()).byte_length as usize };
	if text.is_empty() && byte_len > 0 {
		return Err("File still loading, cannot resolve structural token".to_string());
	}
	Ok(text)
}

fn build_parse_window_around_leaf(
	registry: &UastRegistry,
	leaf: NodeByteTarget,
) -> Result<ParseWindow, String> {
	let mut text = String::new();
	let mut global_start_byte = leaf.node_start_byte;
	let mut cursor_local_byte = leaf.node_byte.get() as usize;

	if let Some(prev) = registry.get_prev_sibling(leaf.node_id) {
		let prev_text = resolve_loaded_node_text(registry, prev)?;
		let prev_len = unsafe { (*registry.metrics[prev.index()].get()).byte_length as u64 };
		cursor_local_byte += prev_text.len();
		global_start_byte = global_start_byte.saturating_sub(prev_len);
		text.push_str(&prev_text);
	}

	let current_text = resolve_loaded_node_text(registry, leaf.node_id)?;
	text.push_str(&current_text);

	if let Some(next) = registry.get_next_sibling(leaf.node_id) {
		let next_text = resolve_loaded_node_text(registry, next)?;
		text.push_str(&next_text);
	}

	Ok(ParseWindow {
		text,
		global_start_byte,
		cursor_local_byte: cursor_local_byte as u32,
	})
}

fn delete_char_delta_at_cursor(
	registry: &UastRegistry,
	root: NodeId,
	cursor_abs_byte_offset: DocByte,
) -> Result<Option<TextDelta>, String> {
	let file_size = registry.get_total_bytes(root);
	if cursor_abs_byte_offset.get() >= file_size {
		return Ok(None);
	}

	let preview_end = DocByte::new((cursor_abs_byte_offset.get() + 4).min(file_size));
	let preview = registry
		.read_loaded_slice(root, cursor_abs_byte_offset, preview_end)
		.map_err(|msg| msg.to_string())?;
	let delete_len = (1..=preview.len())
		.find(|&len| {
			std::str::from_utf8(&preview[..len])
				.map(|text| text.chars().count() == 1)
				.unwrap_or(false)
		})
		.ok_or_else(|| "Delete target is not valid UTF-8".to_string())?;
	let deleted_text = std::str::from_utf8(&preview[..delete_len])
		.expect("validated UTF-8 prefix")
		.to_string();

	Ok(Some(TextDelta {
		global_byte_offset: cursor_abs_byte_offset,
		deleted_text,
		inserted_text: String::new(),
		state_before: StateId::ZERO,
		state_after: StateId::ZERO,
	}))
}

fn word_object_delta_at_cursor(
	registry: &UastRegistry,
	root: NodeId,
	cursor_abs_byte_offset: DocByte,
) -> Result<Option<TextDelta>, String> {
	let leaf = registry.find_node_at_doc_byte(root, cursor_abs_byte_offset);
	let parse_window = build_parse_window_around_leaf(registry, leaf)?;
	let parse = SourceFile::parse(&parse_window.text, Edition::Edition2024);
	let syntax = parse.tree().syntax().clone();
	let offset = TextSize::from(parse_window.cursor_local_byte);

	let Some(token) = select_structural_token_at_offset(&syntax, offset) else {
		return Ok(None);
	};

	let range = token.text_range();
	let start = parse_window
		.global_start_byte
		.saturating_add(u64::from(u32::from(range.start())));
	let end = parse_window
		.global_start_byte
		.saturating_add(u64::from(u32::from(range.end())));

	if end <= start {
		return Ok(None);
	}

	Ok(Some(TextDelta {
		global_byte_offset: start,
		deleted_text: token.text().to_string(),
		inserted_text: String::new(),
		state_before: StateId::ZERO,
		state_after: StateId::ZERO,
	}))
}

#[cfg(test)]
fn delete_to_line_end_delta(
	doc: &[u8],
	cursor_line: DocLine,
	cursor_col: VisualCol,
) -> Option<TextDelta> {
	let cursor_abs_byte_offset = byte_offset_from_line_col(doc, cursor_line, cursor_col);
	let delete_start = cursor_abs_byte_offset.get() as usize;
	let (line_start, line_end_with_newline) = line_byte_range(doc, cursor_line, cursor_line);
	let line_end = if line_end_with_newline > line_start && doc[line_end_with_newline - 1] == b'\n'
	{
		line_end_with_newline - 1
	} else {
		line_end_with_newline
	};

	if delete_start >= line_end {
		return None;
	}

	Some(TextDelta {
		global_byte_offset: cursor_abs_byte_offset,
		deleted_text: String::from_utf8_lossy(&doc[delete_start..line_end]).into_owned(),
		inserted_text: String::new(),
		state_before: StateId::ZERO,
		state_after: StateId::ZERO,
	})
}

fn clamp_visual_byte(byte: DocByte, buffer: &[u8]) -> Option<DocByte> {
	if buffer.is_empty() {
		None
	} else {
		Some(DocByte::new(
			byte.get().min(buffer.len().saturating_sub(1) as u64),
		))
	}
}

fn clamp_existing_doc_byte(byte: DocByte, file_size: u64) -> Option<DocByte> {
	if file_size == 0 {
		None
	} else {
		Some(DocByte::new(byte.get().min(file_size.saturating_sub(1))))
	}
}

pub fn resolve_visual_ranges(
	anchor: DocByte,
	cursor: DocByte,
	kind: VisualKind,
	buffer: &[u8],
) -> Vec<(DocByte, DocByte)> {
	let Some(anchor) = clamp_visual_byte(anchor, buffer) else {
		return Vec::new();
	};
	let Some(cursor) = clamp_visual_byte(cursor, buffer) else {
		return Vec::new();
	};

	let start = DocByte::new(anchor.get().min(cursor.get()));
	let end = DocByte::new(anchor.get().max(cursor.get()));

	match kind {
		VisualKind::Char => vec![(start, end)],
		VisualKind::Line => {
			let mut expanded_start = start.get() as usize;
			while expanded_start > 0 && buffer[expanded_start - 1] != b'\n' {
				expanded_start -= 1;
			}

			let mut expanded_end = end.get() as usize;
			while expanded_end + 1 < buffer.len() && buffer[expanded_end] != b'\n' {
				expanded_end += 1;
			}

			vec![(
				DocByte::new(expanded_start as u64),
				DocByte::new(expanded_end as u64),
			)]
		}
		VisualKind::Block => {
			let (anchor_line, anchor_col) = line_col_from_byte_offset(buffer, anchor);
			let (cursor_line, cursor_col) = line_col_from_byte_offset(buffer, cursor);
			let min_line = anchor_line.get().min(cursor_line.get());
			let max_line = anchor_line.get().max(cursor_line.get());
			let min_col = VisualCol::new(anchor_col.get().min(cursor_col.get()));
			let max_col = VisualCol::new(anchor_col.get().max(cursor_col.get()));
			let mut ranges = Vec::new();

			for line in min_line..=max_line {
				let line = DocLine::new(line);
				let (line_start, line_end_with_break) = line_byte_range(buffer, line, line);
				let line_content_end = if line_end_with_break > line_start
					&& buffer[line_end_with_break - 1] == b'\n'
				{
					line_end_with_break - 1
				} else {
					line_end_with_break
				};

				if line_content_end <= line_start {
					continue;
				}

				let start_byte = byte_offset_from_line_col(buffer, line, min_col).get() as usize;
				if start_byte >= line_content_end {
					continue;
				}

				let raw_end_byte = byte_offset_from_line_col(buffer, line, max_col).get() as usize;
				let end_byte = if raw_end_byte >= line_content_end {
					line_content_end - 1
				} else {
					raw_end_byte
				};

				if start_byte <= end_byte {
					ranges.push((
						DocByte::new(start_byte as u64),
						DocByte::new(end_byte as u64),
					));
				}
			}

			ranges
		}
	}
}

fn resolve_visual_ranges_sparse(
	registry: &UastRegistry,
	root: NodeId,
	anchor: DocByte,
	cursor: DocByte,
	kind: VisualKind,
) -> Result<Vec<(DocByte, DocByte)>, String> {
	let file_size = registry.get_total_bytes(root);
	let Some(anchor) = clamp_existing_doc_byte(anchor, file_size) else {
		return Ok(Vec::new());
	};
	let Some(cursor) = clamp_existing_doc_byte(cursor, file_size) else {
		return Ok(Vec::new());
	};

	let start = DocByte::new(anchor.get().min(cursor.get()));
	let end = DocByte::new(anchor.get().max(cursor.get()));

	match kind {
		VisualKind::Char => Ok(vec![(start, end)]),
		VisualKind::Line => {
			let (start_line, _) = line_col_from_doc_byte_sparse(registry, root, start)?;
			let (end_line, _) = line_col_from_doc_byte_sparse(registry, root, end)?;
			let expanded_start = line_start_byte_sparse(registry, root, start_line);
			let expanded_end_exclusive = line_end_byte_sparse(registry, root, end_line, true)?;
			if expanded_end_exclusive <= expanded_start {
				Ok(Vec::new())
			} else {
				Ok(vec![(
					expanded_start,
					expanded_end_exclusive.saturating_sub(1),
				)])
			}
		}
		VisualKind::Block => {
			let (anchor_line, anchor_col) = line_col_from_doc_byte_sparse(registry, root, anchor)?;
			let (cursor_line, cursor_col) = line_col_from_doc_byte_sparse(registry, root, cursor)?;
			let min_line = anchor_line.get().min(cursor_line.get());
			let max_line = anchor_line.get().max(cursor_line.get());
			let min_col = VisualCol::new(anchor_col.get().min(cursor_col.get()));
			let max_col = VisualCol::new(anchor_col.get().max(cursor_col.get()));
			let mut ranges = Vec::new();

			for line in min_line..=max_line {
				let line = DocLine::new(line);
				let start_target = registry.find_node_at_line_col(root, line, min_col);
				let start_byte = registry.doc_byte_for_node_offset(
					root,
					start_target.node_id,
					start_target.node_byte,
				);
				let line_content_end = line_end_byte_sparse(registry, root, line, false)?;
				if start_byte >= line_content_end {
					continue;
				}

				let end_target = registry.find_node_at_line_col(root, line, max_col);
				let raw_end = registry.doc_byte_for_node_offset(
					root,
					end_target.node_id,
					end_target.node_byte,
				);
				let end_byte = if raw_end >= line_content_end {
					line_content_end.saturating_sub(1)
				} else {
					raw_end
				};

				if start_byte <= end_byte {
					ranges.push((start_byte, end_byte));
				}
			}

			Ok(ranges)
		}
	}
}

fn extract_visual_text_sparse(
	registry: &UastRegistry,
	root: NodeId,
	ranges: &[(DocByte, DocByte)],
) -> Result<String, String> {
	let mut text = String::new();

	for (idx, (start, end)) in ranges.iter().enumerate() {
		let bytes = registry
			.read_loaded_slice(root, *start, end.saturating_add(1))
			.map_err(|msg| msg.to_string())?;
		if bytes.is_empty() {
			continue;
		}

		if idx > 0 && !text.ends_with('\n') {
			text.push('\n');
		}

		text.push_str(&String::from_utf8_lossy(&bytes));
	}

	Ok(text)
}

fn apply_deltas_to_document_internal(
	registry: &UastRegistry,
	root_id: &mut Option<NodeId>,
	cursor_node: &mut NodeId,
	cursor_offset: &mut u32,
	cursor_abs_line: &mut DocLine,
	cursor_abs_col: &mut VisualCol,
	ledger: &mut UndoLedger,
	semantic_highlights: &mut Vec<HighlightSpan>,
	deltas: Vec<TextDelta>,
	record_undo: bool,
	cursor_byte_override: Option<DocByte>,
) -> Result<(), String> {
	if deltas.is_empty() {
		return Ok(());
	}

	let rid = (*root_id).ok_or_else(|| "No file loaded".to_string())?;

	for delta in &deltas {
		if !delta.deleted_text.is_empty() {
			delete_text_sparse(registry, rid, delta.global_byte_offset, &delta.deleted_text)?;
		}
		if !delta.inserted_text.is_empty() {
			insert_text_sparse(
				registry,
				rid,
				delta.global_byte_offset,
				&delta.inserted_text,
			)?;
		}
	}

	let last_delta = deltas.last().expect("non-empty delta group");
	let file_size = registry.get_total_bytes(rid);
	let cursor_byte_after_edit = cursor_byte_override.unwrap_or_else(|| {
		last_delta
			.global_byte_offset
			.saturating_add(last_delta.inserted_text.len() as u64)
	});
	let cursor_byte_after_edit = DocByte::new(cursor_byte_after_edit.get().min(file_size));

	rebase_semantic_highlights_after_deltas(semantic_highlights, &deltas);
	if record_undo {
		ledger.push_group(deltas);
	}
	*root_id = Some(rid);

	let (line, col) = line_col_from_doc_byte_sparse(registry, rid, cursor_byte_after_edit)?;
	*cursor_abs_line = line;
	let target = registry.find_node_at_line_col(rid, line, col);
	apply_cursor_target(target, cursor_node, cursor_offset, cursor_abs_col);

	Ok(())
}

fn apply_deltas_to_document(
	registry: &UastRegistry,
	root_id: &mut Option<NodeId>,
	cursor_node: &mut NodeId,
	cursor_offset: &mut u32,
	cursor_abs_line: &mut DocLine,
	cursor_abs_col: &mut VisualCol,
	ledger: &mut UndoLedger,
	semantic_highlights: &mut Vec<HighlightSpan>,
	deltas: Vec<TextDelta>,
) -> Result<(), String> {
	apply_deltas_to_document_internal(
		registry,
		root_id,
		cursor_node,
		cursor_offset,
		cursor_abs_line,
		cursor_abs_col,
		ledger,
		semantic_highlights,
		deltas,
		true,
		None,
	)
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
		let mut last_semantic_state: Option<StateId> = None;
		let mut last_semantic_len: usize = usize::MAX;
		let mut last_semantic_path: Option<String> = None;
		let mut next_semantic_request_id = RequestId::new(1);
		let mut pending_semantic_request_id: Option<RequestId> = None;
		let mut cached_minimap: Option<MinimapSnapshot> = None;
		let mut last_minimap_state: Option<StateId> = None;
		let mut last_minimap_total_lines: u32 = u32::MAX;
		let mut last_minimap_path: Option<String> = None;

		let mut cursor_abs_line = DocLine::ZERO;
		let mut cursor_abs_col = VisualCol::ZERO;
		let mut scroll_y = 0u32;
		let mut viewport_lines = 50;
		let mut current_theme =
			Theme::try_new("onedark").unwrap_or_else(|_| Theme::try_new("viridis").unwrap());
		let mut root_id: Option<NodeId> = None;
		let mut file_path: Option<String> = None;

		let mut status_message: Option<String> = None;
		let mut pending_quit = false;
		let mut mode_override: Option<EditorMode> = None;
		let mut active_visual: Option<(DocByte, VisualKind)> = None;

		// Search state
		let mut search_pattern: Option<String> = None;
		let mut search_case_insensitive = false;
		let mut search_matches: Vec<SearchMatch> = Vec::new();
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
								if let Ok(line_bytes) =
									read_line_bytes_sparse(&registry, rid, cursor_abs_line, false)
								{
									cursor_abs_col = step_left_visual_col(
										&line_bytes,
										DocLine::ZERO,
										cursor_abs_col,
									);
								} else {
									cursor_abs_col = cursor_abs_col.saturating_sub(1);
								}
							}
							MoveDirection::Right => {
								if let Ok(line_bytes) =
									read_line_bytes_sparse(&registry, rid, cursor_abs_line, false)
								{
									cursor_abs_col = step_right_visual_col(
										&line_bytes,
										DocLine::ZERO,
										cursor_abs_col,
									);
								} else {
									cursor_abs_col += 1;
								}
							}
							MoveDirection::NextWord
							| MoveDirection::PrevWord
							| MoveDirection::NextWordEnd => {
								if let Ok(bytes) = read_loaded_document(&registry, rid) {
									if let Ok(doc) = std::str::from_utf8(&bytes) {
										let cursor_abs_byte = registry
											.doc_byte_for_node_offset(
												rid,
												cursor_node,
												NodeByteOffset::new(cursor_offset),
											)
											.get() as usize;
										let target_byte = match dir {
											MoveDirection::NextWord => {
												next_word_start(doc, cursor_abs_byte)
											}
											MoveDirection::PrevWord => {
												prev_word_start(doc, cursor_abs_byte)
											}
											MoveDirection::NextWordEnd => {
												next_word_end(doc, cursor_abs_byte)
											}
											_ => unreachable!(),
										};
										let (line, col) = line_col_from_byte_offset(
											&bytes,
											DocByte::new(target_byte as u64),
										);
										cursor_abs_line = line;
										cursor_abs_col = col;
									}
								}
							}
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
						cursor_abs_line = DocLine::new(target.get().min(total));
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
				line_motion @ (EditorCommand::LineStart
				| EditorCommand::FirstNonWhitespace
				| EditorCommand::LineEnd
				| EditorCommand::SmartHome) => {
					if let Some(rid) = root_id {
						if let Ok(line_bytes) =
							read_line_bytes_sparse(&registry, rid, cursor_abs_line, false)
						{
							let target_col = match line_motion {
								EditorCommand::LineStart => VisualCol::ZERO,
								EditorCommand::FirstNonWhitespace => {
									first_non_whitespace_visual_col(&line_bytes, DocLine::ZERO)
								}
								EditorCommand::LineEnd => {
									line_end_visual_col(&line_bytes, DocLine::ZERO)
								}
								EditorCommand::SmartHome => smart_home_visual_col(
									&line_bytes,
									DocLine::ZERO,
									cursor_abs_col,
								),
								_ => unreachable!(),
							};
							let target =
								registry.find_node_at_line_col(rid, cursor_abs_line, target_col);
							apply_cursor_target(
								target,
								&mut cursor_node,
								&mut cursor_offset,
								&mut cursor_abs_col,
							);
							needs_render = true;
						}
					}
				}
				EditorCommand::PageUp => {
					if let Some(rid) = root_id {
						cursor_abs_line =
							cursor_abs_line.saturating_sub(page_motion_step(viewport_lines));
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
				EditorCommand::PageDown => {
					if let Some(rid) = root_id {
						let total = registry.get_total_newlines(rid);
						let target_line =
							cursor_abs_line.saturating_add(page_motion_step(viewport_lines));
						cursor_abs_line = DocLine::new(target_line.get().min(total));
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
				EditorCommand::SetVisualSelection { anchor, kind } => {
					active_visual = Some((anchor, kind));
					needs_render = true;
				}
				EditorCommand::ClearVisualSelection => {
					active_visual = None;
					needs_render = true;
				}
				EditorCommand::VisualYank { anchor, kind } => {
					if let Some(rid) = root_id {
						let cursor_abs_byte = registry.doc_byte_for_node_offset(
							rid,
							cursor_node,
							NodeByteOffset::new(cursor_offset),
						);
						match resolve_visual_ranges_sparse(
							&registry,
							rid,
							anchor,
							cursor_abs_byte,
							kind,
						) {
							Ok(ranges) => {
								if ranges.is_empty() {
									status_message = Some("Nothing selected".to_string());
								} else {
									match extract_visual_text_sparse(&registry, rid, &ranges) {
										Ok(selected_text) => {
											registers.insert('"', selected_text.clone());
											clipboard.set_text(&selected_text);
											status_message = Some(format!(
												"{} bytes yanked",
												selected_text.len()
											));
										}
										Err(msg) => status_message = Some(msg),
									}
								}
								active_visual = None;
								needs_render = true;
							}
							Err(msg) => {
								status_message = Some(msg.to_string());
								active_visual = None;
								needs_render = true;
							}
						}
					}
				}
				visual_delete_cmd @ (EditorCommand::VisualDelete { anchor, kind }
				| EditorCommand::VisualChange { anchor, kind }) => {
					if let Some(rid) = root_id {
						let cursor_abs_byte = registry.doc_byte_for_node_offset(
							rid,
							cursor_node,
							NodeByteOffset::new(cursor_offset),
						);
						match resolve_visual_ranges_sparse(
							&registry,
							rid,
							anchor,
							cursor_abs_byte,
							kind,
						) {
							Ok(ranges) => {
								if ranges.is_empty() {
									status_message = Some("Nothing selected".to_string());
								} else {
									match extract_visual_text_sparse(&registry, rid, &ranges) {
										Ok(selected_text) => {
											registers.insert('"', selected_text);
										}
										Err(msg) => {
											status_message = Some(msg);
											active_visual = None;
											continue;
										}
									}
									let mut deltas = Vec::with_capacity(ranges.len());
									for (start, end) in ranges.iter().rev() {
										let deleted_bytes = match registry.read_loaded_slice(
											rid,
											*start,
											end.saturating_add(1),
										) {
											Ok(bytes) => bytes,
											Err(msg) => {
												status_message = Some(msg.to_string());
												deltas.clear();
												break;
											}
										};
										if deleted_bytes.is_empty() {
											continue;
										}

										let delta = TextDelta {
											global_byte_offset: *start,
											deleted_text: String::from_utf8_lossy(&deleted_bytes)
												.into_owned(),
											inserted_text: String::new(),
											state_before: StateId::ZERO,
											state_after: StateId::ZERO,
										};
										deltas.push(delta);
									}

									if !deltas.is_empty() {
										match apply_deltas_to_document(
											&registry,
											&mut root_id,
											&mut cursor_node,
											&mut cursor_offset,
											&mut cursor_abs_line,
											&mut cursor_abs_col,
											&mut ledger,
											&mut semantic_highlights,
											deltas,
										) {
											Ok(()) => {
												if matches!(
													visual_delete_cmd,
													EditorCommand::VisualChange { .. }
												) {
													mode_override = Some(EditorMode::Insert);
												}
											}
											Err(msg) => status_message = Some(msg),
										}
									}
								}
								active_visual = None;
								needs_render = true;
							}
							Err(msg) => {
								status_message = Some(msg.to_string());
								active_visual = None;
								needs_render = true;
							}
						}
					}
				}
				word_cmd @ (EditorCommand::DeleteInnerWord | EditorCommand::ChangeInnerWord) => {
					if let Some(rid) = root_id {
						let cursor_abs_byte_offset = registry.doc_byte_for_node_offset(
							rid,
							cursor_node,
							NodeByteOffset::new(cursor_offset),
						);
						match word_object_delta_at_cursor(&registry, rid, cursor_abs_byte_offset) {
							Ok(Some(delta)) => {
								match apply_deltas_to_document(
									&registry,
									&mut root_id,
									&mut cursor_node,
									&mut cursor_offset,
									&mut cursor_abs_line,
									&mut cursor_abs_col,
									&mut ledger,
									&mut semantic_highlights,
									vec![delta],
								) {
									Ok(()) => {
										if matches!(word_cmd, EditorCommand::ChangeInnerWord) {
											mode_override = Some(EditorMode::Insert);
										}
									}
									Err(msg) => status_message = Some(msg),
								}
							}
							Ok(None) => {
								status_message = Some("No structural word at cursor".to_string())
							}
							Err(msg) => status_message = Some(msg),
						}
						needs_render = true;
					}
				}
				EditorCommand::DeleteToLineEnd => {
					if let Some(rid) = root_id {
						let cursor_abs_byte = registry.doc_byte_for_node_offset(
							rid,
							cursor_node,
							NodeByteOffset::new(cursor_offset),
						);
						match line_end_byte_sparse(&registry, rid, cursor_abs_line, false) {
							Ok(line_end) => {
								if cursor_abs_byte < line_end {
									let deleted_bytes = match registry.read_loaded_slice(
										rid,
										cursor_abs_byte,
										line_end,
									) {
										Ok(bytes) => bytes,
										Err(msg) => {
											status_message = Some(msg.to_string());
											continue;
										}
									};
									let delta = TextDelta {
										global_byte_offset: cursor_abs_byte,
										deleted_text: String::from_utf8_lossy(&deleted_bytes)
											.into_owned(),
										inserted_text: String::new(),
										state_before: StateId::ZERO,
										state_after: StateId::ZERO,
									};
									if let Err(msg) = apply_deltas_to_document(
										&registry,
										&mut root_id,
										&mut cursor_node,
										&mut cursor_offset,
										&mut cursor_abs_line,
										&mut cursor_abs_col,
										&mut ledger,
										&mut semantic_highlights,
										vec![delta],
									) {
										status_message = Some(msg);
									}
								} else {
									status_message = Some("Already at end of line".to_string());
								}
								needs_render = true;
							}
							Err(msg) => {
								status_message = Some(msg.to_string());
								needs_render = true;
							}
						}
					}
				}
				EditorCommand::InsertChar(c) => {
					if let Some(rid) = root_id {
						let global_offset = registry.doc_byte_for_node_offset(
							rid,
							cursor_node,
							NodeByteOffset::new(cursor_offset),
						);
						let mut buf = [0; 4];
						let s = c.encode_utf8(&mut buf);
						let delta = TextDelta {
							global_byte_offset: global_offset,
							deleted_text: String::new(),
							inserted_text: s.to_string(),
							state_before: StateId::ZERO,
							state_after: StateId::ZERO,
						};
						if let Err(msg) = apply_deltas_to_document(
							&registry,
							&mut root_id,
							&mut cursor_node,
							&mut cursor_offset,
							&mut cursor_abs_line,
							&mut cursor_abs_col,
							&mut ledger,
							&mut semantic_highlights,
							vec![delta],
						) {
							status_message = Some(msg);
						}
						needs_render = true;
					}
				}
				EditorCommand::Backspace => {
					if let Some(rid) = root_id {
						let (delete_node, delete_offset) = if cursor_offset == 0 {
							match registry.get_prev_sibling(cursor_node) {
								Some(prev) => {
									let prev_len = unsafe {
										(*registry.metrics[prev.index()].get()).byte_length
									};
									(prev, prev_len)
								}
								None => continue,
							}
						} else {
							(cursor_node, cursor_offset)
						};

						match resolve_loaded_node_text(&registry, delete_node) {
							Ok(text) => {
								let bytes = text.as_bytes();
								let delete_end = delete_offset as usize;
								if delete_end == 0 || delete_end > bytes.len() {
									continue;
								}

								let mut delete_start = delete_end - 1;
								while delete_start > 0 && (bytes[delete_start] & 0xC0) == 0x80 {
									delete_start -= 1;
								}

								let deleted_text =
									String::from_utf8_lossy(&bytes[delete_start..delete_end])
										.into_owned();
								let delete_start_byte = registry.doc_byte_for_node_offset(
									rid,
									delete_node,
									NodeByteOffset::new(delete_start as u32),
								);
								let delta = TextDelta {
									global_byte_offset: delete_start_byte,
									deleted_text,
									inserted_text: String::new(),
									state_before: StateId::ZERO,
									state_after: StateId::ZERO,
								};
								if let Err(msg) = apply_deltas_to_document(
									&registry,
									&mut root_id,
									&mut cursor_node,
									&mut cursor_offset,
									&mut cursor_abs_line,
									&mut cursor_abs_col,
									&mut ledger,
									&mut semantic_highlights,
									vec![delta],
								) {
									status_message = Some(msg);
								}
							}
							Err(msg) => status_message = Some(msg),
						}
						needs_render = true;
					}
				}
				EditorCommand::Delete => {
					if let Some(rid) = root_id {
						let cursor_abs_byte = registry.doc_byte_for_node_offset(
							rid,
							cursor_node,
							NodeByteOffset::new(cursor_offset),
						);
						match delete_char_delta_at_cursor(&registry, rid, cursor_abs_byte) {
							Ok(Some(delta)) => {
								if let Err(msg) = apply_deltas_to_document(
									&registry,
									&mut root_id,
									&mut cursor_node,
									&mut cursor_offset,
									&mut cursor_abs_line,
									&mut cursor_abs_col,
									&mut ledger,
									&mut semantic_highlights,
									vec![delta],
								) {
									status_message = Some(msg);
								}
							}
							Ok(None) => {}
							Err(msg) => status_message = Some(msg),
						}
						needs_render = true;
					}
				}
				EditorCommand::Resize(_w, h) => {
					viewport_lines = h.saturating_sub(1) as u32;
					needs_render = true;
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
				EditorCommand::ScrollViewport(delta) => {
					if let Some(rid) = root_id {
						let total = registry.get_total_newlines(rid);
						scroll_y = scroll_viewport(scroll_y, delta, total, viewport_lines);
						let clamped_cursor_line = clamp_cursor_line_to_viewport(
							cursor_abs_line,
							scroll_y,
							total,
							viewport_lines,
						);
						if clamped_cursor_line != cursor_abs_line {
							cursor_abs_line = clamped_cursor_line;
							let target = registry.find_node_at_line_col(
								rid,
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
					needs_render = true;
				}
				EditorCommand::ClickCursor(target_pos) => {
					if let Some(rid) = root_id {
						let total = registry.get_total_newlines(rid);
						cursor_abs_line = DocLine::new(target_pos.line.get().min(total));
						cursor_abs_col = target_pos.col;
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
						scroll_y = 0;
						ledger.clear();
						active_visual = None;
						semantic_highlights.clear();
						last_semantic_state = None;
						last_semantic_len = usize::MAX;
						last_semantic_path = None;
						pending_semantic_request_id = None;
						cached_minimap = None;
						last_minimap_state = None;
						last_minimap_total_lines = u32::MAX;
						last_minimap_path = None;
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
						scroll_y = 0;
						ledger.clear();
						active_visual = None;
						semantic_highlights.clear();
						last_semantic_state = None;
						last_semantic_len = usize::MAX;
						last_semantic_path = None;
						pending_semantic_request_id = None;
						cached_minimap = None;
						last_minimap_state = None;
						last_minimap_total_lines = u32::MAX;
						last_minimap_path = None;
						needs_render = true;
					}
				}
				EditorCommand::WriteFile => {
					if let Some(rid) = root_id {
						if let Some(ref path) = file_path {
							match save_document_atomic(
								&registry,
								rid,
								Some(Path::new(path)),
								Path::new(path),
							) {
								Ok(len) => {
									resolver.register_device(FILE_DEVICE_ID, path);
									rebind_document_spans_to_saved_file(
										&registry,
										rid,
										FILE_DEVICE_ID,
									);
									ledger.mark_saved();
									status_message = Some(format!("\"{}\" {}B written", path, len));
								}
								Err(msg) => status_message = Some(msg),
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
						let source_path = file_path.as_deref().map(Path::new);
						match save_document_atomic(&registry, rid, source_path, Path::new(&path)) {
							Ok(len) => {
								resolver.register_device(FILE_DEVICE_ID, &path);
								rebind_document_spans_to_saved_file(&registry, rid, FILE_DEVICE_ID);
								ledger.mark_saved();
								status_message = Some(format!("\"{}\" {}B written", path, len));
								file_path = Some(path);
							}
							Err(msg) => status_message = Some(msg),
						}
					} else {
						status_message = Some("No file to write".to_string());
					}
					needs_render = true;
				}
				EditorCommand::WriteAndQuit => {
					if let Some(rid) = root_id {
						if let Some(ref path) = file_path {
							match save_document_atomic(
								&registry,
								rid,
								Some(Path::new(path)),
								Path::new(path),
							) {
								Ok(len) => {
									resolver.register_device(FILE_DEVICE_ID, path);
									rebind_document_spans_to_saved_file(
										&registry,
										rid,
										FILE_DEVICE_ID,
									);
									ledger.mark_saved();
									status_message = Some(format!("\"{}\" {}B written", path, len));
									pending_quit = true;
								}
								Err(msg) => status_message = Some(msg),
							}
						} else {
							status_message = Some("No file name".to_string());
						}
					} else {
						status_message = Some("No file to write".to_string());
					}
					needs_render = true;
				}
				EditorCommand::SetTheme(name) => {
					match Theme::try_new(&name) {
						Ok(theme) => {
							current_theme = theme;
							status_message = Some(format!("Theme set to {}", name));
						}
						Err(e) => {
							status_message = Some(e);
						}
					}
					needs_render = true;
				}
				EditorCommand::SearchStart(pattern) => {
					if let Some(rid) = root_id {
						match build_regex(&pattern, false) {
							Ok(re) => match read_loaded_document(&registry, rid) {
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
											.position(|m| {
												m.line > cursor_abs_line
													|| (m.line == cursor_abs_line
														&& m.col >= cursor_abs_col)
											})
											.unwrap_or(0);
										search_match_index = Some(idx);
										let current_match = matches[idx];
										cursor_abs_line = current_match.line;
										let target = registry.find_node_at_line_col(
											rid,
											current_match.line,
											current_match.col,
										);
										apply_cursor_target(
											target,
											&mut cursor_node,
											&mut cursor_offset,
											&mut cursor_abs_col,
										);
										cursor_abs_col = current_match.col;
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
							let current_match = search_matches[idx];
							cursor_abs_line = current_match.line;
							let target = registry.find_node_at_line_col(
								rid,
								current_match.line,
								current_match.col,
							);
							apply_cursor_target(
								target,
								&mut cursor_node,
								&mut cursor_offset,
								&mut cursor_abs_col,
							);
							cursor_abs_col = current_match.col;
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
							let current_match = search_matches[idx];
							cursor_abs_line = current_match.line;
							let target = registry.find_node_at_line_col(
								rid,
								current_match.line,
								current_match.col,
							);
							apply_cursor_target(
								target,
								&mut cursor_node,
								&mut cursor_offset,
								&mut cursor_abs_col,
							);
							cursor_abs_col = current_match.col;
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
							Ok(re) => match read_loaded_document(&registry, rid) {
								Ok(bytes) => {
									let byte_range =
										resolve_byte_range(&range, &bytes, cursor_abs_line);
									let deltas = substitute_text_deltas(
										&bytes,
										&re,
										&replacement,
										byte_range,
									);
									let count = deltas.len() as u32;
									if count == 0 {
										status_message = Some("Pattern not found".to_string());
									} else {
										match apply_deltas_to_document(
											&registry,
											&mut root_id,
											&mut cursor_node,
											&mut cursor_offset,
											&mut cursor_abs_line,
											&mut cursor_abs_col,
											&mut ledger,
											&mut semantic_highlights,
											deltas,
										) {
											Ok(()) => {
												search_matches.clear();
												search_match_index = None;
												search_match_info = None;
												status_message =
													Some(format!("{} substitution(s)", count));
											}
											Err(msg) => status_message = Some(msg),
										}
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
							Ok(re) => match read_loaded_document(&registry, rid) {
								Ok(bytes) => {
									let all_matches = find_all_matches(&bytes, &re);
									let matches: Vec<SearchMatch> = match &range {
										SubstituteRange::WholeFile => all_matches,
										SubstituteRange::CurrentLine => all_matches
											.into_iter()
											.filter(|m| m.line == cursor_abs_line)
											.collect(),
										SubstituteRange::SingleLine(n) => {
											let n = *n;
											all_matches
												.into_iter()
												.filter(move |m| m.line == n)
												.collect()
										}
										SubstituteRange::LineRange(a, b) => {
											let (a, b) = (*a, *b);
											all_matches
												.into_iter()
												.filter(move |m| m.line >= a && m.line <= b)
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
										let current_match = search_matches[0];
										cursor_abs_line = current_match.line;
										let target = registry.find_node_at_line_col(
											rid,
											current_match.line,
											current_match.col,
										);
										apply_cursor_target(
											target,
											&mut cursor_node,
											&mut cursor_offset,
											&mut cursor_abs_col,
										);
										cursor_abs_col = current_match.col;
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
								// Replace current match, then re-scan from the replacement point.
								if let Ok(bytes) = read_loaded_document(&registry, rid) {
									let idx = search_match_index.unwrap_or(0);
									let current_match = search_matches[idx];
									let byte_off = byte_offset_from_line_col(
										&bytes,
										current_match.line,
										current_match.col,
									)
									.get() as usize;
									let rep = cs.replacement.as_bytes();
									let delta = TextDelta {
										global_byte_offset: DocByte::new(byte_off as u64),
										deleted_text: String::from_utf8_lossy(
											&bytes[byte_off..byte_off + current_match.byte_len],
										)
										.into_owned(),
										inserted_text: cs.replacement.clone(),
										state_before: StateId::ZERO,
										state_after: StateId::ZERO,
									};
									cs.replacements_done += 1;
									let applied_ok = match apply_deltas_to_document(
										&registry,
										&mut root_id,
										&mut cursor_node,
										&mut cursor_offset,
										&mut cursor_abs_line,
										&mut cursor_abs_col,
										&mut ledger,
										&mut semantic_highlights,
										vec![delta],
									) {
										Ok(()) => true,
										Err(msg) => {
											status_message = Some(msg);
											false
										}
									};

									if applied_ok {
										if let Some(new_rid) = root_id {
											let Ok(new_bytes) =
												read_loaded_document(&registry, new_rid)
											else {
												status_message = Some(
													"Failed to reload document after substitution"
														.to_string(),
												);
												continue;
											};
											// Re-scan for remaining matches after replacement point, filtered by range
											let re = build_regex(&pat, cs.flags.case_insensitive)
												.unwrap();
											let all_new = find_all_matches(&new_bytes, &re);
											let new_matches: Vec<SearchMatch> = match &cs.range {
												SubstituteRange::WholeFile => all_new,
												SubstituteRange::CurrentLine => all_new
													.into_iter()
													.filter(|m| m.line == current_match.line)
													.collect(),
												SubstituteRange::SingleLine(n) => {
													let n = *n;
													all_new
														.into_iter()
														.filter(move |m| m.line == n)
														.collect()
												}
												SubstituteRange::LineRange(a, b) => {
													let (a, b) = (*a, *b);
													all_new
														.into_iter()
														.filter(move |m| m.line >= a && m.line <= b)
														.collect()
												}
											};
											// Find next match at or after the replacement point
											let rep_end_line;
											let rep_end_col;
											{
												let mut l = current_match.line;
												let mut c = current_match.col;
												for &b in rep {
													advance_col(b, &mut l, &mut c);
												}
												rep_end_line = l;
												rep_end_col = c;
											}
											let next_idx = new_matches.iter().position(|m| {
												m.line > rep_end_line
													|| (m.line == rep_end_line
														&& m.col >= rep_end_col)
											});

											search_matches = new_matches;

											if let Some(ni) = next_idx {
												search_match_index = Some(ni);
												let current_match = search_matches[ni];
												cursor_abs_line = current_match.line;
												let target = registry.find_node_at_line_col(
													new_rid,
													current_match.line,
													current_match.col,
												);
												apply_cursor_target(
													target,
													&mut cursor_node,
													&mut cursor_offset,
													&mut cursor_abs_col,
												);
												cursor_abs_col = current_match.col;
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
								}
							}
							ConfirmAction::No => {
								let idx = search_match_index.unwrap_or(0);
								if idx + 1 < search_matches.len() {
									search_match_index = Some(idx + 1);
									let current_match = search_matches[idx + 1];
									cursor_abs_line = current_match.line;
									let target = registry.find_node_at_line_col(
										rid,
										current_match.line,
										current_match.col,
									);
									apply_cursor_target(
										target,
										&mut cursor_node,
										&mut cursor_offset,
										&mut cursor_abs_col,
									);
									cursor_abs_col = current_match.col;
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
								if let Ok(bytes) = read_loaded_document(&registry, rid) {
									// Replace all remaining from current position
									let idx = search_match_index.unwrap_or(0);
									let current_match = search_matches[idx];
									let byte_off = byte_offset_from_line_col(
										&bytes,
										current_match.line,
										current_match.col,
									)
									.get() as usize;
									let re = build_regex(&pat, cs.flags.case_insensitive).unwrap();
									let deltas = substitute_text_deltas(
										&bytes,
										&re,
										&cs.replacement,
										Some((byte_off, bytes.len())),
									);
									let count = deltas.len() as u32;
									cs.replacements_done += count;
									match apply_deltas_to_document(
										&registry,
										&mut root_id,
										&mut cursor_node,
										&mut cursor_offset,
										&mut cursor_abs_line,
										&mut cursor_abs_col,
										&mut ledger,
										&mut semantic_highlights,
										deltas,
									) {
										Ok(()) => {
											status_message = Some(format!(
												"{} substitution(s)",
												cs.replacements_done
											));
										}
										Err(msg) => status_message = Some(msg),
									}
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
						let line_start = line_start_byte_sparse(&registry, rid, cursor_abs_line);
						if let Ok(line_bytes) =
							read_line_bytes_sparse(&registry, rid, cursor_abs_line, true)
						{
							let line_text = String::from_utf8_lossy(&line_bytes).into_owned();

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
							let line_end = line_start.saturating_add(line_bytes.len() as u64);
							yank_flash = Some((line_start, line_end));
							let tx_flash = tx_cmd.clone();
							std::thread::spawn(move || {
								std::thread::sleep(std::time::Duration::from_millis(200));
								let _ = tx_flash.send(EditorCommand::ClearFlash);
							});

							status_message = Some(format!("{} bytes yanked", line_bytes.len()));
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
								if let Ok((insert_offset, inserted_text, target_cursor_line)) =
									linewise_put_insertion_sparse(
										&registry,
										rid,
										cursor_abs_line,
										&text,
									) {
									// Insert after current line (Vim 'p' for line-wise yank).
									if let Err(msg) = apply_deltas_to_document(
										&registry,
										&mut root_id,
										&mut cursor_node,
										&mut cursor_offset,
										&mut cursor_abs_line,
										&mut cursor_abs_col,
										&mut ledger,
										&mut semantic_highlights,
										vec![TextDelta {
											global_byte_offset: insert_offset,
											deleted_text: String::new(),
											inserted_text,
											state_before: StateId::ZERO,
											state_after: StateId::ZERO,
										}],
									) {
										status_message = Some(msg);
									} else if let Some(new_root) = root_id {
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
									}
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
					if root_id.is_some() {
						if let Some((inverse_deltas, cursor_byte)) = ledger.undo() {
							if let Err(msg) = apply_deltas_to_document_internal(
								&registry,
								&mut root_id,
								&mut cursor_node,
								&mut cursor_offset,
								&mut cursor_abs_line,
								&mut cursor_abs_col,
								&mut ledger,
								&mut semantic_highlights,
								inverse_deltas,
								false,
								Some(cursor_byte),
							) {
								status_message = Some(msg);
							}
							needs_render = true;
						} else {
							status_message = Some("Already at oldest change".to_string());
							needs_render = true;
						}
					}
				}
				EditorCommand::Redo => {
					if root_id.is_some() {
						if let Some((redo_deltas, cursor_byte)) = ledger.redo() {
							if let Err(msg) = apply_deltas_to_document_internal(
								&registry,
								&mut root_id,
								&mut cursor_node,
								&mut cursor_offset,
								&mut cursor_abs_line,
								&mut cursor_abs_col,
								&mut ledger,
								&mut semantic_highlights,
								redo_deltas,
								false,
								Some(cursor_byte),
							) {
								status_message = Some(msg);
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
					let total_lines = registry.get_total_newlines(rid);
					scroll_y = pan_scroll_y_to_keep_cursor_visible(
						scroll_y,
						cursor_abs_line,
						total_lines,
						viewport_lines,
					);
					let scroll_line = DocLine::new(scroll_y);

					// 1. Virtual Query (Look-behind 60 lines, Look-ahead 120 lines total)
					let virtual_start_line = scroll_line.saturating_sub(60);
					let virtual_line_count = viewport_lines + 120;
					let virtual_tokens =
						registry.query_viewport(rid, virtual_start_line, virtual_line_count);

					// 2. Visible Query (The actual UI screen)
					let visible_tokens = registry.query_viewport(rid, scroll_line, viewport_lines);

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

					(virtual_tokens, visible_tokens, total_lines)
				} else {
					scroll_y = 0;
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
								let live_len = registry.get_total_bytes(rid) as usize;
								let path_changed =
									last_semantic_path.as_deref() != Some(path.as_str());
								if path_changed
									|| last_semantic_state != Some(ledger.current_state_id)
									|| live_len != last_semantic_len
								{
									if live_len == 0 || live_len as u64 >= MAX_SEMANTIC_BYTES {
										semantic_highlights.clear();
										last_semantic_state = Some(ledger.current_state_id);
										last_semantic_len = live_len;
										last_semantic_path = Some(path.clone());
										pending_semantic_request_id = None;
									} else if let Ok(live_bytes) =
										read_loaded_document(&registry, rid)
									{
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
										// Lock only after a successful bounded send.
										last_semantic_state = Some(ledger.current_state_id);
										last_semantic_len = live_len;
										last_semantic_path = Some(path.clone());
										pending_semantic_request_id = Some(request_id);
									}
								}
							}
						} else {
							semantic_highlights.clear();
							last_semantic_state = Some(ledger.current_state_id);
							last_semantic_len = 0;
							last_semantic_path = Some(path.clone());
							pending_semantic_request_id = None;
						}
					} else {
						semantic_highlights.clear();
						last_semantic_state = None;
						last_semantic_len = usize::MAX;
						last_semantic_path = None;
						pending_semantic_request_id = None;
					}
				} else {
					semantic_highlights.clear();
					last_semantic_state = None;
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

				let (cursor_abs_byte, cursor_line_start_byte, file_size) =
					if let Some(rid) = root_id {
						let cursor_abs_byte = registry.doc_byte_for_node_offset(
							rid,
							cursor_node,
							NodeByteOffset::new(cursor_offset),
						);
						let line_start_target =
							registry.find_node_at_line_col(rid, cursor_abs_line, VisualCol::ZERO);
						let cursor_line_start_byte = registry.doc_byte_for_node_offset(
							rid,
							line_start_target.node_id,
							line_start_target.node_byte,
						);
						(
							cursor_abs_byte,
							cursor_line_start_byte,
							registry.get_total_bytes(rid),
						)
					} else {
						(DocByte::ZERO, DocByte::ZERO, 0)
					};
				let selection_ranges = match (active_visual, root_id) {
					(Some((anchor, kind)), Some(rid)) => {
						resolve_visual_ranges_sparse(&registry, rid, anchor, cursor_abs_byte, kind)
							.unwrap_or_default()
					}
					_ => Vec::new(),
				};
				let viewport_start_line = DocLine::new(scroll_y);
				let minimap = if let Some(rid) = root_id {
					let path_changed = last_minimap_path.as_deref() != file_path.as_deref();
					let state_changed = last_minimap_state != Some(ledger.current_state_id);
					let total_lines_changed = last_minimap_total_lines != total_lines;
					if cached_minimap.is_none()
						|| path_changed || state_changed
						|| total_lines_changed
					{
						cached_minimap =
							Some(if file_size > 0 && file_size <= MAX_MINIMAP_TEXT_BYTES {
								match read_loaded_document(&registry, rid) {
									Ok(bytes) => build_text_minimap_snapshot(
										&bytes,
										viewport_start_line,
										viewport_lines,
										cursor_abs_line,
									),
									Err(_) => build_byte_fallback_minimap_snapshot(
										&registry,
										rid,
										viewport_start_line,
										viewport_lines,
										cursor_abs_line,
									),
								}
							} else {
								build_byte_fallback_minimap_snapshot(
									&registry,
									rid,
									viewport_start_line,
									viewport_lines,
									cursor_abs_line,
								)
							});
						last_minimap_state = Some(ledger.current_state_id);
						last_minimap_total_lines = total_lines;
						last_minimap_path = file_path.clone();
					}

					cached_minimap.as_ref().map(|snapshot| {
						let mut snapshot = snapshot.clone();
						let (search_bands, active_search_band) = build_search_minimap_bands(
							&search_matches,
							search_match_index,
							total_lines,
						);
						snapshot.viewport_start_line = viewport_start_line;
						snapshot.viewport_line_count = viewport_lines;
						snapshot.cursor_line = cursor_abs_line;
						snapshot.search_bands = search_bands;
						snapshot.active_search_band = active_search_band;
						snapshot
					})
				} else {
					cached_minimap = None;
					last_minimap_state = None;
					last_minimap_total_lines = u32::MAX;
					last_minimap_path = None;
					None
				};

				let _ = tx_view.send(Viewport {
					tokens,
					scroll_y,
					viewport_line_count: viewport_lines,
					cursor_abs_pos: CursorPosition::new(cursor_abs_line, cursor_abs_col),
					cursor_abs_byte,
					cursor_line_start_byte,
					total_lines,
					status_message: status_message.take(),
					should_quit: pending_quit,
					file_name: file_path.clone(),
					file_size,
					is_dirty: ledger.is_dirty(),
					search_pattern: search_pattern.clone(),
					search_case_insensitive,
					search_match_info: search_match_info.clone(),
					confirm_prompt,
					mode_override: mode_override.take(),
					global_start_byte,
					highlights,
					selection_ranges,
					yank_flash,
					minimap,
					theme_colors: current_theme.syntax_colors.clone(),
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
		FILE_DEVICE_ID, MINIMAP_BANDS, SearchMatch, VisualKind, apply_deltas_to_document,
		apply_deltas_to_document_internal, build_search_minimap_bands,
		clamp_cursor_line_to_viewport, delete_char_delta_at_cursor, delete_to_line_end_delta,
		document_rewrite_delta, first_non_whitespace_visual_col, line_col_from_doc_byte_sparse,
		line_end_visual_col, linewise_put_insertion, next_word_end, next_word_start,
		pan_scroll_y_to_keep_cursor_visible, prev_word_start,
		rebase_semantic_highlights_after_delta, rebind_document_spans_to_saved_file,
		resolve_visual_ranges, save_document_atomic, scroll_viewport, smart_home_visual_col,
		step_left_visual_col, step_right_visual_col, word_object_delta_at_cursor,
	};
	use crate::core::{DocByte, DocLine, NodeByteOffset, StateId, VisualCol};
	use crate::ecs::UastRegistry;
	use crate::engine::undo::{TextDelta, UndoLedger};
	use crate::svp::SvpPointer;
	use crate::svp::highlight::{HighlightSpan, TokenCategory};
	use crate::uast::UastProjection;
	use crate::uast::kind::SemanticKind;
	use crate::uast::metrics::SpanMetrics;
	use std::path::PathBuf;
	use std::sync::atomic::Ordering;
	use std::time::{SystemTime, UNIX_EPOCH};

	fn build_document(text: &str) -> (UastRegistry, crate::ecs::NodeId) {
		let registry = UastRegistry::new(32);
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

	fn build_split_virtual_document(parts: &[&str]) -> (UastRegistry, crate::ecs::NodeId) {
		let registry = UastRegistry::new((parts.len() as u32) + 4);
		let total_bytes = parts.iter().map(|part| part.len()).sum::<usize>() as u32;
		let total_newlines = parts
			.iter()
			.map(|part| part.as_bytes().iter().filter(|&&b| b == b'\n').count() as u32)
			.sum();
		let mut chunk = registry
			.reserve_chunk((parts.len() as u32) + 1)
			.expect("OOM");
		let root = chunk.spawn_node(
			SemanticKind::RelationalTable,
			None,
			SpanMetrics {
				byte_length: total_bytes,
				newlines: total_newlines,
			},
		);

		for part in parts {
			let leaf = chunk.spawn_node(
				SemanticKind::Token,
				None,
				SpanMetrics {
					byte_length: part.len() as u32,
					newlines: part.as_bytes().iter().filter(|&&b| b == b'\n').count() as u32,
				},
			);
			chunk.append_local_child(root, leaf);
			unsafe {
				*registry.virtual_data[leaf.index()].get() = Some(part.as_bytes().to_vec());
			}
		}

		(registry, root)
	}

	fn temp_test_path(name: &str) -> PathBuf {
		let nanos = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.expect("time should move forward")
			.as_nanos();
		std::env::temp_dir().join(format!("baryon-{}-{}-{}", name, std::process::id(), nanos))
	}

	fn build_mixed_save_document() -> (UastRegistry, crate::ecs::NodeId) {
		let registry = UastRegistry::new(16);
		let mut chunk = registry.reserve_chunk(4).expect("OOM");
		let root = chunk.spawn_node(
			SemanticKind::RelationalTable,
			None,
			SpanMetrics {
				byte_length: 12,
				newlines: 0,
			},
		);
		let first = chunk.spawn_node(
			SemanticKind::Token,
			Some(SvpPointer {
				lba: 0,
				byte_length: 5,
				device_id: FILE_DEVICE_ID,
				head_trim: 0,
			}),
			SpanMetrics {
				byte_length: 5,
				newlines: 0,
			},
		);
		let middle = chunk.spawn_node(
			SemanticKind::Token,
			None,
			SpanMetrics {
				byte_length: 2,
				newlines: 0,
			},
		);
		let last = chunk.spawn_node(
			SemanticKind::Token,
			Some(SvpPointer {
				lba: 0,
				byte_length: 5,
				device_id: FILE_DEVICE_ID,
				head_trim: 5,
			}),
			SpanMetrics {
				byte_length: 5,
				newlines: 0,
			},
		);
		chunk.append_local_child(root, first);
		chunk.append_local_child(root, middle);
		chunk.append_local_child(root, last);
		unsafe {
			*registry.virtual_data[middle.index()].get() = Some(b"ZZ".to_vec());
		}
		(registry, root)
	}

	#[test]
	fn rewrite_delta_tracks_changed_middle_span() {
		let delta =
			document_rewrite_delta("fn main() {}\n", "fn demo() {}\n").expect("expected a delta");
		assert_eq!(delta.0, 3);
		assert_eq!(delta.1, "main");
		assert_eq!(delta.2, "demo");
	}

	#[test]
	fn sparse_line_col_tracks_columns_across_leaf_boundaries() {
		let (registry, root) = build_split_virtual_document(&["ab\ncd", "ef"]);
		let (line, col) =
			line_col_from_doc_byte_sparse(&registry, root, DocByte::new(6)).expect("line/col");
		assert_eq!(line, DocLine::new(1));
		assert_eq!(col, VisualCol::new(3));
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
	fn line_end_visual_col_expands_tabs() {
		assert_eq!(
			line_end_visual_col(b"\tfoo\n", DocLine::ZERO),
			VisualCol::new(7),
		);
		assert_eq!(
			line_end_visual_col(b"\t\tlet x = 1;\n", DocLine::ZERO),
			VisualCol::new(18),
		);
	}

	#[test]
	fn first_non_whitespace_visual_col_is_tab_aware() {
		assert_eq!(
			first_non_whitespace_visual_col(b" \t\tfoo\n", DocLine::ZERO),
			VisualCol::new(8),
		);
		assert_eq!(
			first_non_whitespace_visual_col(b"    \n", DocLine::ZERO),
			VisualCol::ZERO,
		);
	}

	#[test]
	fn smart_home_toggles_between_indent_and_line_start() {
		let doc = b"\t  foo\n";
		assert_eq!(
			smart_home_visual_col(doc, DocLine::ZERO, VisualCol::new(10)),
			VisualCol::new(6),
		);
		assert_eq!(
			smart_home_visual_col(doc, DocLine::ZERO, VisualCol::new(6)),
			VisualCol::ZERO,
		);
		assert_eq!(
			smart_home_visual_col(doc, DocLine::ZERO, VisualCol::ZERO),
			VisualCol::new(6),
		);
	}

	#[test]
	fn resolve_visual_ranges_returns_disjoint_char_range() {
		assert_eq!(
			resolve_visual_ranges(
				DocByte::new(8),
				DocByte::new(3),
				VisualKind::Char,
				b"alpha beta\n",
			),
			vec![(DocByte::new(3), DocByte::new(8))]
		);
	}

	#[test]
	fn resolve_visual_ranges_expands_line_mode_to_full_lines() {
		assert_eq!(
			resolve_visual_ranges(
				DocByte::new(4),
				DocByte::new(7),
				VisualKind::Line,
				b"aa\nbb\ncc\n",
			),
			vec![(DocByte::new(3), DocByte::new(8))]
		);
	}

	#[test]
	fn resolve_visual_ranges_handles_line_mode_at_eof() {
		assert_eq!(
			resolve_visual_ranges(
				DocByte::new(6),
				DocByte::new(9),
				VisualKind::Line,
				b"alpha\nbeta",
			),
			vec![(DocByte::new(6), DocByte::new(9))]
		);
	}

	#[test]
	fn resolve_visual_ranges_builds_block_matrix_across_lines() {
		assert_eq!(
			resolve_visual_ranges(
				DocByte::new(1),
				DocByte::new(12),
				VisualKind::Block,
				b"abcd\nefgh\nijkl\n",
			),
			vec![
				(DocByte::new(1), DocByte::new(2)),
				(DocByte::new(6), DocByte::new(7)),
				(DocByte::new(11), DocByte::new(12)),
			]
		);
	}

	#[test]
	fn resolve_visual_ranges_clamps_and_skips_short_lines_in_block_mode() {
		assert_eq!(
			resolve_visual_ranges(
				DocByte::new(2),
				DocByte::new(17),
				VisualKind::Block,
				b"abcdef\nabc\nz\nabcdef\n",
			),
			vec![
				(DocByte::new(2), DocByte::new(4)),
				(DocByte::new(9), DocByte::new(9)),
				(DocByte::new(15), DocByte::new(17)),
			]
		);
	}

	#[test]
	fn word_object_delta_uses_ast_identifier_boundaries() {
		let (registry, root) = build_document("let alpha = beta;\n");
		let delta = word_object_delta_at_cursor(&registry, root, DocByte::new(5))
			.expect("word lookup should succeed")
			.expect("expected a word delta");

		assert_eq!(delta.global_byte_offset, DocByte::new(4));
		assert_eq!(delta.deleted_text, "alpha");
		assert_eq!(delta.inserted_text, "");
	}

	#[test]
	fn word_object_delta_falls_forward_from_whitespace() {
		let (registry, root) = build_document("let alpha beta\n");
		let delta = word_object_delta_at_cursor(&registry, root, DocByte::new(9))
			.expect("word lookup should succeed")
			.expect("expected a word delta");

		assert_eq!(delta.global_byte_offset, DocByte::new(10));
		assert_eq!(delta.deleted_text, "beta");
	}

	#[test]
	fn delete_to_line_end_delta_stops_before_newline() {
		let delta =
			delete_to_line_end_delta(b"alpha beta\nomega\n", DocLine::ZERO, VisualCol::new(6))
				.expect("expected delete-to-eol delta");

		assert_eq!(delta.global_byte_offset, DocByte::new(6));
		assert_eq!(delta.deleted_text, "beta");
		assert_eq!(delta.inserted_text, "");
	}

	#[test]
	fn delete_char_delta_at_cursor_uses_utf8_width() {
		let (registry, root) = build_document("aéz");
		let delta = delete_char_delta_at_cursor(&registry, root, DocByte::new(1))
			.expect("delete lookup should succeed")
			.expect("expected delete delta");

		assert_eq!(delta.global_byte_offset, DocByte::new(1));
		assert_eq!(delta.deleted_text, "é");
		assert_eq!(delta.inserted_text, "");
	}

	#[test]
	fn forward_delete_keeps_cursor_logical_position() {
		let (registry, root) = build_document("abcd");
		let mut root_id = Some(root);
		let target = registry.find_node_at_line_col(root, DocLine::ZERO, VisualCol::new(1));
		let mut cursor_node = target.node_id;
		let mut cursor_offset = target.node_byte.get();
		let mut cursor_abs_line = DocLine::ZERO;
		let mut cursor_abs_col = target.visual_col;
		let mut ledger = UndoLedger::new();
		let mut semantic_highlights = Vec::new();

		let delta = delete_char_delta_at_cursor(&registry, root, DocByte::new(1))
			.expect("delete lookup should succeed")
			.expect("expected delete delta");
		apply_deltas_to_document(
			&registry,
			&mut root_id,
			&mut cursor_node,
			&mut cursor_offset,
			&mut cursor_abs_line,
			&mut cursor_abs_col,
			&mut ledger,
			&mut semantic_highlights,
			vec![delta],
		)
		.expect("forward delete should apply");

		let root = root_id.expect("mutated root");
		let bytes = registry
			.read_loaded_slice(
				root,
				DocByte::ZERO,
				DocByte::new(registry.get_total_bytes(root)),
			)
			.expect("collect mutated bytes");
		assert_eq!(String::from_utf8(bytes).expect("utf8"), "acd");
		assert_eq!(cursor_abs_line, DocLine::ZERO);
		assert_eq!(cursor_abs_col, VisualCol::new(1));
		assert_eq!(
			registry.doc_byte_for_node_offset(
				root,
				cursor_node,
				NodeByteOffset::new(cursor_offset)
			),
			DocByte::new(1),
		);
	}

	#[test]
	fn step_right_visual_col_jumps_across_tabs() {
		assert_eq!(
			step_right_visual_col(b"\tfoo\n", DocLine::ZERO, VisualCol::ZERO),
			VisualCol::new(4),
		);
		assert_eq!(
			step_right_visual_col(b" \tfoo\n", DocLine::ZERO, VisualCol::new(1)),
			VisualCol::new(4),
		);
	}

	#[test]
	fn step_left_visual_col_jumps_back_across_tabs() {
		assert_eq!(
			step_left_visual_col(b"\tfoo\n", DocLine::ZERO, VisualCol::new(4)),
			VisualCol::ZERO,
		);
		assert_eq!(
			step_left_visual_col(b" \tfoo\n", DocLine::ZERO, VisualCol::new(4)),
			VisualCol::new(1),
		);
	}

	#[test]
	fn next_word_start_handles_punctuation_boundaries() {
		let doc = "foo::bar baz";
		assert_eq!(next_word_start(doc, 0), 3);
		assert_eq!(next_word_start(doc, 3), 5);
		assert_eq!(next_word_start(doc, 5), 9);
	}

	#[test]
	fn prev_word_start_respects_utf8_boundaries() {
		let doc = "éx yz";
		assert_eq!(prev_word_start(doc, 4), 0);
		assert_eq!(prev_word_start(doc, 2), 0);
	}

	#[test]
	fn next_word_end_respects_utf8_boundaries() {
		let doc = "éx yz";
		assert_eq!(next_word_end(doc, 0), 2);
		assert_eq!(next_word_end(doc, 2), 5);
	}

	#[test]
	fn search_minimap_bands_track_hits_and_active_match() {
		let matches = vec![
			SearchMatch {
				line: DocLine::new(10),
				col: VisualCol::ZERO,
				byte_len: 3,
			},
			SearchMatch {
				line: DocLine::new(90),
				col: VisualCol::ZERO,
				byte_len: 3,
			},
		];
		let (bands, active) = build_search_minimap_bands(&matches, Some(1), 100);

		assert!(bands.iter().any(|&band| band > 0));
		assert_eq!(active, Some((90usize * MINIMAP_BANDS) / 100usize));
	}

	#[test]
	fn pan_scroll_y_keeps_visible_cursor_stationary() {
		assert_eq!(
			pan_scroll_y_to_keep_cursor_visible(10, DocLine::new(20), 200, 50),
			10
		);
	}

	#[test]
	fn pan_scroll_y_moves_up_when_cursor_above_view() {
		assert_eq!(
			pan_scroll_y_to_keep_cursor_visible(30, DocLine::new(12), 200, 50),
			12
		);
	}

	#[test]
	fn pan_scroll_y_moves_down_when_cursor_below_view() {
		assert_eq!(
			pan_scroll_y_to_keep_cursor_visible(10, DocLine::new(65), 200, 20),
			46
		);
	}

	#[test]
	fn pan_scroll_y_keeps_cursor_on_last_visible_line_stationary() {
		assert_eq!(
			pan_scroll_y_to_keep_cursor_visible(10, DocLine::new(29), 200, 20),
			10
		);
	}

	#[test]
	fn scroll_viewport_clamps_to_document_bounds() {
		assert_eq!(scroll_viewport(0, -3, 99, 50), 0);
		assert_eq!(scroll_viewport(45, 10, 99, 50), 50);
	}

	#[test]
	fn clamp_cursor_line_to_viewport_uses_visible_bounds() {
		assert_eq!(
			clamp_cursor_line_to_viewport(DocLine::new(7), 10, 99, 20),
			DocLine::new(10),
		);
		assert_eq!(
			clamp_cursor_line_to_viewport(DocLine::new(40), 10, 99, 20),
			DocLine::new(29),
		);
	}

	#[test]
	fn grouped_deltas_undo_as_a_single_transaction() {
		let (registry, root) = build_document("abcd\nefgh\nijkl\n");
		let mut root_id = Some(root);
		let mut cursor_node = registry
			.find_node_at_line_col(root, DocLine::ZERO, VisualCol::ZERO)
			.node_id;
		let mut cursor_offset = 0;
		let mut cursor_abs_line = DocLine::ZERO;
		let mut cursor_abs_col = VisualCol::ZERO;
		let mut ledger = UndoLedger::new();
		let mut semantic_highlights = Vec::new();

		apply_deltas_to_document(
			&registry,
			&mut root_id,
			&mut cursor_node,
			&mut cursor_offset,
			&mut cursor_abs_line,
			&mut cursor_abs_col,
			&mut ledger,
			&mut semantic_highlights,
			vec![
				TextDelta {
					global_byte_offset: DocByte::new(12),
					deleted_text: "kl".to_string(),
					inserted_text: String::new(),
					state_before: StateId::ZERO,
					state_after: StateId::ZERO,
				},
				TextDelta {
					global_byte_offset: DocByte::new(7),
					deleted_text: "gh".to_string(),
					inserted_text: String::new(),
					state_before: StateId::ZERO,
					state_after: StateId::ZERO,
				},
				TextDelta {
					global_byte_offset: DocByte::new(2),
					deleted_text: "cd".to_string(),
					inserted_text: String::new(),
					state_before: StateId::ZERO,
					state_after: StateId::ZERO,
				},
			],
		)
		.expect("grouped delete should apply");

		assert_eq!(ledger.current_state_id, StateId::new(1));
		let bytes = registry
			.read_loaded_slice(
				root_id.expect("mutated root"),
				DocByte::ZERO,
				DocByte::new(registry.get_total_bytes(root_id.expect("mutated root"))),
			)
			.expect("collect mutated bytes");
		assert_eq!(String::from_utf8(bytes).expect("utf8"), "ab\nef\nij\n");
		assert_eq!(cursor_abs_line, DocLine::ZERO);
		assert_eq!(cursor_abs_col, VisualCol::new(2));

		let (undo_group, undo_cursor_byte) = ledger
			.undo()
			.expect("single undo should restore whole group");
		assert_eq!(undo_group.len(), 3);
		assert_eq!(ledger.current_state_id, StateId::ZERO);
		apply_deltas_to_document_internal(
			&registry,
			&mut root_id,
			&mut cursor_node,
			&mut cursor_offset,
			&mut cursor_abs_line,
			&mut cursor_abs_col,
			&mut ledger,
			&mut semantic_highlights,
			undo_group,
			false,
			Some(undo_cursor_byte),
		)
		.expect("undo deltas should apply sparsely");
		let undo_root = root_id.expect("root after undo");
		let restored = registry
			.read_loaded_slice(
				undo_root,
				DocByte::ZERO,
				DocByte::new(registry.get_total_bytes(undo_root)),
			)
			.expect("collect restored bytes");
		assert_eq!(
			String::from_utf8(restored).expect("utf8"),
			"abcd\nefgh\nijkl\n"
		);

		let (redo_group, redo_cursor_byte) = ledger
			.redo()
			.expect("single redo should reapply whole group");
		assert_eq!(redo_group.len(), 3);
		assert_eq!(ledger.current_state_id, StateId::new(1));
		apply_deltas_to_document_internal(
			&registry,
			&mut root_id,
			&mut cursor_node,
			&mut cursor_offset,
			&mut cursor_abs_line,
			&mut cursor_abs_col,
			&mut ledger,
			&mut semantic_highlights,
			redo_group,
			false,
			Some(redo_cursor_byte),
		)
		.expect("redo deltas should apply sparsely");
		let redo_root = root_id.expect("root after redo");
		let redone = registry
			.read_loaded_slice(
				redo_root,
				DocByte::ZERO,
				DocByte::new(registry.get_total_bytes(redo_root)),
			)
			.expect("collect redone bytes");
		assert_eq!(String::from_utf8(redone).expect("utf8"), "ab\nef\nij\n");
	}

	#[test]
	fn save_document_atomic_streams_mixed_physical_and_virtual_leaves() {
		let source_path = temp_test_path("save-source");
		let target_path = temp_test_path("save-target");
		std::fs::write(&source_path, b"alphagamma").expect("write source");

		let (registry, root) = build_mixed_save_document();
		let bytes_written = save_document_atomic(&registry, root, Some(&source_path), &target_path)
			.expect("streaming save should succeed");
		assert_eq!(bytes_written, 12);

		let written = std::fs::read(&target_path).expect("read target");
		assert_eq!(&written, b"alphaZZgamma");

		let _ = std::fs::remove_file(source_path);
		let _ = std::fs::remove_file(target_path);
	}

	#[test]
	fn rebind_document_spans_to_saved_file_re_sparsifies_saved_leaves() {
		let (registry, root) = build_mixed_save_document();
		registry.dma_in_flight[1].store(true, Ordering::Relaxed);
		registry.dma_in_flight[2].store(true, Ordering::Relaxed);
		registry.dma_in_flight[3].store(true, Ordering::Relaxed);
		rebind_document_spans_to_saved_file(&registry, root, FILE_DEVICE_ID);

		let first = registry.get_first_child(root).expect("first leaf");
		let second = registry.get_next_sibling(first).expect("second leaf");
		let third = registry.get_next_sibling(second).expect("third leaf");

		let first_span = unsafe { (*registry.spans[first.index()].get()).expect("first span") };
		let second_span = unsafe { (*registry.spans[second.index()].get()).expect("second span") };
		let third_span = unsafe { (*registry.spans[third.index()].get()).expect("third span") };

		assert_eq!(first_span.lba * 512 + u64::from(first_span.head_trim), 0);
		assert_eq!(second_span.lba * 512 + u64::from(second_span.head_trim), 5);
		assert_eq!(third_span.lba * 512 + u64::from(third_span.head_trim), 7);

		assert!(unsafe { (*registry.virtual_data[first.index()].get()).is_none() });
		assert!(unsafe { (*registry.virtual_data[second.index()].get()).is_none() });
		assert!(unsafe { (*registry.virtual_data[third.index()].get()).is_none() });

		assert!(!registry.dma_in_flight[first.index()].load(Ordering::Relaxed));
		assert!(!registry.dma_in_flight[second.index()].load(Ordering::Relaxed));
		assert!(!registry.dma_in_flight[third.index()].load(Ordering::Relaxed));

		assert!(registry.metrics_inflated[first.index()].load(Ordering::Relaxed));
		assert!(registry.metrics_inflated[second.index()].load(Ordering::Relaxed));
		assert!(registry.metrics_inflated[third.index()].load(Ordering::Relaxed));
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
				state_before: StateId::ZERO,
				state_after: StateId::ZERO,
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
				state_before: StateId::ZERO,
				state_after: StateId::ZERO,
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
