use super::core::{
	first_non_whitespace_visual_col, line_byte_range, line_content_slice,
	select_structural_token_at_offset,
};
use crate::core::{DocByte, DocLine, NodeByteOffset, VisualCol};
use crate::ecs::{NodeId, UastRegistry};
use crate::engine::undo::line_col_from_byte_offset;
use crate::uast::kind::SemanticKind;
use crate::uast::metrics::SpanMetrics;
use crate::uast::topology::TreeEdges;
use crate::uast::{NodeCursorTarget, RenderToken, UastProjection};
use ra_ap_syntax::{AstNode, Edition, SourceFile, SyntaxKind, TextSize};
use std::sync::atomic::Ordering;

pub(crate) fn visual_line_index_for_doc_line(
	registry: &UastRegistry,
	root: NodeId,
	target_line: DocLine,
) -> u32 {
	let mut visual_index = 0u32;
	let mut line_accumulator = DocLine::ZERO;
	let mut visit = Some(root);

	while let Some(node) = visit {
		let idx = node.index();
		let has_children = unsafe { (*registry.edges[idx].get()).first_child.is_some() };
		let is_folded = has_children && registry.is_folded[idx].load(Ordering::Acquire);
		if has_children && !is_folded {
			visit = registry.get_first_child(node);
			continue;
		}

		let metrics = unsafe { &*registry.metrics[idx].get() };
		let node_end_line = line_accumulator.saturating_add(metrics.newlines);
		let contains_target_line = target_line < node_end_line
			|| (target_line == node_end_line
				&& (!is_folded || registry.next_node_skipping_children(node).is_none()));
		if contains_target_line {
			if is_folded {
				return visual_index;
			}
			return visual_index
				.saturating_add(target_line.saturating_sub(line_accumulator.get()).get());
		}

		if is_folded {
			visual_index = visual_index.saturating_add(1);
		} else {
			visual_index = visual_index.saturating_add(metrics.newlines);
		}
		line_accumulator = node_end_line;
		visit = registry.next_node_skipping_children(node);
	}

	visual_index
}

pub(crate) fn max_visual_line_index(
	registry: &UastRegistry,
	root: NodeId,
	total_lines: u32,
) -> u32 {
	visual_line_index_for_doc_line(registry, root, DocLine::new(total_lines))
}

pub(crate) fn doc_line_for_visual_index(
	registry: &UastRegistry,
	root: NodeId,
	target_visual_index: u32,
	total_lines: u32,
) -> DocLine {
	let mut visual_index = 0u32;
	let mut line_accumulator = DocLine::ZERO;
	let mut visit = Some(root);

	while let Some(node) = visit {
		let idx = node.index();
		let has_children = unsafe { (*registry.edges[idx].get()).first_child.is_some() };
		let is_folded = has_children && registry.is_folded[idx].load(Ordering::Acquire);
		if has_children && !is_folded {
			visit = registry.get_first_child(node);
			continue;
		}

		let metrics = unsafe { &*registry.metrics[idx].get() };
		if is_folded {
			if visual_index >= target_visual_index {
				return line_accumulator;
			}
			visual_index = visual_index.saturating_add(1);
		} else {
			let next_visual = visual_index.saturating_add(metrics.newlines);
			if target_visual_index <= next_visual {
				return line_accumulator
					.saturating_add(target_visual_index.saturating_sub(visual_index));
			}
			visual_index = next_visual;
		}

		line_accumulator = line_accumulator.saturating_add(metrics.newlines);
		visit = registry.next_node_skipping_children(node);
	}

	DocLine::new(total_lines)
}

pub(crate) fn snap_line_to_visible_boundary(
	registry: &UastRegistry,
	root: NodeId,
	line: DocLine,
) -> DocLine {
	let visual_index = visual_line_index_for_doc_line(registry, root, line);
	let total_lines = registry.get_total_newlines(root);
	doc_line_for_visual_index(registry, root, visual_index, total_lines)
}

