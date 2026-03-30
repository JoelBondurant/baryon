use crate::ecs::{RenderToken, SemanticKind, SpanMetrics, SvpPointer, UastRegistry};
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
use std::{io, sync::mpsc, thread, time::Duration};

mod ecs;

/// ==========================================
/// COMMANDS (UI -> ENGINE)
/// ==========================================
pub enum EditorCommand {
	InsertChar(char),
	Backspace,
	Scroll(i32),
	Quit,
}

fn main() -> Result<(), io::Error> {
	// 1. Setup Channels
	let (tx_cmd, rx_cmd) = mpsc::channel::<EditorCommand>();
	let (tx_view, rx_view) = mpsc::channel::<Vec<RenderToken>>();

	// 2. Spawn Engine Thread
	thread::spawn(move || {
		let mut registry = UastRegistry::new(1000);
		let mut chunk = registry.reserve_chunk(10).expect("Failed to reserve chunk");

		let root_id = chunk.spawn_node(
			SemanticKind::RelationalTable,
			None,
			SpanMetrics {
				byte_length: 8,
				newlines: 1,
			},
		);

		let child_id = chunk.spawn_node(
			SemanticKind::Token,
			Some(SvpPointer {
				offset: 0,
				length: 8,
			}), // "LINE1: S"
			SpanMetrics {
				byte_length: 8,
				newlines: 0,
			},
		);

		chunk.append_local_child(root_id, child_id);
		drop(chunk);

		let mut cursor_absolute_line = 0;
		let mut viewport_lines = 50;

		let mut cursor_node = child_id;
		let mut cursor_offset = 7;

		// Initial render
		let tokens = registry.query_viewport(root_id, cursor_absolute_line, viewport_lines);
		let _ = tx_view.send(tokens);

		// Engine Loop
		while let Ok(cmd) = rx_cmd.recv() {
			let mut needs_render = false;

			match cmd {
				EditorCommand::InsertChar(c) => {
					let mut buf = [0; 4];
					let s = c.encode_utf8(&mut buf);
					let (new_node, new_offset) =
						registry.insert_text(cursor_node, cursor_offset, s.as_bytes());
					cursor_node = new_node;
					cursor_offset = new_offset;
					needs_render = true;
				}
				EditorCommand::Backspace => {
					let (new_node, new_offset) =
						registry.delete_backwards(cursor_node, cursor_offset);
					cursor_node = new_node;
					cursor_offset = new_offset;
					needs_render = true;
				}
				EditorCommand::Scroll(delta) => {
					cursor_absolute_line = (cursor_absolute_line as i32 + delta).max(0) as u32;
					needs_render = true;
				}
				EditorCommand::Quit => break,
			}

			if needs_render {
				let tokens = registry.query_viewport(root_id, cursor_absolute_line, viewport_lines);
				let _ = tx_view.send(tokens);
			}
		}
	});

	// 3. Setup Frontend (Crossterm + Ratatui)
	enable_raw_mode()?;
	let mut stdout = io::stdout();
	execute!(stdout, EnterAlternateScreen)?;
	let backend = CrosstermBackend::new(stdout);
	let mut terminal = Terminal::new(backend)?;

	let mut current_tokens: Vec<RenderToken> = Vec::new();

	// 4. Main UI Loop
	let mut initial_draw = true;
	loop {
		let mut got_new_view = initial_draw;
		initial_draw = false;

		// Non-blocking drain of incoming viewport updates
		while let Ok(tokens) = rx_view.try_recv() {
			current_tokens = tokens;
			got_new_view = true;
		}

		if got_new_view {
			// Draw directly to the Frame buffer (no high-level widgets)
			terminal.draw(|f| {
				let buf = f.buffer_mut();
				let max_width = buf.area.width;
				let max_height = buf.area.height;

				let mut x = 0;
				let mut y = 0;

				for token in &current_tokens {
					// Map SemanticKind to Ratatui Styles
					let mut style = match token.kind {
						SemanticKind::Token => Style::default().fg(Color::LightGreen),
						SemanticKind::RelationalRow => Style::default().fg(Color::LightBlue),
						SemanticKind::RelationalTable => Style::default().fg(Color::LightMagenta),
					};

					// Highlight Virtual nodes (uncommitted text)
					if token.is_virtual {
						style = style.add_modifier(Modifier::ITALIC).fg(Color::Yellow);
					}

					for c in token.text.chars() {
						if y >= max_height {
							break;
						}
						if c == '\n' {
							y += 1;
							x = 0;
						} else {
							if x < max_width {
								// Direct buffer manipulation
								if let Some(cell) = buf.cell_mut((x, y)) {
									cell.set_char(c).set_style(style);
								}
							}
							x += 1;
						}
					}
					if y >= max_height {
						break;
					}
				}
			})?;
		}

		let mut should_quit = false;

		// Poll for input (60 FPS ~ 16ms timeout)
		if event::poll(Duration::from_millis(16))? {
			// Drain ALL currently available events so we don't fall behind
			loop {
				if let Event::Key(key) = event::read()? {
					if key.kind == KeyEventKind::Press {
						match key.code {
							KeyCode::Esc => {
								let _ = tx_cmd.send(EditorCommand::Quit);
								should_quit = true;
								break;
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

	// 5. Teardown
	disable_raw_mode()?;
	execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
	terminal.show_cursor()?;

	Ok(())
}
