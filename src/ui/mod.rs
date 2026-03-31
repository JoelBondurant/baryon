use crate::uast::kind::SemanticKind;
use crate::uast::projection::Viewport;
use crate::engine::{EditorMode, EditorCommand, MoveDirection};
use crossterm::cursor::SetCursorStyle;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
	backend::Backend,
	style::{Color, Modifier, Style},
	Terminal,
};
use crossterm::execute;
use std::io;
use std::sync::mpsc;
use std::time::Duration;

pub struct Frontend<B: Backend + io::Write> {
	terminal: Terminal<B>,
	tx_cmd: mpsc::Sender<EditorCommand>,
	rx_view: mpsc::Receiver<Viewport>,
	current_viewport: Option<Viewport>,
	current_mode: EditorMode,
	command_buffer: String,
	g_prefix: bool,
	status_message: Option<String>,
	needs_redraw: bool,
}

impl<B: Backend + io::Write> Frontend<B> {
	pub fn new(
		terminal: Terminal<B>,
		tx_cmd: mpsc::Sender<EditorCommand>,
		rx_view: mpsc::Receiver<Viewport>,
	) -> Self {
		Self {
			terminal,
			tx_cmd,
			rx_view,
			current_viewport: None,
			current_mode: EditorMode::Normal,
			command_buffer: String::new(),
			g_prefix: false,
			status_message: None,
			needs_redraw: false,
		}
	}

	pub fn run(&mut self) -> Result<(), io::Error> {
		self.apply_cursor_style();
		let mut initial_draw = true;
		loop {
			let mut got_new_view = initial_draw;
			initial_draw = false;
			let mut should_quit = false;

			while let Ok(view) = self.rx_view.try_recv() {
				if view.should_quit {
					should_quit = true;
				}
				if let Some(msg) = view.status_message.clone() {
					self.status_message = Some(msg);
				}
				self.current_viewport = Some(view);
				got_new_view = true;
			}

			if got_new_view || self.needs_redraw || self.current_mode == EditorMode::Command || self.current_mode == EditorMode::Insert {
				self.needs_redraw = false;
				self.draw().map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
			}
			if event::poll(Duration::from_millis(16))? {
				loop {
					if let Event::Key(key) = event::read()? {
						if key.kind == KeyEventKind::Press {
							self.status_message = None;
							self.needs_redraw = true;
							match self.current_mode {
								EditorMode::Normal => {
									if self.handle_normal_key(key.code, key.modifiers, &mut should_quit) {
										break;
									}
								}
								EditorMode::Insert => {
									if self.handle_insert_key(key.code, key.modifiers, &mut should_quit) {
										break;
									}
								}
								EditorMode::Command => {
									if self.handle_command_key(key.code, key.modifiers, &mut should_quit) {
										break;
									}
								}
							}
						}
					}
					if should_quit || !event::poll(Duration::from_millis(0))? {
						break;
					}
				}
			}

			if should_quit {
				break;
			}
		}
		Ok(())
	}