pub(crate) fn pan_scroll_y_to_keep_cursor_visible(
	registry: &UastRegistry,
	root: NodeId,
	scroll_y: u32,
	cursor_line: DocLine,
	total_lines: u32,
	viewport_lines: u32,
) -> u32 {
	let viewport_lines = viewport_lines.max(1);
	let max_visual = max_visual_line_index(registry, root, total_lines);
	let mut top_visual =
		visual_line_index_for_doc_line(registry, root, DocLine::new(scroll_y.min(total_lines)));
	let cursor_visual = visual_line_index_for_doc_line(registry, root, cursor_line);

	if cursor_visual < top_visual {
		top_visual = cursor_visual;
	} else {
		let bottom_visual = top_visual.saturating_add(viewport_lines.saturating_sub(1));
		if cursor_visual > bottom_visual {
			top_visual = cursor_visual.saturating_sub(viewport_lines.saturating_sub(1));
		}
	}

	doc_line_for_visual_index(registry, root, top_visual.min(max_visual), total_lines).get()
}

pub(crate) fn scroll_viewport(
	registry: &UastRegistry,
	root: NodeId,
	scroll_y: u32,
	delta: i32,
	total_lines: u32,
	_viewport_lines: u32,
) -> u32 {
	let top_visual =
		visual_line_index_for_doc_line(registry, root, DocLine::new(scroll_y.min(total_lines)));
	let max_visual = max_visual_line_index(registry, root, total_lines);
	let new_visual = (top_visual as i64 + delta as i64).clamp(0, max_visual as i64) as u32;
	doc_line_for_visual_index(registry, root, new_visual, total_lines).get()
}

pub(crate) fn clamp_cursor_line_to_viewport(
	registry: &UastRegistry,
	root: NodeId,
	cursor_line: DocLine,
	scroll_y: u32,
	total_lines: u32,
	viewport_lines: u32,
) -> DocLine {
	let viewport_lines = viewport_lines.max(1);
	let top_visual =
		visual_line_index_for_doc_line(registry, root, DocLine::new(scroll_y.min(total_lines)));
	let max_visual = max_visual_line_index(registry, root, total_lines);
	let bottom_visual = top_visual
		.saturating_add(viewport_lines.saturating_sub(1))
		.min(max_visual);
	let cursor_visual = visual_line_index_for_doc_line(registry, root, cursor_line);
	if cursor_visual < top_visual {
		doc_line_for_visual_index(registry, root, top_visual, total_lines)
	} else if cursor_visual > bottom_visual {
		doc_line_for_visual_index(registry, root, bottom_visual, total_lines)
	} else {
		cursor_line
	}
}

fn node_is_foldable_boundary(registry: &UastRegistry, root: NodeId, node: NodeId) -> bool {
	if node == root {
		return false;
	}

	let idx = node.index();
	let has_children = unsafe { (*registry.edges[idx].get()).first_child.is_some() };
	has_children && !matches!(unsafe { *registry.kinds[idx].get() }, SemanticKind::Token)
}

pub(crate) fn nearest_foldable_boundary(
	registry: &UastRegistry,
	root: NodeId,
	start: NodeId,
) -> Option<NodeId> {
	let mut curr = Some(start);
	while let Some(node) = curr {
		if node_is_foldable_boundary(registry, root, node) {
			return Some(node);
		}
		curr = unsafe { (*registry.edges[node.index()].get()).parent };
	}
	None
}

pub(crate) fn set_subtree_fold_state(registry: &UastRegistry, root: NodeId, folded: bool) {
	let mut visit = Some(root);
	while let Some(node) = visit {
		if node_is_foldable_boundary(registry, root, node) {
			registry.is_folded[node.index()].store(folded, Ordering::Release);
		}
		visit = registry.get_next_node_in_walk(node);
	}
}

pub(crate) fn unfold_ancestor_chain(registry: &UastRegistry, node: NodeId) {
	let mut curr = unsafe { (*registry.edges[node.index()].get()).parent };
	while let Some(parent) = curr {
		registry.is_folded[parent.index()].store(false, Ordering::Release);
		curr = unsafe { (*registry.edges[parent.index()].get()).parent };
	}
}

