use crate::core::TAB_SIZE;
use crate::engine::{
	ConfirmAction, EditorCommand, EditorMode, MoveDirection, SubstituteFlags, SubstituteRange,
};
use crate::uast::kind::SemanticKind;
use crate::uast::projection::Viewport;
use crossterm::cursor::SetCursorStyle;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind};
use crossterm::execute;
use ratatui::{
	Terminal,
	backend::Backend,
	style::{Color, Style},
};
use regex_automata::meta::Regex;
use regex_automata::util::syntax;
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
	y_prefix: bool,
	pending_register: Option<char>,
	status_message: Option<String>,
	needs_redraw: bool,
	search_buffer: String,
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
			y_prefix: false,
			pending_register: None,
			status_message: None,
			needs_redraw: false,
			search_buffer: String::new(),
		}
	}

	pub fn run(&mut self) -> Result<(), io::Error> {
		self.terminal
			.clear()
			.map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
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
				if let Some(mode) = view.mode_override.clone() {
					self.current_mode = mode;
					self.apply_cursor_style();
				}
				self.current_viewport = Some(view);
				got_new_view = true;
			}

			if got_new_view
				|| self.needs_redraw
				|| matches!(
					self.current_mode,
					EditorMode::Command
						| EditorMode::Insert
						| EditorMode::Search
						| EditorMode::Confirm
				) {
				self.needs_redraw = false;
				self.draw()
					.map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
			}
			if event::poll(Duration::from_millis(16))? {
				loop {
					match event::read()? {
						Event::Key(key) if key.kind == KeyEventKind::Press => {
							self.status_message = None;
							self.needs_redraw = true;
							match self.current_mode {
								EditorMode::Normal => {
									if self.handle_normal_key(
										key.code,
										key.modifiers,
										&mut should_quit,
									) {
										break;
									}
								}
								EditorMode::Insert => {
									if self.handle_insert_key(
										key.code,
										key.modifiers,
										&mut should_quit,
									) {
										break;
									}
								}
								EditorMode::Command => {
									if self.handle_command_key(
										key.code,
										key.modifiers,
										&mut should_quit,
									) {
										break;
									}
								}
								EditorMode::Search => {
									if self.handle_search_key(
										key.code,
										key.modifiers,
										&mut should_quit,
									) {
										break;
									}
								}
								EditorMode::Confirm => {
									if self.handle_confirm_key(
										key.code,
										key.modifiers,
										&mut should_quit,
									) {
										break;
									}
								}
							}
						}
						Event::Mouse(mouse) => {
							self.handle_mouse(mouse);
						}
						_ => {}
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
		let search_buffer = &self.search_buffer;
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
				let gutter_style = Style::default()
					.bg(Color::Rgb(18, 18, 18))
					.fg(Color::Indexed(242));

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

				let search_pat = view.search_pattern.as_deref().unwrap_or("");

				let projector =
					crate::svp::projector::HighlightProjector::new(view.highlights.clone());
				let mut current_global_byte: u64 = view.global_start_byte;

				for token in &view.tokens {
					let base_style = match token.kind {
						SemanticKind::Token => Style::default().fg(Color::Indexed(253)),
						_ => Style::default().fg(Color::Indexed(253)),
					};

					let virtual_style = base_style;

					let text = if token.text.is_empty() {
						"[DMA PENDING...]"
					} else {
						&token.text
					};

					// Precompute highlight byte ranges for search matches in this token
					let highlight_style = Style::default()
						.bg(Color::Rgb(180, 140, 50))
						.fg(Color::Black);
					let search_ci = view.search_case_insensitive;
					let mut highlight_ranges: Vec<(usize, usize)> = Vec::new();
					if !search_pat.is_empty() && !token.text.is_empty() {
						let tbytes = text.as_bytes();
						if let Ok(re) = Regex::builder()
							.syntax(syntax::Config::new().case_insensitive(search_ci))
							.build(search_pat)
						{
							for m in re.find_iter(tbytes) {
								highlight_ranges.push((m.start(), m.end()));
							}
						}
					}

					let mut byte_idx = 0usize;
					for c in text.chars() {
						let in_highlight = highlight_ranges
							.iter()
							.any(|&(s, e)| byte_idx >= s && byte_idx < e);

						let mut style = if in_highlight {
							highlight_style
						} else {
							virtual_style
						};
						// Undo/redo can rebuild the document as virtual nodes that still
						// carry real bytes, so syntax colors must follow loaded text rather
						// than the node's storage backing.
						if !in_highlight && !token.text.is_empty() {
							if let Some(fg) = projector.style_for_byte(current_global_byte) {
								style = style.fg(fg);
							}
						}

						// Yank flash override: gold highlight over the yanked byte range.
						if let Some((flash_start, flash_end)) = view.yank_flash {
							if current_global_byte >= flash_start && current_global_byte < flash_end
							{
								style = Style::default()
									.bg(Color::Rgb(229, 192, 123))
									.fg(Color::Black);
							}
						}

						// Peek ahead to detect trailing spaces (space before \n or end of token).
						let char_len = c.len_utf8();
						let is_trailing_space = c == ' ' && {
							let rest = &text.as_bytes()[byte_idx + char_len..];
							rest.is_empty() || rest[0] == b'\n'
						};

						byte_idx += char_len;
						current_global_byte += char_len as u64;

						if y >= render_height {
							break;
						}

						let ws_style = style.fg(crate::svp::projector::WHITESPACE_COLOR);

						if c == '\n' {
							// EOL marker before advancing the line.
							if x < max_width as usize {
								if let Some(cell) = buf.cell_mut((x as u16, y as u16)) {
									cell.set_char('\u{00AC}').set_style(ws_style);
								}
							}
							y += 1;
							x = gutter_width as usize;
						} else if c == '\t' {
							// Dynamic tab expansion (tabstop = TAB_SIZE).
							let col = x - gutter_width as usize;
							let spaces_to_add = TAB_SIZE as usize - (col % TAB_SIZE as usize);
							// First cell: tab start glyph.
							if x < max_width as usize {
								if let Some(cell) = buf.cell_mut((x as u16, y as u16)) {
									cell.set_char('\u{25B8}').set_style(ws_style);
								}
							}
							x += 1;
							// Remaining cells: plain spaces.
							for _ in 1..spaces_to_add {
								if x < max_width as usize {
									if let Some(cell) = buf.cell_mut((x as u16, y as u16)) {
										cell.set_char(' ').set_style(ws_style);
									}
								}
								x += 1;
							}
						} else if is_trailing_space {
							// Trailing space marker.
							if x < max_width as usize {
								if let Some(cell) = buf.cell_mut((x as u16, y as u16)) {
									cell.set_char('~').set_style(ws_style);
								}
							}
							x += 1;
						} else if c == ' ' {
							// Mid-line space marker.
							if x < max_width as usize {
								if let Some(cell) = buf.cell_mut((x as u16, y as u16)) {
									cell.set_char('\u{2423}').set_style(ws_style);
								}
							}
							x += 1;
						} else {
							if x < max_width as usize {
								if let Some(cell) = buf.cell_mut((x as u16, y as u16)) {
									cell.set_char(c).set_style(style);
								}
							}
							x += 1;
						}
					}
					if y >= render_height {
						break;
					}
				}

				// --- HARDWARE CURSOR ---
				let visual_cursor_y = view.cursor_abs_pos.0.saturating_sub(scroll_y) as u16;
				let visual_cursor_x = (view.cursor_abs_pos.1 as u16)
					.checked_add(gutter_width)
					.unwrap_or(max_width);
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
					if i >= w {
						break;
					}
					if let Some(cell) = buf.cell_mut((i as u16, bar_y)) {
						cell.set_char(c);
					}
				}
			} else {
				// Mode label
				let mode_str = match current_mode {
					EditorMode::Normal => {
						if g_prefix {
							"NOR g"
						} else {
							"NOR"
						}
					}
					EditorMode::Insert => "INS",
					EditorMode::Command => "CMD",
					EditorMode::Search => "FIND",
					EditorMode::Confirm => "Y/N",
				};
				let mode_style = bar_bg.fg(Color::Rgb(0, 0, 0)).bg(match current_mode {
					EditorMode::Normal => Color::Rgb(130, 170, 255),
					EditorMode::Insert => Color::Rgb(180, 230, 130),
					EditorMode::Command => Color::Rgb(255, 180, 100),
					EditorMode::Search => Color::Rgb(200, 160, 255),
					EditorMode::Confirm => Color::Rgb(255, 120, 120),
				});

				let mut x = 0usize;

				// [ MODE ]
				let mode_text = format!(" {} ", mode_str);
				for c in mode_text.chars() {
					if x >= w {
						break;
					}
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
				let (file_name, file_sz, dirty) = current_viewport
					.as_ref()
					.map(|v| (v.file_name.as_deref(), v.file_size, v.is_dirty))
					.unwrap_or((None, 0, false));

				if *current_mode == EditorMode::Command {
					let cmd_text = format!(":{}", command_buffer);
					for c in cmd_text.chars() {
						if x >= w {
							break;
						}
						if let Some(cell) = buf.cell_mut((x as u16, bar_y)) {
							cell.set_char(c).set_style(bar_bg);
						}
						x += 1;
					}
				} else if *current_mode == EditorMode::Search {
					let search_text = format!("/{}", search_buffer);
					for c in search_text.chars() {
						if x >= w {
							break;
						}
						if let Some(cell) = buf.cell_mut((x as u16, bar_y)) {
							cell.set_char(c).set_style(bar_bg);
						}
						x += 1;
					}
				} else if *current_mode == EditorMode::Confirm {
					let prompt = current_viewport
						.as_ref()
						.and_then(|v| v.confirm_prompt.as_deref())
						.unwrap_or("Replace? [y/n/a/q]");
					for c in prompt.chars() {
						if x >= w {
							break;
						}
						if let Some(cell) = buf.cell_mut((x as u16, bar_y)) {
							cell.set_char(c).set_style(bar_bg);
						}
						x += 1;
					}
				} else {
					let display_name = file_name
						.map(|p| {
							std::path::Path::new(p)
								.file_name()
								.and_then(|n| n.to_str())
								.unwrap_or(p)
						})
						.unwrap_or("[No File]");
					let name_style = if dirty {
						bar_bg.fg(Color::Rgb(255, 200, 120))
					} else {
						bar_bg
					};
					for c in display_name.chars() {
						if x >= w {
							break;
						}
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
								cell.set_char('\u{25CF}')
									.set_style(bar_bg.fg(Color::Rgb(255, 160, 80)));
							}
							x += 1;
						}
					}
				}

				// Right-aligned segments: [search] | filesize | encoding | line:col
				let (cursor_line, cursor_col) = current_viewport
					.as_ref()
					.map(|v| (v.cursor_abs_pos.0 + 1, v.cursor_abs_pos.1 + 1))
					.unwrap_or((1, 1));

				let search_info = current_viewport
					.as_ref()
					.and_then(|v| v.search_match_info.as_deref());
				let size_str = format_file_size(file_sz);
				let right_text = match search_info {
					Some(info) => format!(
						"{} | {} | UTF-8 | {}:{} ",
						info, size_str, cursor_line, cursor_col
					),
					None => format!("{} | UTF-8 | {}:{} ", size_str, cursor_line, cursor_col),
				};

				let right_start = w.saturating_sub(right_text.len());
				if right_start > x {
					let dim_style = bar_bg.fg(Color::Indexed(242));
					let search_style = bar_bg.fg(Color::Rgb(200, 160, 255));
					for (i, c) in right_text.chars().enumerate() {
						let rx = right_start + i;
						if rx >= w {
							break;
						}
						let style = if c == '|' {
							dim_style
						} else if search_info.is_some()
							&& rx < right_start + search_info.unwrap().len()
						{
							search_style
						} else {
							bar_bg
						};
						if let Some(cell) = buf.cell_mut((rx as u16, bar_y)) {
							cell.set_char(c).set_style(style);
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

	fn clear_prefixes(&mut self) {
		self.g_prefix = false;
		self.y_prefix = false;
		self.pending_register = None;
	}

	fn handle_normal_key(
		&mut self,
		code: KeyCode,
		modifiers: event::KeyModifiers,
		should_quit: &mut bool,
	) -> bool {
		// --- Register prefix state machine ---
		// `"` starts register selection; next char becomes the register name.
		if self.pending_register == Some('\0') {
			// We are awaiting the register name character.
			if let KeyCode::Char(c) = code {
				self.pending_register = Some(c);
				// Now awaiting the action (y/p). Don't clear prefixes.
				return false;
			} else {
				self.clear_prefixes();
				return false;
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
				// Enter register-pending state. Use '\0' as sentinel for "awaiting name".
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
			KeyCode::Char('y') => {
				if self.y_prefix {
					let _ = self.tx_cmd.send(EditorCommand::YankLine { register });
					self.clear_prefixes();
				} else {
					self.y_prefix = true;
					// Preserve pending_register through the y prefix.
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
			KeyCode::Esc => {
				self.clear_prefixes();
			}
			_ => {
				self.clear_prefixes();
			}
		}
		false
	}

	fn handle_command_key(
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
				} else if let Ok(line_num) = self.command_buffer.parse::<u32>() {
					let _ = self
						.tx_cmd
						.send(EditorCommand::GotoLine(line_num.saturating_sub(1)));
				} else if self.command_buffer == "q" {
					let _ = self.tx_cmd.send(EditorCommand::Quit);
					*should_quit = true;
					return true;
				} else if self.command_buffer.contains("s/") {
					let cursor_line = self
						.current_viewport
						.as_ref()
						.map(|v| v.cursor_abs_pos.0)
						.unwrap_or(0);
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

	fn handle_insert_key(
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
			KeyCode::Backspace => {
				let _ = self.tx_cmd.send(EditorCommand::Backspace);
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
			KeyCode::Char(c) => {
				let _ = self.tx_cmd.send(EditorCommand::InsertChar(c));
			}
			_ => {}
		}
		false
	}

	fn handle_search_key(
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

	fn handle_confirm_key(
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

	fn handle_mouse(&mut self, mouse: event::MouseEvent) {
		match mouse.kind {
			MouseEventKind::ScrollUp => {
				let _ = self.tx_cmd.send(EditorCommand::Scroll(-3));
				self.needs_redraw = true;
			}
			MouseEventKind::ScrollDown => {
				let _ = self.tx_cmd.send(EditorCommand::Scroll(3));
				self.needs_redraw = true;
			}
			MouseEventKind::Down(MouseButton::Left) => {
				let view = match &self.current_viewport {
					Some(v) => v,
					None => return,
				};

				let scroll_y = view.cursor_abs_pos.0.saturating_sub(20);
				let max_line = scroll_y + view.total_lines;
				let digits = max_line.max(1).ilog10() + 1;
				let gutter_width = digits as u16 + 1;

				let click_x = mouse.column;
				let click_y = mouse.row;

				// Bounds check: click must be in the text area.
				if click_x < gutter_width {
					return;
				}

				let abs_line = scroll_y + click_y as u32;
				let target_visual_col = u32::from(click_x - gutter_width);

				let _ = self
					.tx_cmd
					.send(EditorCommand::ClickCursor(abs_line, target_visual_col));
				self.needs_redraw = true;
			}
			_ => {}
		}
	}

	fn apply_cursor_style(&mut self) {
		let style = match self.current_mode {
			EditorMode::Normal | EditorMode::Command | EditorMode::Search | EditorMode::Confirm => {
				SetCursorStyle::SteadyBlock
			}
			EditorMode::Insert => SetCursorStyle::SteadyBar,
		};
		let _ = execute!(self.terminal.backend_mut(), style);
	}

	pub fn release_terminal(self) -> Terminal<B> {
		self.terminal
	}
}

fn parse_substitute(
	cmd: &str,
	cursor_line: u32,
) -> Option<(String, String, SubstituteFlags, SubstituteRange)> {
	// Find "s/" to split range from pattern
	let s_pos = cmd.find("s/")?;
	let range_str = &cmd[..s_pos];
	let rest = &cmd[s_pos + 2..]; // after "s/"

	// Parse range
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
		SubstituteRange::LineRange(a, b)
	} else if let Ok(n) = range_str.parse::<u32>() {
		SubstituteRange::SingleLine(n.saturating_sub(1))
	} else {
		return None;
	};

	// Parse pattern/replacement/flags
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
