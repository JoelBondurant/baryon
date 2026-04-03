use super::minimap::MinimapController;
use crate::engine::{EditorCommand, EditorMode};
use crate::uast::projection::Viewport;
use crossterm::cursor::SetCursorStyle;
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use ratatui::{Terminal, backend::Backend};
use std::io;
use std::sync::mpsc;
use std::time::Duration;

#[derive(Clone, Copy)]
pub(super) enum PendingOperator {
	Delete,
	Change,
}

pub struct Frontend<B: Backend + io::Write> {
	pub(super) terminal: Terminal<B>,
	pub(super) tx_cmd: mpsc::Sender<EditorCommand>,
	pub(super) rx_view: mpsc::Receiver<Viewport>,
	pub(super) current_viewport: Option<Viewport>,
	pub(super) current_mode: EditorMode,
	pub(super) command_buffer: String,
	pub(super) g_prefix: bool,
	pub(super) y_prefix: bool,
	pub(super) pending_operator: Option<PendingOperator>,
	pub(super) awaiting_inner_word: bool,
	pub(super) pending_register: Option<char>,
	pub(super) status_message: Option<String>,
	pub(super) needs_redraw: bool,
	pub(super) search_buffer: String,
	pub(super) minimap: MinimapController,
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
			pending_operator: None,
			awaiting_inner_word: false,
			pending_register: None,
			status_message: None,
			needs_redraw: false,
			search_buffer: String::new(),
			minimap: MinimapController::new(),
		}
	}

	pub fn run(&mut self) -> Result<(), io::Error> {
		self.terminal
			.clear()
			.map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
		let size = self
			.terminal
			.size()
			.map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
		let _ = self
			.tx_cmd
			.send(EditorCommand::Resize(size.width, size.height));
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
					if self.current_mode != mode {
						self.current_mode = mode;
						self.apply_cursor_style();
					}
				}
				self.current_viewport = Some(view);
				got_new_view = true;
			}

			if got_new_view || self.needs_redraw {
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
								EditorMode::Visual { .. } => {
									if self.handle_visual_key(
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
						Event::Resize(w, h) => {
							let _ = self.tx_cmd.send(EditorCommand::Resize(w, h));
							self.needs_redraw = true;
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

	pub(super) fn clear_prefixes(&mut self) {
		self.g_prefix = false;
		self.y_prefix = false;
		self.pending_operator = None;
		self.awaiting_inner_word = false;
		self.pending_register = None;
	}

	pub(super) fn clear_operator_pending(&mut self) {
		self.pending_operator = None;
		self.awaiting_inner_word = false;
	}

	pub(super) fn apply_cursor_style(&mut self) {
		let style = match self.current_mode {
			EditorMode::Normal
			| EditorMode::Command
			| EditorMode::Search
			| EditorMode::Confirm
			| EditorMode::Visual { .. } => SetCursorStyle::SteadyBlock,
			EditorMode::Insert => SetCursorStyle::SteadyBar,
		};
		let _ = execute!(self.terminal.backend_mut(), style);
	}

	pub fn release_terminal(self) -> Terminal<B> {
		self.terminal
	}
}