pub(crate) fn reveal_line_col_target(
	registry: &UastRegistry,
	root: NodeId,
	target_line: DocLine,
	target_col: VisualCol,
) -> NodeCursorTarget {
	let raw_target = registry.find_node_at_line_col_raw(root, target_line, target_col);
	unfold_ancestor_chain(registry, raw_target.node_id);
	registry.find_node_at_line_col(root, target_line, target_col)
}

pub(crate) fn collect_visible_line_starts(tokens: &[RenderToken]) -> Vec<DocLine> {
	let mut lines = Vec::new();
	for token in tokens {
		if lines.last().copied() != Some(token.absolute_start_line) {
			lines.push(token.absolute_start_line);
		}
		if token.is_folded {
			continue;
		}

		let mut line = token.absolute_start_line;
		for &byte in &token.text {
			if byte == b'\n' {
				line = line.saturating_add(1);
				if lines.last().copied() != Some(line) {
					lines.push(line);
				}
			}
		}
	}
	lines
}

fn count_newlines(bytes: &[u8]) -> u32 {
	bytes.iter().filter(|&&byte| byte == b'\n').count() as u32
}

fn init_virtual_token_node(
	registry: &UastRegistry,
	node: NodeId,
	parent: NodeId,
	next_sibling: Option<NodeId>,
	bytes: &[u8],
) {
	let idx = node.index();
	unsafe {
		*registry.kinds[idx].get() = SemanticKind::Token;
		*registry.spans[idx].get() = None;
		*registry.metrics[idx].get() = SpanMetrics {
			byte_length: bytes.len() as u32,
			newlines: count_newlines(bytes),
		};
		*registry.edges[idx].get() = TreeEdges {
			parent: Some(parent),
			first_child: None,
			next_sibling,
		};
		*registry.child_tails[idx].get() = None;
		*registry.virtual_data[idx].get() = Some(bytes.to_vec());
	}
	registry.dma_in_flight[idx].store(false, Ordering::Relaxed);
	registry.metric_inflating[idx].store(false, Ordering::Relaxed);
	registry.metrics_inflated[idx].store(true, Ordering::Relaxed);
	registry.is_folded[idx].store(false, Ordering::Relaxed);
}

fn init_fold_boundary_node(
	registry: &UastRegistry,
	node: NodeId,
	parent: NodeId,
	next_sibling: Option<NodeId>,
	child: NodeId,
	metrics: SpanMetrics,
) {
	let idx = node.index();
	unsafe {
		*registry.kinds[idx].get() = SemanticKind::RelationalRow;
		*registry.spans[idx].get() = None;
		*registry.metrics[idx].get() = metrics;
		*registry.edges[idx].get() = TreeEdges {
			parent: Some(parent),
			first_child: Some(child),
			next_sibling,
		};
		*registry.child_tails[idx].get() = Some(child);
		*registry.virtual_data[idx].get() = None;
	}
	registry.dma_in_flight[idx].store(false, Ordering::Relaxed);
	registry.metric_inflating[idx].store(false, Ordering::Relaxed);
	registry.metrics_inflated[idx].store(true, Ordering::Relaxed);
	registry.is_folded[idx].store(false, Ordering::Relaxed);
}

fn trim_horizontal_whitespace(mut bytes: &[u8]) -> &[u8] {
	while matches!(bytes.first(), Some(b' ' | b'\t' | b'\r')) {
		bytes = &bytes[1..];
	}
	while matches!(bytes.last(), Some(b' ' | b'\t' | b'\r')) {
		bytes = &bytes[..bytes.len() - 1];
	}
	bytes
}

