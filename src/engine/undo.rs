use crate::ecs::{NodeId, UastRegistry};
use crate::uast::UastProjection;

#[derive(Debug, Clone)]
pub struct TextDelta {
	pub global_byte_offset: u64,
	pub deleted_text: String,
	pub inserted_text: String,
	pub state_before: u64,
	pub state_after: u64,
}

pub struct UndoLedger {
	undo_stack: Vec<TextDelta>,
	redo_stack: Vec<TextDelta>,
	pub current_state_id: u64,
	pub saved_state_id: u64,
	next_id: u64,
}

impl UndoLedger {
	pub fn new() -> Self {
		Self {
			undo_stack: Vec::new(),
			redo_stack: Vec::new(),
			current_state_id: 0,
			saved_state_id: 0,
			next_id: 1,
		}
	}

	/// Allocate a new state ID for a forward mutation.
	/// Sets `state_before`/`state_after` on the delta and advances `current_state_id`.
	pub fn push(&mut self, mut delta: TextDelta) {
		delta.state_before = self.current_state_id;
		delta.state_after = self.next_id;
		self.current_state_id = self.next_id;
		self.next_id += 1;
		self.undo_stack.push(delta);
		self.redo_stack.clear();
	}

	pub fn clear(&mut self) {
		self.undo_stack.clear();
		self.redo_stack.clear();
		self.current_state_id = 0;
		self.saved_state_id = 0;
		self.next_id = 1;
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
	) -> Option<(NodeId, NodeId, u64, u64)> {
		let delta = self.undo_stack.pop()?;
		let bytes = registry.collect_document_bytes(root).ok()?;

		let offset = delta.global_byte_offset as usize;
		let insert_len = delta.inserted_text.len();
		let mut new_bytes = Vec::with_capacity(bytes.len() - insert_len + delta.deleted_text.len());

		// Inverse: remove what was inserted, re-insert what was deleted.
		new_bytes.extend_from_slice(&bytes[..offset]);
		new_bytes.extend_from_slice(delta.deleted_text.as_bytes());
		new_bytes.extend_from_slice(&bytes[offset + insert_len..]);

		let new_size = new_bytes.len() as u64;
		let (new_root, new_leaf) = create_document(registry, &new_bytes);
		let cursor_byte = delta.global_byte_offset + delta.deleted_text.len() as u64;

		self.current_state_id = delta.state_before;
		self.redo_stack.push(delta);
		Some((new_root, new_leaf, cursor_byte, new_size))
	}

	/// Pop the most recent undone delta, re-apply it forward,
	/// and return (root, leaf, cursor_global_byte, new_doc_size).
	pub fn redo(
		&mut self,
		registry: &UastRegistry,
		root: NodeId,
	) -> Option<(NodeId, NodeId, u64, u64)> {
		let delta = self.redo_stack.pop()?;
		let bytes = registry.collect_document_bytes(root).ok()?;

		let offset = delta.global_byte_offset as usize;
		let delete_len = delta.deleted_text.len();
		let mut new_bytes = Vec::with_capacity(bytes.len() - delete_len + delta.inserted_text.len());

		// Forward: remove deleted_text, insert inserted_text.
		new_bytes.extend_from_slice(&bytes[..offset]);
		new_bytes.extend_from_slice(delta.inserted_text.as_bytes());
		new_bytes.extend_from_slice(&bytes[offset + delete_len..]);

		let new_size = new_bytes.len() as u64;
		let (new_root, new_leaf) = create_document(registry, &new_bytes);
		let cursor_byte = delta.global_byte_offset + delta.inserted_text.len() as u64;

		self.current_state_id = delta.state_after;
		self.undo_stack.push(delta);
		Some((new_root, new_leaf, cursor_byte, new_size))
	}
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
pub fn line_col_from_byte_offset(doc: &[u8], byte_offset: u64) -> (u32, u32) {
	let mut line = 0u32;
	let mut col = 0u32;
	for (i, &b) in doc.iter().enumerate() {
		if i as u64 >= byte_offset {
			break;
		}
		if b == b'\n' {
			line += 1;
			col = 0;
		} else if b == b'\t' {
			col += 4 - (col % 4);
		} else {
			col += 1;
		}
	}
	(line, col)
}

/// Compute global byte offset from (line, col) in raw document bytes.
pub fn byte_offset_from_line_col(doc: &[u8], target_line: u32, target_col: u32) -> u64 {
	let mut line = 0u32;
	let mut col = 0u32;
	for (i, &b) in doc.iter().enumerate() {
		if line == target_line && col >= target_col {
			return i as u64;
		}
		if b == b'\n' {
			if line == target_line {
				// Cursor is past end of this line; clamp to newline position.
				return i as u64;
			}
			line += 1;
			col = 0;
		} else if b == b'\t' {
			col += 4 - (col % 4);
		} else {
			col += 1;
		}
	}
	doc.len() as u64
}
