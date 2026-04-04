use crate::core::{DocByte, DocLine, TAB_SIZE, VisualCol};
use crate::ecs::{NodeId, UastRegistry};
use crate::uast::{RootChildLineIndex, UastProjection};
use ra_ap_syntax::{Direction, Edition, SyntaxKind, SyntaxToken, TextSize};

pub(crate) fn line_byte_range(
	doc: &[u8],
	start_line: DocLine,
	end_line: DocLine,
) -> (usize, usize) {
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

pub(crate) fn line_content_slice(doc: &[u8], target_line: DocLine) -> &[u8] {
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

pub(crate) fn line_end_visual_col(doc: &[u8], target_line: DocLine) -> VisualCol {
	let mut col = VisualCol::ZERO;
	for &b in line_content_slice(doc, target_line) {
		advance_visual_col_only(&mut col, b);
	}
	col
}

pub(crate) fn first_non_whitespace_visual_col(doc: &[u8], target_line: DocLine) -> VisualCol {
	let mut col = VisualCol::ZERO;
	for &b in line_content_slice(doc, target_line) {
		match b {
			b' ' | b'\t' => advance_visual_col_only(&mut col, b),
			_ => return col,
		}
	}

	VisualCol::ZERO
}

pub(crate) fn line_start_byte_sparse(
	registry: &UastRegistry,
	root: NodeId,
	target_line: DocLine,
) -> DocByte {
	line_start_byte_sparse_with_root_line_index(registry, root, target_line, None)
}

pub(crate) fn line_start_byte_sparse_with_root_line_index(
	registry: &UastRegistry,
	root: NodeId,
	target_line: DocLine,
	line_index: Option<&RootChildLineIndex>,
) -> DocByte {
	let target = if let Some(line_index) = line_index {
		registry.find_node_at_line_col_raw_with_root_line_index(
			root,
			target_line,
			VisualCol::ZERO,
			line_index,
		)
	} else {
		registry.find_node_at_line_col_raw(root, target_line, VisualCol::ZERO)
	};
	registry.doc_byte_for_node_offset(root, target.node_id, target.node_byte)
}

pub(crate) fn line_end_byte_sparse(
	registry: &UastRegistry,
	root: NodeId,
	target_line: DocLine,
	include_newline: bool,
) -> Result<DocByte, String> {
	line_end_byte_sparse_with_root_line_index(registry, root, target_line, include_newline, None)
}

pub(crate) fn line_end_byte_sparse_with_root_line_index(
	registry: &UastRegistry,
	root: NodeId,
	target_line: DocLine,
	include_newline: bool,
	line_index: Option<&RootChildLineIndex>,
) -> Result<DocByte, String> {
	let line_start_target = if let Some(line_index) = line_index {
		registry.find_node_at_line_col_raw_with_root_line_index(
			root,
			target_line,
			VisualCol::ZERO,
			line_index,
		)
	} else {
		registry.find_node_at_line_col_raw(root, target_line, VisualCol::ZERO)
	};
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

pub(crate) fn read_line_bytes_sparse(
	registry: &UastRegistry,
	root: NodeId,
	target_line: DocLine,
	include_newline: bool,
) -> Result<Vec<u8>, String> {
	read_line_bytes_sparse_with_root_line_index(registry, root, target_line, include_newline, None)
}

pub(crate) fn read_line_bytes_sparse_with_root_line_index(
	registry: &UastRegistry,
	root: NodeId,
	target_line: DocLine,
	include_newline: bool,
	line_index: Option<&RootChildLineIndex>,
) -> Result<Vec<u8>, String> {
	let start =
		line_start_byte_sparse_with_root_line_index(registry, root, target_line, line_index);
	let end = line_end_byte_sparse_with_root_line_index(
		registry,
		root,
		target_line,
		include_newline,
		line_index,
	)?;
	registry
		.read_loaded_slice(root, start, end)
		.map_err(|msg| msg.to_string())
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

pub(crate) fn select_structural_token_at_offset(
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

pub(crate) fn resolve_loaded_node_text(
	registry: &UastRegistry,
	node: NodeId,
) -> Result<String, String> {
	let text = registry.resolve_physical_bytes(node);
	let byte_len = unsafe { (*registry.metrics[node.index()].get()).byte_length as usize };
	if text.is_empty() && byte_len > 0 {
		return Err("File still loading, cannot resolve structural token".to_string());
	}
	String::from_utf8(text).map_err(|_| "Loaded node is not valid UTF-8".to_string())
}