fn preceding_attribute_block_start_line(bytes: &[u8], start_line: DocLine) -> DocLine {
	if start_line == DocLine::ZERO {
		return start_line;
	}

	let mut lines = Vec::new();
	let mut current = start_line.get();
	while current > 0 {
		current -= 1;
		let line = DocLine::new(current);
		let trimmed = trim_horizontal_whitespace(line_content_slice(bytes, line));
		if trimmed.is_empty() {
			break;
		}
		lines.push(line);
	}

	if lines.is_empty() {
		return start_line;
	}

	lines.reverse();
	let mut bracket_depth = 0i32;
	for &line in &lines {
		let trimmed = trim_horizontal_whitespace(line_content_slice(bytes, line));
		let starts_attr = trimmed.starts_with(b"#[") || trimmed.starts_with(b"#![");
		if bracket_depth == 0 && !starts_attr {
			return start_line;
		}
		for &byte in trimmed {
			if byte == b'[' {
				bracket_depth += 1;
			} else if byte == b']' {
				bracket_depth = bracket_depth.saturating_sub(1);
			}
		}
	}

	if bracket_depth == 0 {
		lines.first().copied().unwrap_or(start_line)
	} else {
		start_line
	}
}

fn multiline_node_range(bytes: &[u8], node: &ra_ap_syntax::SyntaxNode) -> Option<(usize, usize)> {
	let range = node.text_range();
	let start = u32::from(range.start()) as usize;
	let end = u32::from(range.end()) as usize;
	if end <= start {
		return None;
	}

	let start_line = line_col_from_byte_offset(bytes, DocByte::new(start as u64)).0;
	let end_line = line_col_from_byte_offset(bytes, DocByte::new(end.saturating_sub(1) as u64)).0;
	if end_line <= start_line {
		return None;
	}

	let start_line = preceding_attribute_block_start_line(bytes, start_line);
	let (line_start, line_end) = line_byte_range(bytes, start_line, end_line);
	(line_end > line_start).then_some((line_start, line_end))
}

fn find_syntax_fold_range(bytes: &[u8], cursor_local_byte: usize) -> Option<(usize, usize)> {
	let text = std::str::from_utf8(bytes).ok()?;
	if text.is_empty() {
		return None;
	}

	let parse = SourceFile::parse(text, Edition::Edition2024);
	let syntax = parse.tree().syntax().clone();
	let offset = TextSize::from(fold_anchor_offset(bytes, cursor_local_byte) as u32);
	let token = select_structural_token_at_offset(&syntax, offset).or_else(|| {
		syntax
			.token_at_offset(offset)
			.left_biased()
			.or_else(|| syntax.token_at_offset(offset).right_biased())
	})?;

	let mut node = token.parent();
	let mut fallback_range = None;
	let mut fn_candidate_in_impl = None;
	while let Some(current) = node {
		if current.kind() == SyntaxKind::SOURCE_FILE {
			break;
		}

		if let Some(range) = multiline_node_range(bytes, &current) {
			if fallback_range.is_none() {
				fallback_range = Some(range);
			}
			if current.kind() == SyntaxKind::FN && fn_candidate_in_impl.is_none() {
				fn_candidate_in_impl = Some(range);
			}
		}

		if current.kind() == SyntaxKind::IMPL {
			if let Some(range) = fn_candidate_in_impl {
				return Some(range);
			}
		}

		node = current.parent();
	}

	fallback_range
}

fn is_blank_line(doc: &[u8], line: DocLine) -> bool {
	line_content_slice(doc, line)
		.iter()
		.all(|byte| matches!(byte, b' ' | b'\t' | b'\r'))
}

fn is_scope_closer_line(doc: &[u8], line: DocLine) -> bool {
	let slice = line_content_slice(doc, line);
	let Some(start) = slice
		.iter()
		.position(|byte| !matches!(byte, b' ' | b'\t' | b'\r'))
	else {
		return false;
	};
	let end = slice
		.iter()
		.rposition(|byte| !matches!(byte, b' ' | b'\t' | b'\r'))
		.expect("trimmed non-blank line must have an end")
		+ 1;
	slice[start..end]
		.iter()
		.all(|byte| matches!(byte, b'}' | b')' | b']' | b';' | b','))
}

fn first_significant_byte_on_line(doc: &[u8], line: DocLine) -> Option<usize> {
	let (line_start, _) = line_byte_range(doc, line, line);
	line_content_slice(doc, line)
		.iter()
		.position(|byte| !matches!(byte, b' ' | b'\t' | b'\r'))
		.map(|offset| line_start + offset)
}

