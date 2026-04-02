use super::Frontend;
use crate::core::{CursorPosition, DocLine, VisualCol};
use crate::engine::EditorCommand;
use crossterm::event::{self, MouseButton, MouseEventKind};
use ratatui::backend::Backend;
use std::io;

impl<B: Backend + io::Write> Frontend<B> {
	pub(super) fn handle_mouse(&mut self, mouse: event::MouseEvent) {
		match mouse.kind {
			MouseEventKind::ScrollUp => {
				let _ = self.tx_cmd.send(EditorCommand::ScrollViewport(-3));
				self.needs_redraw = true;
			}
			MouseEventKind::ScrollDown => {
				let _ = self.tx_cmd.send(EditorCommand::ScrollViewport(3));
				self.needs_redraw = true;
			}
			MouseEventKind::Down(MouseButton::Left) => {
				let view = match &self.current_viewport {
					Some(v) => v,
					None => return,
				};

				let max_height = self.terminal.size().map(|area| area.height).unwrap_or(1);
				let viewport_line_count = view.viewport_line_count.max(1);
				let render_line_count =
					viewport_line_count.min(max_height.saturating_sub(1) as u32) as u16;
				if mouse.row >= render_line_count {
					return;
				}
				let max_line_on_screen = view.scroll_y + viewport_line_count.saturating_sub(1);
				let digits = max_line_on_screen.max(1).ilog10() + 1;
				let gutter_width = digits as u16 + 1;

				let click_x = mouse.column;
				let click_y = mouse.row;
				if click_x < gutter_width {
					return;
				}

				let abs_line = view.scroll_y + click_y as u32;
				let target_visual_col = u32::from(click_x - gutter_width);

				let _ = self
					.tx_cmd
					.send(EditorCommand::ClickCursor(CursorPosition::new(
						DocLine::new(abs_line),
						VisualCol::new(target_visual_col),
					)));
				self.needs_redraw = true;
			}
			_ => {}
		}
	}
}
