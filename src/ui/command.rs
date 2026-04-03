use super::Frontend;
use crate::core::DocLine;
use crate::engine::{ConfirmAction, EditorCommand, EditorMode, SubstituteFlags, SubstituteRange};
use crossterm::event::{self, KeyCode};
use ratatui::backend::Backend;
use std::io;

impl<B: Backend + io::Write> Frontend<B> {
	pub(super) fn handle_command_key(
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
			KeyCode::Enter => {
				if self.command_buffer == "w" {
					let _ = self.tx_cmd.send(EditorCommand::WriteFile);
				} else if self.command_buffer.starts_with("w ") {
					let path = self.command_buffer[2..].trim();
					let expanded = crate::core::path::expand_path(path);
					let _ = self.tx_cmd.send(EditorCommand::WriteFileAs(
						expanded.to_string_lossy().to_string(),
					));
				} else if self.command_buffer == "x" || self.command_buffer == "wq" {
					let _ = self.tx_cmd.send(EditorCommand::WriteAndQuit);
				} else if self.command_buffer.starts_with("e ") {
					let path = self.command_buffer[2..].trim();
					let expanded = crate::core::path::expand_path(path);
					let _ = self.tx_cmd.send(EditorCommand::LoadFile(
						expanded.to_string_lossy().to_string(),
					));
				} else if self.command_buffer.starts_with("theme ") {
					let name = self.command_buffer[6..].trim().to_string();
					let _ = self.tx_cmd.send(EditorCommand::SetTheme(name));
				} else if let Ok(line_num) = self.command_buffer.parse::<u32>() {
					let _ = self.tx_cmd.send(EditorCommand::GotoLine(DocLine::new(
						line_num.saturating_sub(1),
					)));
				} else if self.command_buffer == "q" {
					let _ = self.tx_cmd.send(EditorCommand::Quit);
					*should_quit = true;
					return true;
				} else if self.command_buffer.contains("s/") {
					let cursor_line = self
						.current_viewport
						.as_ref()
						.map(|v| v.cursor_abs_pos.line)
						.unwrap_or(DocLine::ZERO);
					if let Some((pattern, replacement, flags, range)) =
						parse_substitute(&self.command_buffer, cursor_line)
					{
						if flags.confirm {
							let _ = self.tx_cmd.send(EditorCommand::SubstituteConfirm {
								pattern,
								replacement,
								range,
								flags,
							});
						} else {
							let _ = self.tx_cmd.send(EditorCommand::SubstituteAll {
								pattern,
								replacement,
								range,
								flags,
							});
						}
					} else {
						self.status_message = Some("Invalid substitution syntax".to_string());
					}
				} else if !self.command_buffer.is_empty() {
					self.status_message = Some(format!("Unknown command: {}", self.command_buffer));
				}
				self.current_mode = EditorMode::Normal;
			}
			KeyCode::Esc => {
				self.current_mode = EditorMode::Normal;
			}
			KeyCode::Backspace => {
				if self.command_buffer.is_empty() {
					self.current_mode = EditorMode::Normal;
				} else {
					self.command_buffer.pop();
				}
			}
			KeyCode::Char(c) => {
				self.command_buffer.push(c);
			}
			_ => {}
		}
		false
	}

	pub(super) fn handle_search_key(
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
			KeyCode::Enter => {
				if !self.search_buffer.is_empty() {
					let _ = self
						.tx_cmd
						.send(EditorCommand::SearchStart(self.search_buffer.clone()));
				}
				self.current_mode = EditorMode::Normal;
			}
			KeyCode::Esc => {
				self.current_mode = EditorMode::Normal;
			}
			KeyCode::Backspace => {
				if self.search_buffer.is_empty() {
					self.current_mode = EditorMode::Normal;
				} else {
					self.search_buffer.pop();
				}
			}
			KeyCode::Char(c) => {
				self.search_buffer.push(c);
			}
			_ => {}
		}
		false
	}

	pub(super) fn handle_confirm_key(
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
			KeyCode::Char('y') => {
				let _ = self
					.tx_cmd
					.send(EditorCommand::ConfirmResponse(ConfirmAction::Yes));
			}
			KeyCode::Char('n') => {
				let _ = self
					.tx_cmd
					.send(EditorCommand::ConfirmResponse(ConfirmAction::No));
			}
			KeyCode::Char('a') => {
				let _ = self
					.tx_cmd
					.send(EditorCommand::ConfirmResponse(ConfirmAction::All));
			}
			KeyCode::Char('q') | KeyCode::Esc => {
				let _ = self
					.tx_cmd
					.send(EditorCommand::ConfirmResponse(ConfirmAction::Quit));
			}
			_ => {}
		}
		false
	}
}

fn parse_substitute(
	cmd: &str,
	cursor_line: DocLine,
) -> Option<(String, String, SubstituteFlags, SubstituteRange)> {
	let s_pos = cmd.find("s/")?;
	let range_str = &cmd[..s_pos];
	let rest = &cmd[s_pos + 2..];

	let range = if range_str == "%" {
		SubstituteRange::WholeFile
	} else if range_str.is_empty() || range_str == "." {
		SubstituteRange::CurrentLine
	} else if let Some(offset_str) = range_str.strip_prefix(".,+") {
		let n: u32 = offset_str.parse().ok()?;
		SubstituteRange::LineRange(cursor_line, cursor_line + n)
	} else if let Some((a_str, b_str)) = range_str.split_once(',') {
		let a: u32 = a_str.parse::<u32>().ok()?.saturating_sub(1);
		let b: u32 = b_str.parse::<u32>().ok()?.saturating_sub(1);
		SubstituteRange::LineRange(DocLine::new(a), DocLine::new(b))
	} else if let Ok(n) = range_str.parse::<u32>() {
		SubstituteRange::SingleLine(DocLine::new(n.saturating_sub(1)))
	} else {
		return None;
	};

	let parts: Vec<&str> = rest.splitn(3, '/').collect();
	if parts.len() < 2 {
		return None;
	}
	let pattern = parts[0].to_string();
	let replacement = parts[1].to_string();
	let flags_str = parts.get(2).unwrap_or(&"");
	if pattern.is_empty() {
		return None;
	}

	let mut flags = SubstituteFlags::default();
	for c in flags_str.chars() {
		match c {
			'g' => flags.global = true,
			'c' => flags.confirm = true,
			'i' => flags.case_insensitive = true,
			'I' => flags.case_insensitive = false,
			_ => {}
		}
	}

	Some((pattern, replacement, flags, range))
}
