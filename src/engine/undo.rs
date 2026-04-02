use crate::core::{DocByte, DocLine, StateId, TAB_SIZE, VisualCol};
use crate::ecs::{NodeId, UastRegistry};
use crate::uast::UastProjection;

#[derive(Debug, Clone)]
pub struct TextDelta {
	pub global_byte_offset: DocByte,
	pub deleted_text: String,
	pub inserted_text: String,
	pub state_before: StateId,
	pub state_after: StateId,
}

pub struct UndoLedger {
	undo_stack: Vec<Vec<TextDelta>>,
	redo_stack: Vec<Vec<TextDelta>>,
	pub current_state_id: StateId,
	pub saved_state_id: StateId,
	next_state_id: StateId,
}

impl UndoLedger {
	pub fn new() -> Self {
		Self {
			undo_stack: Vec::new(),
			redo_stack: Vec::new(),
			current_state_id: StateId::ZERO,
			saved_state_id: StateId::ZERO,
			next_state_id: StateId::new(1),
		}
	}

	/// Allocate a single new state ID for a forward transaction group.
	/// Sets `state_before`/`state_after` on every delta in the group and advances
	/// `current_state_id` once for the whole transaction.
	pub fn push_group(&mut self, mut deltas: Vec<TextDelta>) {
		if deltas.is_empty() {
			return;
		}

		let state_before = self.current_state_id;
		let state_after = self.next_state_id;
		for delta in &mut deltas {
			delta.state_before = state_before;
			delta.state_after = state_after;
		}

		self.current_state_id = state_after;
		self.next_state_id += 1;
		self.undo_stack.push(deltas);
		self.redo_stack.clear();
	}

	pub fn clear(&mut self) {
		self.undo_stack.clear();
		self.redo_stack.clear();
		self.current_state_id = StateId::ZERO;
		self.saved_state_id = StateId::ZERO;
		self.next_state_id = StateId::new(1);
	}

	/// Mark the current state as the saved-to-disk state.
	pub fn mark_saved(&mut self) {
		self.saved_state_id = self.current_state_id;
	}

	/// True if the document has been modified since the last save.
	pub fn is_dirty(&self) -> bool {
		self.current_state_id != self.saved_state_id
	}

	/// Pop the most recent delta, apply its inverse to the document,
	/// and return (root, leaf, cursor_global_byte, new_doc_size).
	pub fn undo(
		&mut self,
		registry: &UastRegistry,
		root: NodeId,
	) -> Option<(NodeId, NodeId, DocByte, u64, Vec<TextDelta>)> {
		let deltas = self.undo_stack.pop()?;
		let bytes = registry.collect_document_bytes(root).ok()?;
		let mut new_bytes = bytes;
		for delta in deltas.iter().rev() {
			splice_document_bytes(
				&mut new_bytes,
				delta.global_byte_offset,
				delta.inserted_text.as_bytes(),
				delta.deleted_text.as_bytes(),
			)?;
		}
		let new_size = new_bytes.len() as u64;
		let (new_root, new_leaf) = create_document(registry, &new_bytes);
		let last_delta = deltas.last()?;
		let cursor_byte = last_delta
			.global_byte_offset
			.saturating_add(last_delta.deleted_text.len() as u64);

		self.current_state_id = last_delta.state_before;
		let cache_deltas = deltas.clone();
		self.redo_stack.push(deltas);
		Some((new_root, new_leaf, cursor_byte, new_size, cache_deltas))
	}

	/// Pop the most recent undone delta, re-apply it forward,
	/// and return (root, leaf, cursor_global_byte, new_doc_size).
	pub fn redo(
		&mut self,
		registry: &UastRegistry,
		root: NodeId,
	) -> Option<(NodeId, NodeId, DocByte, u64, Vec<TextDelta>)> {
		let deltas = self.redo_stack.pop()?;
		let bytes = registry.collect_document_bytes(root).ok()?;
		let mut new_bytes = bytes;
		for delta in &deltas {
			splice_document_bytes(
				&mut new_bytes,
				delta.global_byte_offset,
				delta.deleted_text.as_bytes(),
				delta.inserted_text.as_bytes(),
			)?;
		}
		let new_size = new_bytes.len() as u64;
		let (new_root, new_leaf) = create_document(registry, &new_bytes);
		let last_delta = deltas.last()?;
		let cursor_byte = last_delta
			.global_byte_offset
			.saturating_add(last_delta.inserted_text.len() as u64);

		self.current_state_id = last_delta.state_after;
		let cache_deltas = deltas.clone();
		self.undo_stack.push(deltas);
		Some((new_root, new_leaf, cursor_byte, new_size, cache_deltas))
	}
}

fn splice_document_bytes(
	bytes: &mut Vec<u8>,
	global_byte_offset: DocByte,
	expected_deleted: &[u8],
	inserted: &[u8],
) -> Option<()> {
	let start = global_byte_offset.get() as usize;
	let end = start.checked_add(expected_deleted.len())?;
	if end > bytes.len() || bytes[start..end] != expected_deleted[..] {
		return None;
	}

	let mut new_bytes = Vec::with_capacity(bytes.len() - expected_deleted.len() + inserted.len());
	new_bytes.extend_from_slice(&bytes[..start]);
	new_bytes.extend_from_slice(inserted);
	new_bytes.extend_from_slice(&bytes[end..]);
	*bytes = new_bytes;
	Some(())
}

/// Reconstruct a single-leaf document from raw bytes.
fn create_document(registry: &UastRegistry, bytes: &[u8]) -> (NodeId, NodeId) {
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

/// Compute (line, col) from a global byte offset into raw document bytes.
pub fn line_col_from_byte_offset(doc: &[u8], byte_offset: DocByte) -> (DocLine, VisualCol) {
	let mut line = DocLine::ZERO;
	let mut col = VisualCol::ZERO;
	for (i, &b) in doc.iter().enumerate() {
		if i as u64 >= byte_offset.get() {
			break;
		}
		if b == b'\n' {
			line += 1;
			col = VisualCol::ZERO;
		} else if b == b'\t' {
			col += TAB_SIZE - (col.get() % TAB_SIZE);
		} else {
			col += 1;
		}
	}
	(line, col)
}

/// Compute global byte offset from (line, col) in raw document bytes.
pub fn byte_offset_from_line_col(
	doc: &[u8],
	target_line: DocLine,
	target_col: VisualCol,
) -> DocByte {
	let mut line = DocLine::ZERO;
	let mut col = VisualCol::ZERO;
	for (i, &b) in doc.iter().enumerate() {
		if line == target_line && col >= target_col {
			return DocByte::new(i as u64);
		}
		if b == b'\n' {
			if line == target_line {
				// Cursor is past end of this line; clamp to newline position.
				return DocByte::new(i as u64);
			}
			line += 1;
			col = VisualCol::ZERO;
		} else if b == b'\t' {
			col += TAB_SIZE - (col.get() % TAB_SIZE);
		} else {
			col += 1;
		}
	}
	DocByte::new(doc.len() as u64)
}