fn fold_anchor_offset(bytes: &[u8], cursor_local_byte: usize) -> usize {
	if bytes.is_empty() {
		return 0;
	}

	let clamped = cursor_local_byte.min(bytes.len().saturating_sub(1));
	let current_line = line_col_from_byte_offset(bytes, DocByte::new(clamped as u64)).0;
	if !is_blank_line(bytes, current_line) && !is_scope_closer_line(bytes, current_line) {
		return clamped;
	}

	let mut search_line = current_line.get();
	while search_line > 0 {
		search_line -= 1;
		let line = DocLine::new(search_line);
		if is_blank_line(bytes, line) || is_scope_closer_line(bytes, line) {
			continue;
		}
		if let Some(anchor) = first_significant_byte_on_line(bytes, line) {
			return anchor;
		}
	}

	clamped
}

fn find_indentation_fold_range(bytes: &[u8], cursor_line: DocLine) -> Option<(usize, usize)> {
	let total_lines = count_newlines(bytes);
	let mut start_line = cursor_line.min(DocLine::new(total_lines));
	while start_line > DocLine::ZERO && is_blank_line(bytes, start_line) {
		start_line -= 1;
	}
	if is_blank_line(bytes, start_line) {
		return None;
	}

	let base_indent = first_non_whitespace_visual_col(bytes, start_line);
	let mut end_line = start_line;
	let mut saw_nested_line = false;

	for line_idx in start_line.get().saturating_add(1)..=total_lines {
		let line = DocLine::new(line_idx);
		if is_blank_line(bytes, line) {
			if saw_nested_line {
				end_line = line;
			}
			continue;
		}

		let indent = first_non_whitespace_visual_col(bytes, line);
		if indent > base_indent {
			saw_nested_line = true;
			end_line = line;
			continue;
		}

		break;
	}

	if !saw_nested_line || end_line == start_line {
		return None;
	}

	Some(line_byte_range(bytes, start_line, end_line))
}

#[derive(Clone)]
struct MaterializeNodeSlice {
	node: NodeId,
	start: usize,
	end: usize,
	bytes: Vec<u8>,
}

struct SyntheticFoldPlan {
	parent: NodeId,
	start_slice: MaterializeNodeSlice,
	start_offset: usize,
	end_slice: MaterializeNodeSlice,
	end_offset: usize,
	metrics: SpanMetrics,
}

fn collect_parent_child_bytes(
	registry: &UastRegistry,
	root: NodeId,
	parent: NodeId,
) -> Result<Vec<MaterializeNodeSlice>, String> {
	let Some(mut child) = registry.get_first_child(parent) else {
		return Ok(Vec::new());
	};
	let mut absolute_start = registry.doc_byte_for_node_offset(root, child, NodeByteOffset::ZERO);
	let mut offset = 0usize;
	let mut slices = Vec::new();

	loop {
		let byte_len = registry.get_total_bytes(child);
		let absolute_end = absolute_start.saturating_add(byte_len);
		let bytes = registry
			.read_loaded_slice(root, absolute_start, absolute_end)
			.map_err(|msg| msg.to_string())?;
		let start = offset;
		offset += bytes.len();
		slices.push(MaterializeNodeSlice {
			node: child,
			start,
			end: offset,
			bytes,
		});

		let Some(next) = registry.get_next_sibling(child) else {
			break;
		};
		child = next;
		absolute_start = absolute_end;
	}

	Ok(slices)
}

fn locate_materialize_start(
	slices: &[MaterializeNodeSlice],
	offset: usize,
) -> Option<(&MaterializeNodeSlice, usize)> {
	slices
		.iter()
		.find(|slice| offset >= slice.start && offset < slice.end)
		.map(|slice| (slice, offset - slice.start))
		.or_else(|| {
			slices
				.iter()
				.find(|slice| offset == slice.start)
				.map(|slice| (slice, 0))
		})
}

