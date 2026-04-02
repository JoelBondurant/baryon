use super::Frontend;
use crate::engine::{EditorCommand, EditorMode, MoveDirection};
use crossterm::event::{self, KeyCode};
use ratatui::backend::Backend;
use std::io;

impl<B: Backend + io::Write> Frontend<B> {
	pub(super) fn handle_visual_key(
		&mut self,
		code: KeyCode,
		modifiers: event::KeyModifiers,
		should_quit: &mut bool,
	) -> bool {
		let (anchor, kind) = match self.current_mode {
			EditorMode::Visual { anchor, kind } => (anchor, kind),
			_ => return false,
		};

		if self.g_prefix {
			if let KeyCode::Char('g') = code {
				let _ = self
					.tx_cmd
					.send(EditorCommand::MoveCursor(MoveDirection::Top));
			}
			self.g_prefix = false;
			return false;
		}

		match code {
			KeyCode::Char('z') if modifiers.contains(event::KeyModifiers::CONTROL) => {
				let _ = self.tx_cmd.send(EditorCommand::Quit);
				*should_quit = true;
				return true;
			}
			KeyCode::Esc => {
				self.clear_prefixes();
				self.current_mode = EditorMode::Normal;
				let _ = self.tx_cmd.send(EditorCommand::ClearVisualSelection);
				self.apply_cursor_style();
			}
			KeyCode::Char('y') => {
				self.clear_prefixes();
				let _ = self.tx_cmd.send(EditorCommand::VisualYank { anchor, kind });
				self.current_mode = EditorMode::Normal;
				self.apply_cursor_style();
			}
			KeyCode::Char('d') => {
				self.clear_prefixes();
				let _ = self
					.tx_cmd
					.send(EditorCommand::VisualDelete { anchor, kind });
				self.current_mode = EditorMode::Normal;
				self.apply_cursor_style();
			}
			KeyCode::Char('c') => {
				self.clear_prefixes();
				let _ = self
					.tx_cmd
					.send(EditorCommand::VisualChange { anchor, kind });
				self.current_mode = EditorMode::Normal;
				self.apply_cursor_style();
			}
			KeyCode::Char('h') | KeyCode::Left => {
				let _ = self
					.tx_cmd
					.send(EditorCommand::MoveCursor(MoveDirection::Left));
			}
			KeyCode::Char('j') | KeyCode::Down => {
				let _ = self
					.tx_cmd
					.send(EditorCommand::MoveCursor(MoveDirection::Down));
			}
			KeyCode::Char('k') | KeyCode::Up => {
				let _ = self
					.tx_cmd
					.send(EditorCommand::MoveCursor(MoveDirection::Up));
			}
			KeyCode::Char('l') | KeyCode::Right => {
				let _ = self
					.tx_cmd
					.send(EditorCommand::MoveCursor(MoveDirection::Right));
			}
			KeyCode::Char('w') => {
				let _ = self
					.tx_cmd
					.send(EditorCommand::MoveCursor(MoveDirection::NextWord));
			}
			KeyCode::Char('b') => {
				let _ = self
					.tx_cmd
					.send(EditorCommand::MoveCursor(MoveDirection::PrevWord));
			}
			KeyCode::Char('e') => {
				let _ = self
					.tx_cmd
					.send(EditorCommand::MoveCursor(MoveDirection::NextWordEnd));
			}
			KeyCode::Char('0') => {
				let _ = self.tx_cmd.send(EditorCommand::LineStart);
			}
			KeyCode::Char('^') => {
				let _ = self.tx_cmd.send(EditorCommand::FirstNonWhitespace);
			}
			KeyCode::Char('$') | KeyCode::End => {
				let _ = self.tx_cmd.send(EditorCommand::LineEnd);
			}
			KeyCode::Home => {
				let _ = self.tx_cmd.send(EditorCommand::SmartHome);
			}
			KeyCode::PageUp => {
				let _ = self.tx_cmd.send(EditorCommand::PageUp);
			}
			KeyCode::PageDown => {
				let _ = self.tx_cmd.send(EditorCommand::PageDown);
			}
			KeyCode::Char('g') => {
				self.g_prefix = true;
			}
			KeyCode::Char('G') => {
				let _ = self
					.tx_cmd
					.send(EditorCommand::MoveCursor(MoveDirection::Bottom));
			}
			_ => {}
		}

		false
	}
}
