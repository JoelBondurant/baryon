use super::Frontend;
use crate::engine::{EditorCommand, EditorMode, MoveDirection};
use crossterm::event::{self, KeyCode};
use ratatui::backend::Backend;
use std::io;

impl<B: Backend + io::Write> Frontend<B> {
	pub(super) fn handle_insert_key(
		&mut self,
		code: KeyCode,
		modifiers: event::KeyModifiers,
		should_quit: &mut bool,
	) -> bool {
		match code {
			KeyCode::Char('z') if modifiers.contains(event::KeyModifiers::CONTROL) => {
				let _ = self.tx_cmd.send(EditorCommand::Quit);
				*should_quit = true;
				return true;
			}
			KeyCode::Esc => {
				self.current_mode = EditorMode::Normal;
				self.apply_cursor_style();
			}
			KeyCode::Enter => {
				let _ = self.tx_cmd.send(EditorCommand::InsertChar('\n'));
			}
			KeyCode::Tab => {
				let _ = self.tx_cmd.send(EditorCommand::InsertChar('\t'));
			}
			KeyCode::Backspace => {
				let _ = self.tx_cmd.send(EditorCommand::Backspace);
			}
			KeyCode::Delete => {
				let _ = self.tx_cmd.send(EditorCommand::Delete);
			}
			KeyCode::Up => {
				let _ = self
					.tx_cmd
					.send(EditorCommand::MoveCursor(MoveDirection::Up));
			}
			KeyCode::Down => {
				let _ = self
					.tx_cmd
					.send(EditorCommand::MoveCursor(MoveDirection::Down));
			}
			KeyCode::Left => {
				let _ = self
					.tx_cmd
					.send(EditorCommand::MoveCursor(MoveDirection::Left));
			}
			KeyCode::Right => {
				let _ = self
					.tx_cmd
					.send(EditorCommand::MoveCursor(MoveDirection::Right));
			}
			KeyCode::Home => {
				let _ = self.tx_cmd.send(EditorCommand::SmartHome);
			}
			KeyCode::End => {
				let _ = self.tx_cmd.send(EditorCommand::LineEnd);
			}
			KeyCode::PageUp => {
				let _ = self.tx_cmd.send(EditorCommand::PageUp);
			}
			KeyCode::PageDown => {
				let _ = self.tx_cmd.send(EditorCommand::PageDown);
			}
			KeyCode::Char(c) => {
				let _ = self.tx_cmd.send(EditorCommand::InsertChar(c));
			}
			_ => {}
		}
		false
	}
}