	fn draw(&mut self) -> Result<(), B::Error> {
		let current_viewport = &self.current_viewport;
		let current_mode = &self.current_mode;
		let status_message = &self.status_message;
		let command_buffer = &self.command_buffer;
		let g_prefix = self.g_prefix;

		self.terminal.draw(|f| {
			let mut cursor_to_set = None;
			let buf = f.buffer_mut();
			let max_width = buf.area.width;
			let max_height = buf.area.height;

			if let Some(view) = current_viewport {
				let scroll_y = view.cursor_abs_pos.0.saturating_sub(20);
				let max_line_on_screen = scroll_y + max_height.saturating_sub(1) as u32;

				let digits = max_line_on_screen.max(1).ilog10() + 1;
				let gutter_width: u16 = digits as u16 + 1;
				let gutter_style = Style::default().bg(Color::Rgb(18, 18, 18)).fg(Color::Indexed(242));

				// --- GUTTER RENDERING ---
				for gy in 0..(max_height - 1) {
					for gx in 0..gutter_width {
						if let Some(cell) = buf.cell_mut((gx, gy)) {
							cell.set_char(' ').set_style(gutter_style);
						}
					}
					let line_num = scroll_y + gy as u32 + 1;
					if line_num <= view.total_lines + 1 {
						let line_str = line_num.to_string();
						if line_str.len() < gutter_width as usize {
							let start_x = (gutter_width - 1).saturating_sub(line_str.len() as u16);
							for (i, c) in line_str.chars().enumerate() {
								if let Some(cell) = buf.cell_mut((start_x + i as u16, gy)) {
									cell.set_char(c);
								}
							}
						}
					}
				}

				// --- VIEWPORT RENDERING ---
				let mut x: usize = gutter_width as usize;
				let mut y: usize = 0;
				let render_height = (max_height as usize).saturating_sub(1);

				for token in &view.tokens {
					let mut style = match token.kind {
						SemanticKind::Token => Style::default().fg(Color::LightGreen),
						_ => Style::default().fg(Color::White),
					};

					if token.is_virtual {
						style = style.add_modifier(Modifier::ITALIC).fg(Color::Yellow);
					}

					let text = if token.text.is_empty() { "[DMA PENDING...]" } else { &token.text };

					for c in text.chars() {
						if y >= render_height { break; }
						if c == '\n' {
							y += 1;
							x = gutter_width as usize;
						} else if c == '\t' {
							let tab_size = 4;
							let spaces = tab_size - (x - gutter_width as usize) % tab_size;
							for _ in 0..spaces {
								if x < max_width as usize {
									if let Some(cell) = buf.cell_mut((x as u16, y as u16)) {
										cell.set_char(' ').set_style(style);
									}
								}
								x += 1;
							}
						} else {
							if x < max_width as usize {
								if let Some(cell) = buf.cell_mut((x as u16, y as u16)) {
									cell.set_char(c).set_style(style);
								}
							}
							x += 1;
						}
					}
					if y >= render_height { break; }
				}

				// --- HARDWARE CURSOR ---
				let visual_cursor_y = view.cursor_abs_pos.0.saturating_sub(scroll_y) as u16;
				let visual_cursor_x = (view.cursor_abs_pos.1 as u16).checked_add(gutter_width).unwrap_or(max_width);
				if visual_cursor_y < max_height - 1 && visual_cursor_x < max_width {
					cursor_to_set = Some((visual_cursor_x, visual_cursor_y));
				}
			}

			// --- UNIBAR RENDERING ---
			let bar_y = max_height.saturating_sub(1);
			let bar_bg = Style::default().bg(Color::Rgb(18, 18, 18)).fg(Color::White);
			let w = max_width as usize;

			// Clear bar
			for sx in 0..max_width {
				if let Some(cell) = buf.cell_mut((sx, bar_y)) {
					cell.set_char(' ').set_style(bar_bg);
				}
			}

			if let Some(msg) = status_message {
				// Overlay status message (write feedback, errors)
				for (i, c) in msg.chars().enumerate() {
					if i >= w { break; }
					if let Some(cell) = buf.cell_mut((i as u16, bar_y)) {
						cell.set_char(c);
					}
				}
			} else {
				// Mode label
				let mode_str = match current_mode {
					EditorMode::Normal => if g_prefix { "NOR g" } else { "NOR" },
					EditorMode::Insert => "INS",
					EditorMode::Command => "CMD",
				};
				let mode_style = bar_bg.fg(Color::Rgb(0, 0, 0)).bg(match current_mode {
					EditorMode::Normal => Color::Rgb(130, 170, 255),
					EditorMode::Insert => Color::Rgb(180, 230, 130),
					EditorMode::Command => Color::Rgb(255, 180, 100),
				});

				let mut x = 0usize;

				// [ MODE ]
				let mode_text = format!(" {} ", mode_str);
				for c in mode_text.chars() {
					if x >= w { break; }
					if let Some(cell) = buf.cell_mut((x as u16, bar_y)) {
						cell.set_char(c).set_style(mode_style);
					}
					x += 1;
				}

				// Separator
				if x < w {
					if let Some(cell) = buf.cell_mut((x as u16, bar_y)) {
						cell.set_char(' ').set_style(bar_bg);
					}
					x += 1;
				}

				// Middle section: command input OR filename
				let (file_name, file_sz, dirty) = current_viewport.as_ref()
					.map(|v| (v.file_name.as_deref(), v.file_size, v.is_dirty))
					.unwrap_or((None, 0, false));

				if *current_mode == EditorMode::Command {
					let cmd_text = format!(":{}", command_buffer);
					for c in cmd_text.chars() {
						if x >= w { break; }
						if let Some(cell) = buf.cell_mut((x as u16, bar_y)) {
							cell.set_char(c).set_style(bar_bg);
						}
						x += 1;
					}
				} else {
					let display_name = file_name
						.map(|p| std::path::Path::new(p).file_name()
							.and_then(|n| n.to_str())
							.unwrap_or(p))
						.unwrap_or("[No File]");
					let name_style = if dirty {
						bar_bg.fg(Color::Rgb(255, 200, 120))
					} else {
						bar_bg
					};
					for c in display_name.chars() {
						if x >= w { break; }
						if let Some(cell) = buf.cell_mut((x as u16, bar_y)) {
							cell.set_char(c).set_style(name_style);
						}
						x += 1;
					}
					if dirty {
						if x < w {
							if let Some(cell) = buf.cell_mut((x as u16, bar_y)) {
								cell.set_char(' ').set_style(bar_bg);
							}
							x += 1;
						}
						if x < w {
							if let Some(cell) = buf.cell_mut((x as u16, bar_y)) {
								cell.set_char('\u{25CF}').set_style(bar_bg.fg(Color::Rgb(255, 160, 80)));
							}
							x += 1;
						}
					}
				}

				// Right-aligned segments: filesize | encoding | line:col
				let (cursor_line, cursor_col) = current_viewport.as_ref()
					.map(|v| (v.cursor_abs_pos.0 + 1, v.cursor_abs_pos.1 + 1))
					.unwrap_or((1, 1));

				let size_str = format_file_size(file_sz);
				let right_text = format!("{} | UTF-8 | {}:{} ", size_str, cursor_line, cursor_col);

				let right_start = w.saturating_sub(right_text.len());
				if right_start > x {
					let dim_style = bar_bg.fg(Color::Indexed(242));
					for (i, c) in right_text.chars().enumerate() {
						let rx = right_start + i;
						if rx >= w { break; }
						if let Some(cell) = buf.cell_mut((rx as u16, bar_y)) {
							cell.set_char(c).set_style(if c == '|' { dim_style } else { bar_bg });
						}
					}
				}
			}

			if let Some(pos) = cursor_to_set {
				f.set_cursor_position(pos);
			}
		})?;
		Ok(())
	}