fn locate_materialize_end(
	slices: &[MaterializeNodeSlice],
	offset: usize,
) -> Option<(&MaterializeNodeSlice, usize)> {
	slices
		.iter()
		.find(|slice| offset > slice.start && offset <= slice.end)
		.map(|slice| (slice, offset - slice.start))
		.or_else(|| {
			slices
				.windows(2)
				.find(|pair| offset == pair[1].start)
				.map(|pair| (&pair[0], pair[0].bytes.len()))
		})
}

fn materialize_synthetic_fold_plan(
	registry: &UastRegistry,
	plan: SyntheticFoldPlan,
) -> Option<NodeId> {
	let SyntheticFoldPlan {
		parent,
		start_slice,
		start_offset,
		end_slice,
		end_offset,
		metrics,
	} = plan;
	if start_slice.start > end_slice.start
		|| start_offset > start_slice.bytes.len()
		|| end_offset > end_slice.bytes.len()
		|| (start_slice.node == end_slice.node && start_offset >= end_offset)
	{
		return None;
	}

	if start_offset > 0
		&& unsafe {
			(*registry.edges[start_slice.node.index()].get())
				.first_child
				.is_some()
		} {
		return None;
	}
	if end_offset < end_slice.bytes.len()
		&& unsafe {
			(*registry.edges[end_slice.node.index()].get())
				.first_child
				.is_some()
		} {
		return None;
	}

	let prev = registry.get_prev_sibling(start_slice.node);
	let next = registry.get_next_sibling(end_slice.node);
	let old_was_tail =
		unsafe { *registry.child_tails[parent.index()].get() == Some(end_slice.node) };
	let fold_node = registry.alloc_node_internal();
	let prefix = &start_slice.bytes[..start_offset];
	let suffix = &end_slice.bytes[end_offset..];

	let prefix_node = if prefix.is_empty() {
		None
	} else {
		let node = registry.alloc_node_internal();
		init_virtual_token_node(registry, node, parent, Some(fold_node), prefix);
		Some(node)
	};

	let suffix_node = if suffix.is_empty() {
		None
	} else {
		let node = registry.alloc_node_internal();
		init_virtual_token_node(registry, node, parent, next, suffix);
		Some(node)
	};

	let mut first_fold_child: Option<NodeId> = None;
	let mut last_fold_child: Option<NodeId> = None;
	let mut append_child = |child: NodeId| {
		unsafe {
			(*registry.edges[child.index()].get()).parent = Some(fold_node);
			(*registry.edges[child.index()].get()).next_sibling = None;
		}
		if let Some(last) = last_fold_child {
			unsafe {
				(*registry.edges[last.index()].get()).next_sibling = Some(child);
			}
		} else {
			first_fold_child = Some(child);
		}
		last_fold_child = Some(child);
	};

	if start_slice.node == end_slice.node {
		let selected = &start_slice.bytes[start_offset..end_offset];
		if selected.is_empty() {
			return None;
		}
		if start_offset == 0 && end_offset == start_slice.bytes.len() {
			append_child(start_slice.node);
		} else {
			let node = registry.alloc_node_internal();
			init_virtual_token_node(registry, node, fold_node, None, selected);
			append_child(node);
		}
	} else {
		let mut current = registry.get_next_sibling(start_slice.node);
		if start_offset == 0 {
			append_child(start_slice.node);
		} else {
			let node = registry.alloc_node_internal();
			init_virtual_token_node(
				registry,
				node,
				fold_node,
				None,
				&start_slice.bytes[start_offset..],
			);
			append_child(node);
		}

		while let Some(node) = current {
			if node == end_slice.node {
				break;
			}
			current = registry.get_next_sibling(node);
			append_child(node);
		}

		if end_offset == end_slice.bytes.len() {
			append_child(end_slice.node);
		} else {
			let node = registry.alloc_node_internal();
			init_virtual_token_node(
				registry,
				node,
				fold_node,
				None,
				&end_slice.bytes[..end_offset],
			);
			append_child(node);
		}
	}

	let first_fold_child = first_fold_child?;
	init_fold_boundary_node(
		registry,
		fold_node,
		parent,
		suffix_node.or(next),
		first_fold_child,
		metrics,
	);
	unsafe {
		*registry.child_tails[fold_node.index()].get() = last_fold_child;
	}

	let replacement_start = prefix_node.unwrap_or(fold_node);
	let replacement_end = suffix_node.unwrap_or(fold_node);
	unsafe {
		if let Some(prev_sibling) = prev {
			(*registry.edges[prev_sibling.index()].get()).next_sibling = Some(replacement_start);
		} else {
			(*registry.edges[parent.index()].get()).first_child = Some(replacement_start);
		}
		if old_was_tail {
			*registry.child_tails[parent.index()].get() = Some(replacement_end);
		}
	}

	Some(fold_node)
}

