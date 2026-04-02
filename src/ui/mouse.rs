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

				let scroll_y = view.cursor_abs_pos.line.saturating_sub(20);
				let max_line = scroll_y.get() + view.total_lines;
				let digits = max_line.max(1).ilog10() + 1;
				let gutter_width = digits as u16 + 1;

				let click_x = mouse.column;
				let click_y = mouse.row;
				if click_x < gutter_width {
					return;
				}

				let abs_line = scroll_y.get() + click_y as u32;
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