	fn handle_normal_key(&mut self, code: KeyCode, modifiers: event::KeyModifiers, should_quit: &mut bool) -> bool {
		match code {
			KeyCode::Char('z') if modifiers.contains(event::KeyModifiers::CONTROL) => {
				let _ = self.tx_cmd.send(EditorCommand::Quit);
				*should_quit = true;
				return true;
			}
			KeyCode::Char(':') => {
				self.current_mode = EditorMode::Command;
				self.command_buffer.clear();
				self.g_prefix = false;
			}
			KeyCode::Char('h') => { let _ = self.tx_cmd.send(EditorCommand::MoveCursor(MoveDirection::Left)); self.g_prefix = false; }
			KeyCode::Char('j') => { let _ = self.tx_cmd.send(EditorCommand::MoveCursor(MoveDirection::Down)); self.g_prefix = false; }
			KeyCode::Char('k') => { let _ = self.tx_cmd.send(EditorCommand::MoveCursor(MoveDirection::Up)); self.g_prefix = false; }
			KeyCode::Char('l') => { let _ = self.tx_cmd.send(EditorCommand::MoveCursor(MoveDirection::Right)); self.g_prefix = false; }
			KeyCode::Char('g') => {
				if self.g_prefix {
					let _ = self.tx_cmd.send(EditorCommand::MoveCursor(MoveDirection::Top));
					self.g_prefix = false;
				} else {
					self.g_prefix = true;
				}
			}
			KeyCode::Char('G') => { let _ = self.tx_cmd.send(EditorCommand::MoveCursor(MoveDirection::Bottom)); self.g_prefix = false; }
			KeyCode::Char('i') => {
				self.current_mode = EditorMode::Insert;
				self.g_prefix = false;
				self.apply_cursor_style();
			}
			KeyCode::Backspace | KeyCode::Delete => { let _ = self.tx_cmd.send(EditorCommand::Backspace); self.g_prefix = false; }
			KeyCode::Esc => { self.g_prefix = false; }
			_ => { self.g_prefix = false; }
		}
		false
	}

