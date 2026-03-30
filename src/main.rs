use crate::ecs::{NodeId, RenderToken, SemanticKind, UastRegistry};
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

pub enum EditorCommand {
	InsertChar(char),
	Backspace,
	Scroll(i32),
	LoadFile(String),
	InternalRefresh,
	Quit,
}

fn main() -> Result<(), std_io::Error> {
	// 1. Setup Channels & Registry
	let (tx_cmd, rx_cmd) = mpsc::channel::<EditorCommand>();
	let (tx_view, rx_view) = mpsc::channel::<Vec<RenderToken>>();
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

	// 2. Initial State (Empty or Mock)
	let _current_root_id = registry.capacity; // Placeholder
	let cursor_node = NodeId(std::num::NonZeroU32::new(1).unwrap());
	let cursor_offset = 0;

	// 3. Spawn Engine Thread
	let registry_engine = registry.clone();
	let resolver_engine = resolver.clone();
	let tx_view_engine = tx_view.clone();

	thread::spawn(move || {
		let mut cursor_absolute_line = 0;
		let viewport_lines = 50;
		let mut root_id = None;

		let mut cursor_node = cursor_node;
		let mut cursor_offset = cursor_offset;

		// Engine Loop
		while let Ok(cmd) = rx_cmd.recv() {
			let mut needs_render = false;

			match cmd {
				EditorCommand::InsertChar(c) => {
					if let Some(_rid) = root_id {
						let mut buf = [0; 4];
						let s = c.encode_utf8(&mut buf);
						let (new_node, new_offset) =
							registry_engine.insert_text(cursor_node, cursor_offset, s.as_bytes());
						cursor_node = new_node;
						cursor_offset = new_offset;
						needs_render = true;
					}
				}
				EditorCommand::Backspace => {
					if let Some(_rid) = root_id {
						let (new_node, new_offset) =
							registry_engine.delete_backwards(cursor_node, cursor_offset);
						cursor_node = new_node;
						cursor_offset = new_offset;
						needs_render = true;
					}
				}
				EditorCommand::Scroll(delta) => {
					cursor_absolute_line = (cursor_absolute_line as i32 + delta).max(0) as u32;
					needs_render = true;
				}
				EditorCommand::LoadFile(path) => {
					if let Ok(metadata) = std::fs::metadata(&path) {
						let file_size = metadata.len();
						let device_id = 0x42; // For prototype, use same ID
						resolver_engine.register_device(device_id, &path);

						let rid = ingest_svp_file(&registry_engine, file_size, device_id);
						root_id = Some(rid);
						cursor_node = registry_engine
							.get_first_child(rid)
							.expect("Failed to load new file root");
						cursor_offset = 0;
						cursor_absolute_line = 0;
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
					let tokens = registry_engine.query_viewport(
						rid,
						cursor_absolute_line,
						viewport_lines,
					);

					// TRIGGER DMA for physical nodes
					for token in &tokens {
						if !token.is_virtual && token.text.is_empty() {
							let idx = token.node_id.index();
							// Only request if not already in flight (atomic check-and-set)
							if !registry_engine.dma_in_flight[idx].swap(true, Ordering::Relaxed) {
								if let Some(svp) =
									unsafe { *registry_engine.spans[idx].get() }
								{
									resolver_engine.request_dma(token.node_id, svp);
								}
							}
						}
					}

					let _ = tx_view_engine.send(tokens);
				}
			}
		}
	});

	// 4. Setup Frontend (Crossterm + Ratatui)
	enable_raw_mode()?;
	let mut stdout = std_io::stdout();
	execute!(stdout, EnterAlternateScreen)?;
	let backend = CrosstermBackend::new(stdout);
	let mut terminal = Terminal::new(backend)?;

	let mut current_tokens: Vec<RenderToken> = Vec::new();
	let mut current_mode = EditorMode::Normal;
	let mut command_buffer = String::new();

	// 5. Main UI Loop
	let mut initial_draw = true;
	loop {
		let mut got_new_view = initial_draw;
		initial_draw = false;

		while let Ok(tokens) = rx_view.try_recv() {
			current_tokens = tokens;
			got_new_view = true;
		}

		if got_new_view || current_mode == EditorMode::Command {
			terminal
				.draw(|f| {
					let buf = f.buffer_mut();
					let max_width = buf.area.width;
					let max_height = buf.area.height;

					// --- VIEWPORT RENDERING ---
					let mut x = 0;
					let mut y = 0;

					for token in &current_tokens {
						let mut style = match token.kind {
							SemanticKind::Token => Style::default().fg(Color::LightGreen),
							_ => Style::default().fg(Color::White),
						};

						if token.is_virtual {
							style = style.add_modifier(Modifier::ITALIC).fg(Color::Yellow);
						}

						let text = if token.text.is_empty() {
							"[DMA PENDING...]"
						} else {
							&token.text
						};

						for c in text.chars() {
							if y >= max_height - 1 {
								break;
							}
							if c == '\n' {
								y += 1;
								x = 0;
							} else {
								if x < max_width {
									if let Some(cell) = buf.cell_mut((x, y)) {
										cell.set_char(c).set_style(style);
									}
								}
								x += 1;
							}
						}
						if y >= max_height - 1 {
							break;
						}
					}

					// --- STATUS BAR RENDERING ---
					let status_bar_y = max_height - 1;
					let status_bar_style = Style::default()
						.bg(Color::Rgb(18, 18, 18))
						.fg(Color::White);

					// Fill status bar background
					for sx in 0..max_width {
						if let Some(cell) = buf.cell_mut((sx, status_bar_y)) {
							cell.set_char(' ').set_style(status_bar_style);
						}
					}

					let status_text = match current_mode {
						EditorMode::Normal => "-- NORMAL --".to_string(),
						EditorMode::Command => format!(":{}", command_buffer),
					};

					for (i, c) in status_text.chars().enumerate() {
						if i < max_width as usize {
							if let Some(cell) = buf.cell_mut((i as u16, status_bar_y)) {
								cell.set_char(c);
							}
						}
					}
				})
				.unwrap();
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
								}
								KeyCode::Backspace | KeyCode::Delete => {
									let _ = tx_cmd.send(EditorCommand::Backspace);
								}
								KeyCode::Char(c) => {
									let _ = tx_cmd.send(EditorCommand::InsertChar(c));
								}
								KeyCode::Up => {
									let _ = tx_cmd.send(EditorCommand::Scroll(-1));
								}
								KeyCode::Down => {
									let _ = tx_cmd.send(EditorCommand::Scroll(1));
								}
								_ => {}
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
								KeyCode::Esc => {
									current_mode = EditorMode::Normal;
								}
								KeyCode::Backspace => {
									command_buffer.pop();
								}
								KeyCode::Char(c) => {
									command_buffer.push(c);
								}
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

		if should_quit {
			break;
		}
	}

	disable_raw_mode()?;
	execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
	terminal.show_cursor()?;

	Ok(())
}
