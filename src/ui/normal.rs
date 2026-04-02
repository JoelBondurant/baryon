use super::{Frontend, PendingOperator};
use crate::engine::{EditorCommand, EditorMode, MoveDirection, VisualKind};
use crossterm::event::{self, KeyCode};
use ratatui::backend::Backend;
use std::io;

impl<B: Backend + io::Write> Frontend<B> {
	pub(super) fn handle_normal_key(
		&mut self,
		code: KeyCode,
		modifiers: event::KeyModifiers,
		should_quit: &mut bool,
	) -> bool {
		if self.pending_register == Some('\0') {
			if let KeyCode::Char(c) = code {
				self.pending_register = Some(c);
				return false;
			} else {
				self.clear_prefixes();
				return false;
			}
		}

		if let Some(operator) = self.pending_operator {
			if self.awaiting_inner_word {
				if let KeyCode::Char('w') = code {
					let command = match operator {
						PendingOperator::Delete => EditorCommand::DeleteInnerWord,
						PendingOperator::Change => EditorCommand::ChangeInnerWord,
					};
					let _ = self.tx_cmd.send(command);
					self.clear_prefixes();
					return false;
				}

				self.clear_operator_pending();
			} else if let KeyCode::Char('i') = code {
				self.awaiting_inner_word = true;
				return false;
			} else {
				self.clear_operator_pending();
			}
		}

		let register = self.pending_register.unwrap_or('"');

		match code {
			KeyCode::Char('z') if modifiers.contains(event::KeyModifiers::CONTROL) => {
				let _ = self.tx_cmd.send(EditorCommand::Quit);
				*should_quit = true;
				return true;
			}
			KeyCode::Char('"') => {
				self.pending_register = Some('\0');
				return false;
			}
			KeyCode::Char(':') => {
				self.current_mode = EditorMode::Command;
				self.command_buffer.clear();
				self.clear_prefixes();
			}
			KeyCode::Char('h') => {
				let _ = self
					.tx_cmd
					.send(EditorCommand::MoveCursor(MoveDirection::Left));
				self.clear_prefixes();
			}
			KeyCode::Char('j') => {
				let _ = self
					.tx_cmd
					.send(EditorCommand::MoveCursor(MoveDirection::Down));
				self.clear_prefixes();
			}
			KeyCode::Char('k') => {
				let _ = self
					.tx_cmd
					.send(EditorCommand::MoveCursor(MoveDirection::Up));
				self.clear_prefixes();
			}
			KeyCode::Char('l') => {
				let _ = self
					.tx_cmd
					.send(EditorCommand::MoveCursor(MoveDirection::Right));
				self.clear_prefixes();
			}
			KeyCode::Char('w') => {
				let _ = self
					.tx_cmd
					.send(EditorCommand::MoveCursor(MoveDirection::NextWord));
				self.clear_prefixes();
			}
			KeyCode::Char('b') => {
				let _ = self
					.tx_cmd
					.send(EditorCommand::MoveCursor(MoveDirection::PrevWord));
				self.clear_prefixes();
			}
			KeyCode::Char('e') => {
				let _ = self
					.tx_cmd
					.send(EditorCommand::MoveCursor(MoveDirection::NextWordEnd));
				self.clear_prefixes();
			}
			KeyCode::Char('0') => {
				let _ = self.tx_cmd.send(EditorCommand::LineStart);
				self.clear_prefixes();
			}
			KeyCode::Char('^') => {
				let _ = self.tx_cmd.send(EditorCommand::FirstNonWhitespace);
				self.clear_prefixes();
			}
			KeyCode::Char('$') => {
				let _ = self.tx_cmd.send(EditorCommand::LineEnd);
				self.clear_prefixes();
			}
			KeyCode::Char('g') => {
				if self.g_prefix {
					let _ = self
						.tx_cmd
						.send(EditorCommand::MoveCursor(MoveDirection::Top));
					self.clear_prefixes();
				} else {
					self.g_prefix = true;
					self.y_prefix = false;
					self.pending_register = None;
				}
			}
			KeyCode::Char('G') => {
				let _ = self
					.tx_cmd
					.send(EditorCommand::MoveCursor(MoveDirection::Bottom));
				self.clear_prefixes();
			}
			KeyCode::Char('d') => {
				self.clear_prefixes();
				self.pending_operator = Some(PendingOperator::Delete);
			}
			KeyCode::Char('c') => {
				self.clear_prefixes();
				self.pending_operator = Some(PendingOperator::Change);
			}
			KeyCode::Char('D') => {
				let _ = self.tx_cmd.send(EditorCommand::DeleteToLineEnd);
				self.clear_prefixes();
			}
			KeyCode::Char('v') if modifiers.contains(event::KeyModifiers::CONTROL) => {
				if let Some(view) = &self.current_viewport {
					let anchor = view.cursor_abs_byte;
					let kind = VisualKind::Block;
					self.current_mode = EditorMode::Visual { anchor, kind };
					let _ = self
						.tx_cmd
						.send(EditorCommand::SetVisualSelection { anchor, kind });
					self.clear_prefixes();
					self.apply_cursor_style();
				}
			}
			KeyCode::Char('v') => {
				if let Some(view) = &self.current_viewport {
					let anchor = view.cursor_abs_byte;
					let kind = VisualKind::Char;
					self.current_mode = EditorMode::Visual { anchor, kind };
					let _ = self
						.tx_cmd
						.send(EditorCommand::SetVisualSelection { anchor, kind });
					self.clear_prefixes();
					self.apply_cursor_style();
				}
			}
			KeyCode::Char('V') => {
				if let Some(view) = &self.current_viewport {
					let anchor = view.cursor_line_start_byte;
					let kind = VisualKind::Line;
					self.current_mode = EditorMode::Visual { anchor, kind };
					let _ = self
						.tx_cmd
						.send(EditorCommand::SetVisualSelection { anchor, kind });
					self.clear_prefixes();
					self.apply_cursor_style();
				}
			}
			KeyCode::Char('y') => {
				if self.y_prefix {
					let _ = self.tx_cmd.send(EditorCommand::YankLine { register });
					self.clear_prefixes();
				} else {
					self.y_prefix = true;
					self.g_prefix = false;
				}
			}
			KeyCode::Char('p') => {
				let _ = self.tx_cmd.send(EditorCommand::Put { register });
				self.clear_prefixes();
			}
			KeyCode::Char('u') => {
				let _ = self.tx_cmd.send(EditorCommand::Undo);
				self.clear_prefixes();
			}
			KeyCode::Char('r') if modifiers.contains(event::KeyModifiers::CONTROL) => {
				let _ = self.tx_cmd.send(EditorCommand::Redo);
				self.clear_prefixes();
			}
			KeyCode::Char('i') => {
				self.current_mode = EditorMode::Insert;
				self.clear_prefixes();
				self.apply_cursor_style();
			}
			KeyCode::Char('/') => {
				self.current_mode = EditorMode::Search;
				self.search_buffer.clear();
				self.clear_prefixes();
			}
			KeyCode::Char('n') => {
				let _ = self.tx_cmd.send(EditorCommand::SearchNext);
				self.clear_prefixes();
			}
			KeyCode::Char('N') => {
				let _ = self.tx_cmd.send(EditorCommand::SearchPrev);
				self.clear_prefixes();
			}
			KeyCode::Backspace | KeyCode::Delete => {
				let _ = self.tx_cmd.send(EditorCommand::Backspace);
				self.clear_prefixes();
			}
			KeyCode::Up => {
				let _ = self
					.tx_cmd
					.send(EditorCommand::MoveCursor(MoveDirection::Up));
				self.clear_prefixes();
			}
			KeyCode::Down => {
				let _ = self
					.tx_cmd
					.send(EditorCommand::MoveCursor(MoveDirection::Down));
				self.clear_prefixes();
			}
			KeyCode::Left => {
				let _ = self
					.tx_cmd
					.send(EditorCommand::MoveCursor(MoveDirection::Left));
				self.clear_prefixes();
			}
			KeyCode::Right => {
				let _ = self
					.tx_cmd
					.send(EditorCommand::MoveCursor(MoveDirection::Right));
				self.clear_prefixes();
			}
			KeyCode::Home => {
				let _ = self.tx_cmd.send(EditorCommand::SmartHome);
				self.clear_prefixes();
			}
			KeyCode::End => {
				let _ = self.tx_cmd.send(EditorCommand::LineEnd);
				self.clear_prefixes();
			}
			KeyCode::PageUp => {
				let _ = self.tx_cmd.send(EditorCommand::PageUp);
				self.clear_prefixes();
			}
			KeyCode::PageDown => {
				let _ = self.tx_cmd.send(EditorCommand::PageDown);
				self.clear_prefixes();
			}
			KeyCode::Esc => {
				self.clear_prefixes();
			}
			_ => {
				self.clear_prefixes();
			}
		}
		false
	}
}