	fn handle_command_key(&mut self, code: KeyCode, modifiers: event::KeyModifiers, should_quit: &mut bool) -> bool {
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
					let _ = self.tx_cmd.send(EditorCommand::WriteFileAs(expanded.to_string_lossy().to_string()));
				} else if self.command_buffer == "x" || self.command_buffer == "wq" {
					let _ = self.tx_cmd.send(EditorCommand::WriteAndQuit);
				} else if self.command_buffer.starts_with("e ") {
					let path = self.command_buffer[2..].trim();
					let expanded = crate::core::path::expand_path(path);
					let _ = self.tx_cmd.send(EditorCommand::LoadFile(expanded.to_string_lossy().to_string()));
				} else if let Ok(line_num) = self.command_buffer.parse::<u32>() {
					let _ = self.tx_cmd.send(EditorCommand::GotoLine(line_num.saturating_sub(1)));
				} else if self.command_buffer == "q" {
					let _ = self.tx_cmd.send(EditorCommand::Quit);
					*should_quit = true;
					return true;
				} else if !self.command_buffer.is_empty() {
					self.status_message = Some(format!("Unknown command: {}", self.command_buffer));
				}
				self.current_mode = EditorMode::Normal;
			}
			KeyCode::Esc => { self.current_mode = EditorMode::Normal; }
			KeyCode::Backspace => {
				if self.command_buffer.is_empty() {
					self.current_mode = EditorMode::Normal;
				} else {
					self.command_buffer.pop();
				}
			}
			KeyCode::Char(c) => { self.command_buffer.push(c); }
			_ => {}
		}
		false
	}

	fn handle_insert_key(&mut self, code: KeyCode, modifiers: event::KeyModifiers, should_quit: &mut bool) -> bool {
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
			KeyCode::Enter => { let _ = self.tx_cmd.send(EditorCommand::InsertChar('\n')); }
			KeyCode::Backspace => { let _ = self.tx_cmd.send(EditorCommand::Backspace); }
			KeyCode::Char(c) => { let _ = self.tx_cmd.send(EditorCommand::InsertChar(c)); }
			_ => {}
		}
		false
	}

	fn apply_cursor_style(&mut self) {
		let style = match self.current_mode {
			EditorMode::Normal | EditorMode::Command => SetCursorStyle::SteadyBlock,
			EditorMode::Insert => SetCursorStyle::SteadyBar,
		};
		let _ = execute!(self.terminal.backend_mut(), style);
	}

	pub fn release_terminal(self) -> Terminal<B> {
		self.terminal
	}
}

fn format_file_size(bytes: u64) -> String {
	const KB: u64 = 1024;
	const MB: u64 = 1024 * KB;
	const GB: u64 = 1024 * MB;
	if bytes >= GB {
		format!("{:.1} GB", bytes as f64 / GB as f64)
	} else if bytes >= MB {
		format!("{:.1} MB", bytes as f64 / MB as f64)
	} else if bytes >= KB {
		format!("{:.1} KB", bytes as f64 / KB as f64)
	} else {
		format!("{} B", bytes)
	}
}