fn preview_synthetic_fold_plan_at_cursor(
	registry: &UastRegistry,
	root: NodeId,
	cursor_line: DocLine,
	cursor_col: VisualCol,
) -> Option<SyntheticFoldPlan> {
	let target = registry.find_node_at_line_col_raw(root, cursor_line, cursor_col);
	let parent = unsafe { (*registry.edges[target.node_id.index()].get()).parent }?;
	let slices = collect_parent_child_bytes(registry, root, parent).ok()?;
	let target_slice = slices.iter().find(|slice| slice.node == target.node_id)?;
	let bytes: Vec<u8> = slices
		.iter()
		.flat_map(|slice| slice.bytes.iter().copied())
		.collect();
	let cursor_offset = target_slice.start + target.node_byte.get() as usize;
	let fold_range = find_syntax_fold_range(&bytes, cursor_offset).or_else(|| {
		let local_line = line_col_from_byte_offset(&bytes, DocByte::new(cursor_offset as u64)).0;
		find_indentation_fold_range(&bytes, local_line)
	})?;

	if fold_range.0 == 0
		&& fold_range.1 == bytes.len()
		&& registry.get_prev_sibling(target.node_id).is_none()
		&& registry.get_next_sibling(target.node_id).is_none()
	{
		return None;
	}

	let (start_slice, start_offset) = locate_materialize_start(&slices, fold_range.0)?;
	let (end_slice, end_offset) = locate_materialize_end(&slices, fold_range.1)?;
	let selected = &bytes[fold_range.0..fold_range.1];
	Some(SyntheticFoldPlan {
		parent,
		start_slice: start_slice.clone(),
		start_offset,
		end_slice: end_slice.clone(),
		end_offset,
		metrics: SpanMetrics {
			byte_length: selected.len() as u32,
			newlines: count_newlines(selected),
		},
	})
}

#[cfg(test)]
pub(crate) fn materialize_fold_boundary_at_cursor(
	registry: &UastRegistry,
	root: NodeId,
	cursor_line: DocLine,
	cursor_col: VisualCol,
) -> Option<NodeId> {
	let plan = preview_synthetic_fold_plan_at_cursor(registry, root, cursor_line, cursor_col)?;
	materialize_synthetic_fold_plan(registry, plan)
}

pub(crate) fn resolve_fold_boundary_at_cursor(
	registry: &UastRegistry,
	root: NodeId,
	cursor_node: NodeId,
	cursor_line: DocLine,
	cursor_col: VisualCol,
	allow_materialize: bool,
) -> Option<NodeId> {
	let existing_target = nearest_foldable_boundary(registry, root, cursor_node);
	if !allow_materialize {
		return existing_target;
	}

	let synthetic_plan =
		preview_synthetic_fold_plan_at_cursor(registry, root, cursor_line, cursor_col);
	match (existing_target, synthetic_plan) {
		(Some(existing), Some(plan)) => {
			let existing_len = unsafe { (*registry.metrics[existing.index()].get()).byte_length };
			if plan.metrics.byte_length < existing_len {
				materialize_synthetic_fold_plan(registry, plan).or(Some(existing))
			} else {
				Some(existing)
			}
		}
		(Some(existing), None) => Some(existing),
		(None, Some(plan)) => materialize_synthetic_fold_plan(registry, plan),
		(None, None) => None,
	}
}
