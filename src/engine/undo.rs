use crate::core::{DocByte, DocLine, StateId, TAB_SIZE, VisualCol};

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

	/// Pop the most recent transaction group and return its inverse deltas in the
	/// correct sparse replay order, along with the cursor target byte.
	pub fn undo(&mut self) -> Option<(Vec<TextDelta>, DocByte)> {
		let deltas = self.undo_stack.pop()?;
		let last_delta = deltas.last()?;
		let cursor_byte = last_delta
			.global_byte_offset
			.saturating_add(last_delta.deleted_text.len() as u64);

		self.current_state_id = last_delta.state_before;
		let inverse_deltas = deltas
			.iter()
			.rev()
			.map(|delta| TextDelta {
				global_byte_offset: delta.global_byte_offset,
				deleted_text: delta.inserted_text.clone(),
				inserted_text: delta.deleted_text.clone(),
				state_before: StateId::ZERO,
				state_after: StateId::ZERO,
			})
			.collect();
		self.redo_stack.push(deltas);
		Some((inverse_deltas, cursor_byte))
	}

	/// Pop the most recent undone transaction group and return its forward deltas
	/// for sparse replay, along with the cursor target byte.
	pub fn redo(&mut self) -> Option<(Vec<TextDelta>, DocByte)> {
		let deltas = self.redo_stack.pop()?;
		let last_delta = deltas.last()?;
		let cursor_byte = last_delta
			.global_byte_offset
			.saturating_add(last_delta.inserted_text.len() as u64);

		self.current_state_id = last_delta.state_after;
		self.undo_stack.push(deltas.clone());
		Some((deltas, cursor_byte))
	}
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
