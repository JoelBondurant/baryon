use crate::ecs::{NodeId, SemanticKind, UastRegistry, Viewport};
use crate::io::{SvpResolver, ingest_svp_file};
use crossterm::{
	event::{self, Event, KeyCode, KeyEventKind},
	execute,
	terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
	backend::CrosstermBackend,
	style::{Color, Modifier, Style},
	Terminal,
};
use std::{io as std_io, sync::atomic::Ordering, sync::mpsc, thread, time::Duration, sync::Arc};

mod ecs;
mod io;

/// ==========================================
/// COMMANDS (UI -> ENGINE)
/// ==========================================
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorMode {
	Normal,
	Command,
}

pub enum MoveDirection {
	Up,
	Down,
	Left,
	Right,
	Top,
	Bottom,
}

pub enum EditorCommand {
	InsertChar(char),
	Backspace,
	Scroll(i32),
	MoveCursor(MoveDirection),
	LoadFile(String),
	InternalRefresh,
	Quit,
}

fn main() -> Result<(), std_io::Error> {
	// 1. Setup Channels & Registry
	let (tx_cmd, rx_cmd) = mpsc::channel::<EditorCommand>();
	let (tx_view, rx_view) = mpsc::channel::<Viewport>();
	let (tx_io_notify, rx_io_notify) = mpsc::channel::<()>();

	let registry = Arc::new(UastRegistry::new(1_000_000)); // Larger capacity for 10GB test
	let resolver = Arc::new(SvpResolver::new(registry.clone(), tx_io_notify));

	// Bridge IO notifications to the Engine command loop
	let tx_cmd_bridge = tx_cmd.clone();
	thread::spawn(move || {
		while let Ok(_) = rx_io_notify.recv() {
			let _ = tx_cmd_bridge.send(EditorCommand::InternalRefresh);
		}
	});

	// 2. Initial State
	let initial_cursor_node = NodeId(std::num::NonZeroU32::new(1).unwrap());

	// 3. Spawn Engine Thread
	let registry_engine = registry.clone();
	let resolver_engine = resolver.clone();
	let tx_view_engine = tx_view.clone();

	thread::spawn(move || {
		let mut cursor_abs_line: u32 = 0;
		let mut cursor_abs_col: u32 = 0;
		let viewport_lines = 50;
		let mut root_id: Option<NodeId> = None;

		let mut cursor_node = initial_cursor_node;
		let mut cursor_offset = 0;

		// Engine Loop
		while let Ok(cmd) = rx_cmd.recv() {
			let mut needs_render = false;

			match cmd {
				EditorCommand::MoveCursor(dir) => {
					if let Some(rid) = root_id {
						match dir {
							MoveDirection::Up => cursor_abs_line = cursor_abs_line.saturating_sub(1),
							MoveDirection::Down => {
								let total = registry_engine.get_total_newlines(rid);
								if cursor_abs_line < total {
									cursor_abs_line += 1;
								}
							}
							MoveDirection::Left => cursor_abs_col = cursor_abs_col.saturating_sub(1),
							MoveDirection::Right => cursor_abs_col += 1,
							MoveDirection::Top => {
								cursor_abs_line = 0;
								cursor_abs_col = 0;
							}
							MoveDirection::Bottom => {
								cursor_abs_line = registry_engine.get_total_newlines(rid);
								cursor_abs_col = 0;
							}
						}
						// Sync node/offset
						let (node, offset) = registry_engine.find_node_at_line_col(
							rid,
							cursor_abs_line,
							cursor_abs_col,
						);
						cursor_node = node;
						cursor_offset = offset;
						needs_render = true;
					}
				}
				EditorCommand::InsertChar(c) => {
					if let Some(_rid) = root_id {
						let mut buf = [0; 4];
						let s = c.encode_utf8(&mut buf);
						let (new_node, new_offset) =
							registry_engine.insert_text(cursor_node, cursor_offset, s.as_bytes());
						cursor_node = new_node;
						cursor_offset = new_offset;
						cursor_abs_col += 1;
						needs_render = true;
					}
				}
				EditorCommand::Backspace => {
					if let Some(_rid) = root_id {
						let (new_node, new_offset) =
							registry_engine.delete_backwards(cursor_node, cursor_offset);
						cursor_node = new_node;
						cursor_offset = new_offset;
						cursor_abs_col = cursor_abs_col.saturating_sub(1);
						needs_render = true;
					}
				}
				EditorCommand::Scroll(delta) => {
					cursor_abs_line = (cursor_abs_line as i32 + delta).max(0) as u32;
					needs_render = true;
				}
				EditorCommand::LoadFile(path) => {
					if let Ok(metadata) = std::fs::metadata(&path) {
						let file_size = metadata.len();
						let device_id = 0x42;
						resolver_engine.register_device(device_id, &path);

						let rid = ingest_svp_file(&registry_engine, file_size, device_id);
						root_id = Some(rid);
						cursor_node = registry_engine
							.get_first_child(rid)
							.expect("Failed to load new file root");
						cursor_offset = 0;
						cursor_abs_line = 0;
						cursor_abs_col = 0;
						needs_render = true;
					}
				}
				EditorCommand::InternalRefresh => {
					needs_render = true;
				}
				EditorCommand::Quit => break,
			}

			if needs_render {
				if let Some(rid) = root_id {
					let scroll_y = cursor_abs_line.saturating_sub(20);
					let tokens = registry_engine.query_viewport(
						rid,
						scroll_y,
						viewport_lines,
					);

					// TRIGGER DMA
					for token in &tokens {
						if !token.is_virtual && token.text.is_empty() {
							let idx = token.node_id.index();
							if !registry_engine.dma_in_flight[idx].swap(true, Ordering::Relaxed) {
								if let Some(svp) = unsafe { *registry_engine.spans[idx].get() } {
									resolver_engine.request_dma(token.node_id, svp);
								}
							}
						}
					}

					let _ = tx_view_engine.send(Viewport {
						tokens,
						cursor_abs_pos: (cursor_abs_line, cursor_abs_col),
					});
				}
			}
		}
	});

	// 4. Setup Frontend
	enable_raw_mode()?;
	let mut stdout = std_io::stdout();
	execute!(stdout, EnterAlternateScreen)?;
	let backend = CrosstermBackend::new(stdout);
	let mut terminal = Terminal::new(backend)?;

	let mut current_viewport: Option<Viewport> = None;
	let mut current_mode = EditorMode::Normal;
	let mut command_buffer = String::new();
	let mut g_prefix = false;

	// 5. Main UI Loop
	let mut initial_draw = true;
	loop {
		let mut got_new_view = initial_draw;
		initial_draw = false;

		while let Ok(view) = rx_view.try_recv() {
			current_viewport = Some(view);
			got_new_view = true;
		}

		if got_new_view || current_mode == EditorMode::Command {
			terminal.draw(|f| {
				let mut cursor_to_set = None;
				let buf = f.buffer_mut();
				let max_width = buf.area.width;
				let max_height = buf.area.height;

				const GUTTER_WIDTH: u16 = 6;
				let gutter_style = Style::default().bg(Color::Rgb(18, 18, 18)).fg(Color::Indexed(242));

				if let Some(view) = &current_viewport {
					let scroll_y = view.cursor_abs_pos.0.saturating_sub(20);
					
					// --- GUTTER RENDERING ---
					for gy in 0..(max_height - 1) {
						// Fill background
						for gx in 0..GUTTER_WIDTH {
							if let Some(cell) = buf.cell_mut((gx, gy)) {
								cell.set_char(' ').set_style(gutter_style);
							}
						}
						
						// Render line number (1-indexed)
						let line_num = scroll_y + gy as u32 + 1;
						let line_str = line_num.to_string();
						if line_str.len() < GUTTER_WIDTH as usize {
							let start_x = (GUTTER_WIDTH - 1).saturating_sub(line_str.len() as u16);
							for (i, c) in line_str.chars().enumerate() {
								if let Some(cell) = buf.cell_mut((start_x + i as u16, gy)) {
									cell.set_char(c);
								}
							}
						}
					}

					// --- VIEWPORT RENDERING ---
					let mut x = GUTTER_WIDTH;
					let mut y = 0;

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
							if y >= max_height - 1 { break; }
							if c == '\n' {
								y += 1;
								x = GUTTER_WIDTH;
							} else {
								if x < max_width {
									if let Some(cell) = buf.cell_mut((x, y)) {
										cell.set_char(c).set_style(style);
									}
								}
								x += 1;
							}
						}
						if y >= max_height - 1 { break; }
					}

					// --- HARDWARE CURSOR ---
					let visual_cursor_y = view.cursor_abs_pos.0.saturating_sub(scroll_y) as u16;
					let visual_cursor_x = (view.cursor_abs_pos.1 as u16).checked_add(GUTTER_WIDTH).unwrap_or(max_width);
					if visual_cursor_y < max_height - 1 && visual_cursor_x < max_width {
						cursor_to_set = Some((visual_cursor_x, visual_cursor_y));
					}
				}

				// --- STATUS BAR RENDERING ---
				let status_bar_y = max_height - 1;
				let status_bar_style = Style::default().bg(Color::Rgb(18, 18, 18)).fg(Color::White);

				for sx in 0..max_width {
					if let Some(cell) = buf.cell_mut((sx, status_bar_y)) {
						cell.set_char(' ').set_style(status_bar_style);
					}
				}

				let status_text = match current_mode {
					EditorMode::Normal => if g_prefix { "-- NORMAL (g pending) --" } else { "-- NORMAL --" }.to_string(),
					EditorMode::Command => format!(":{}", command_buffer),
				};

				for (i, c) in status_text.chars().enumerate() {
					if i < max_width as usize {
						if let Some(cell) = buf.cell_mut((i as u16, status_bar_y)) {
							cell.set_char(c);
						}
					}
				}

				if let Some(pos) = cursor_to_set {
					f.set_cursor_position(pos);
				}
			}).unwrap();
		}

		let mut should_quit = false;
		if event::poll(Duration::from_millis(16)).unwrap() {
			loop {
				if let Event::Key(key) = event::read().unwrap() {
					if key.kind == KeyEventKind::Press {
						match current_mode {
							EditorMode::Normal => match key.code {
								KeyCode::Char('z') if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
									let _ = tx_cmd.send(EditorCommand::Quit);
									should_quit = true;
									break;
								}
								KeyCode::Char(':') => {
									current_mode = EditorMode::Command;
									command_buffer.clear();
									g_prefix = false;
								}
								KeyCode::Char('h') => { let _ = tx_cmd.send(EditorCommand::MoveCursor(MoveDirection::Left)); g_prefix = false; }
								KeyCode::Char('j') => { let _ = tx_cmd.send(EditorCommand::MoveCursor(MoveDirection::Down)); g_prefix = false; }
								KeyCode::Char('k') => { let _ = tx_cmd.send(EditorCommand::MoveCursor(MoveDirection::Up)); g_prefix = false; }
								KeyCode::Char('l') => { let _ = tx_cmd.send(EditorCommand::MoveCursor(MoveDirection::Right)); g_prefix = false; }
								KeyCode::Char('g') => {
									if g_prefix {
										let _ = tx_cmd.send(EditorCommand::MoveCursor(MoveDirection::Top));
										g_prefix = false;
									} else {
										g_prefix = true;
									}
								}
								KeyCode::Char('G') => { let _ = tx_cmd.send(EditorCommand::MoveCursor(MoveDirection::Bottom)); g_prefix = false; }
								KeyCode::Backspace | KeyCode::Delete => { let _ = tx_cmd.send(EditorCommand::Backspace); g_prefix = false; }
								KeyCode::Char(c) => { let _ = tx_cmd.send(EditorCommand::InsertChar(c)); g_prefix = false; }
								KeyCode::Esc => { g_prefix = false; }
								_ => { g_prefix = false; }
							},
							EditorMode::Command => match key.code {
								KeyCode::Char('z') if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
									let _ = tx_cmd.send(EditorCommand::Quit);
									should_quit = true;
									break;
								}
								KeyCode::Enter => {
									if command_buffer.starts_with("e ") {
										let path = command_buffer[2..].trim().to_string();
										let _ = tx_cmd.send(EditorCommand::LoadFile(path));
									} else if command_buffer == "q" {
										let _ = tx_cmd.send(EditorCommand::Quit);
										should_quit = true;
										break;
									}
									current_mode = EditorMode::Normal;
								}
								KeyCode::Esc => { current_mode = EditorMode::Normal; }
								KeyCode::Backspace => { command_buffer.pop(); }
								KeyCode::Char(c) => { command_buffer.push(c); }
								_ => {}
							},
						}
					}
				}
				if should_quit || !event::poll(Duration::from_millis(0)).unwrap() {
					break;
				}
			}
		}
		
		if should_quit { break; }
	}

	disable_raw_mode()?;
	execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
	terminal.show_cursor()?;

	Ok(())
}
