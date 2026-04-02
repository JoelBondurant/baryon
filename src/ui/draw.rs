use super::Frontend;
use crate::core::TAB_SIZE;
use crate::engine::{EditorMode, VisualKind};
use crate::svp::projector::{HighlightProjector, WHITESPACE_COLOR};
use crate::uast::kind::SemanticKind;
use ratatui::{
	backend::Backend,
	layout::Rect,
	style::{Color, Style},
};
use regex_automata::meta::Regex;
use regex_automata::util::syntax;
use std::io;

impl<B: Backend + io::Write> Frontend<B> {
	pub(super) fn draw(&mut self) -> Result<(), B::Error> {
		let current_viewport = &self.current_viewport;
		let current_mode = &self.current_mode;
		let status_message = &self.status_message;
		let command_buffer = &self.command_buffer;
		let search_buffer = &self.search_buffer;
		let g_prefix = self.g_prefix;
		self.minimap.poll();
		let minimap = &mut self.minimap;

		{
			let backend = self.terminal.backend_mut();
			backend.hide_cursor()?;
			Backend::flush(backend)?;
		}

		let draw_result = self
			.terminal
			.draw(|f| {
				let mut cursor_to_set = None;
				let mut pending_minimap_area: Option<Rect> = None;
				let max_width = f.area().width;
				let max_height = f.area().height;

				{
					let buf = f.buffer_mut();
					if let Some(view) = current_viewport {
						let scroll_y = view.scroll_y;
						let viewport_line_count = view.viewport_line_count.max(1);
						let render_line_count =
							viewport_line_count.min(max_height.saturating_sub(1) as u32) as u16;
						let max_line_on_screen = scroll_y + viewport_line_count.saturating_sub(1);
						let minimap_width = if view.minimap.is_some() && max_width > 40 {
							14u16.min(max_width.saturating_sub(24))
						} else {
							0
						};
						let separator_width = if minimap_width > 0 { 1 } else { 0 };
						let text_right =
							max_width.saturating_sub(minimap_width + separator_width) as usize;
						let minimap_area = Rect {
							x: max_width.saturating_sub(minimap_width),
							y: 0,
							width: minimap_width,
							height: max_height.saturating_sub(1),
						};

						let digits = max_line_on_screen.max(1).ilog10() + 1;
						let gutter_width: u16 = digits as u16 + 1;
						let gutter_style = Style::default()
							.bg(Color::Rgb(18, 18, 18))
							.fg(Color::Indexed(242));

						for gy in 0..render_line_count {
							for gx in 0..gutter_width {
								if let Some(cell) = buf.cell_mut((gx, gy)) {
									cell.set_char(' ').set_style(gutter_style);
								}
							}
							let line_num = scroll_y + gy as u32 + 1;
							if line_num <= view.total_lines + 1 {
								let line_str = line_num.to_string();
								if line_str.len() < gutter_width as usize {
									let start_x =
										(gutter_width - 1).saturating_sub(line_str.len() as u16);
									for (i, c) in line_str.chars().enumerate() {
										if let Some(cell) = buf.cell_mut((start_x + i as u16, gy)) {
											cell.set_char(c);
										}
									}
								}
							}
						}

						if minimap_width > 0 {
							let separator_x =
								max_width.saturating_sub(minimap_width + separator_width);
							for gy in 0..max_height.saturating_sub(1) {
								if let Some(cell) = buf.cell_mut((separator_x, gy)) {
									cell.set_char(' ').set_style(
										Style::default()
											.bg(Color::Rgb(16, 20, 28))
											.fg(Color::Rgb(16, 20, 28)),
									);
								}
								for gx in minimap_area.x..(minimap_area.x + minimap_area.width) {
									if let Some(cell) = buf.cell_mut((gx, gy)) {
										cell.set_char(' ').set_style(
											Style::default()
												.bg(Color::Rgb(18, 18, 18))
												.fg(Color::Rgb(18, 18, 18)),
										);
									}
								}
							}
						}

						let mut x: usize = gutter_width as usize;
						let mut y: usize = 0;
						let render_height = (max_height as usize).saturating_sub(1);

						let search_pat = view.search_pattern.as_deref().unwrap_or("");
						let selection_bg = Color::Rgb(62, 68, 82);
						let projector = HighlightProjector::new(view.highlights.clone());
						let mut current_global_byte = view.global_start_byte;

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
								if !in_highlight && !token.text.is_empty() {
									if let Some(fg) = projector.style_for_byte(current_global_byte)
									{
										style = style.fg(fg);
									}
								}

								let char_len = c.len_utf8();
								let char_start_byte = current_global_byte;
								let char_end_byte = current_global_byte
									.saturating_add(char_len as u64)
									.saturating_sub(1);
								let in_selection =
									view.selection_ranges.iter().any(|(start, end)| {
										*start <= char_end_byte && *end >= char_start_byte
									});
								if in_selection {
									style = style.bg(selection_bg);
								}

								if let Some((flash_start, flash_end)) = view.yank_flash {
									if current_global_byte >= flash_start
										&& current_global_byte < flash_end
									{
										style = Style::default()
											.bg(Color::Rgb(229, 192, 123))
											.fg(Color::Black);
									}
								}

								let is_trailing_space = c == ' ' && {
									let rest = &text.as_bytes()[byte_idx + char_len..];
									rest.is_empty() || rest[0] == b'\n'
								};

								byte_idx += char_len;
								current_global_byte =
									current_global_byte.saturating_add(char_len as u64);

								if y >= render_height {
									break;
								}

								let ws_style = style.fg(WHITESPACE_COLOR);

								if c == '\n' {
									if x < text_right {
										if let Some(cell) = buf.cell_mut((x as u16, y as u16)) {
											cell.set_char('\u{00AC}').set_style(ws_style);
										}
									}
									y += 1;
									x = gutter_width as usize;
								} else if c == '\t' {
									let col = x - gutter_width as usize;
									let spaces_to_add =
										TAB_SIZE as usize - (col % TAB_SIZE as usize);
									if x < text_right {
										if let Some(cell) = buf.cell_mut((x as u16, y as u16)) {
											cell.set_char('\u{25B8}').set_style(ws_style);
										}
									}
									x += 1;
									for _ in 1..spaces_to_add {
										if x < text_right {
											if let Some(cell) = buf.cell_mut((x as u16, y as u16)) {
												cell.set_char(' ').set_style(ws_style);
											}
										}
										x += 1;
									}
								} else if is_trailing_space {
									if x < text_right {
										if let Some(cell) = buf.cell_mut((x as u16, y as u16)) {
											cell.set_char('~').set_style(ws_style);
										}
									}
									x += 1;
								} else if c == ' ' {
									if x < text_right {
										if let Some(cell) = buf.cell_mut((x as u16, y as u16)) {
											cell.set_char('\u{2423}').set_style(ws_style);
										}
									}
									x += 1;
								} else {
									if x < text_right {
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

						let visual_cursor_y =
							view.cursor_abs_pos.line.saturating_sub(scroll_y).get() as u16;
						let visual_cursor_x = (view.cursor_abs_pos.col.get() as u16)
							.checked_add(gutter_width)
							.unwrap_or(text_right as u16);
						if visual_cursor_y < max_height - 1 && visual_cursor_x < text_right as u16 {
							cursor_to_set = Some((visual_cursor_x, visual_cursor_y));
						}

						if view.minimap.is_some()
							&& minimap_area.width > 0
							&& minimap_area.height > 0
						{
							pending_minimap_area = Some(minimap_area);
						}
					}
				}

				if let (Some(view), Some(minimap_area), Some(snapshot)) = (
					current_viewport,
					pending_minimap_area,
					current_viewport
						.as_ref()
						.and_then(|view| view.minimap.as_ref()),
				) {
					let _ = view;
					minimap.request(snapshot, minimap_area);
					minimap.render(f, minimap_area);
				}

				let bar_y = max_height.saturating_sub(1);
				let bar_bg = Style::default().bg(Color::Rgb(18, 18, 18)).fg(Color::White);
				let w = max_width as usize;
				let buf = f.buffer_mut();

				for sx in 0..max_width {
					if let Some(cell) = buf.cell_mut((sx, bar_y)) {
						cell.set_char(' ').set_style(bar_bg);
					}
				}

				if let Some(msg) = status_message {
					for (i, c) in msg.chars().enumerate() {
						if i >= w {
							break;
						}
						if let Some(cell) = buf.cell_mut((i as u16, bar_y)) {
							cell.set_char(c);
						}
					}
				} else {
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
						EditorMode::Visual { kind, .. } => match kind {
							VisualKind::Char => "VIS",
							VisualKind::Line => "VIS-LN",
							VisualKind::Block => "VIS-BL",
						},
					};
					let mode_style = bar_bg.fg(Color::Rgb(0, 0, 0)).bg(match current_mode {
						EditorMode::Normal => Color::Rgb(130, 170, 255),
						EditorMode::Insert => Color::Rgb(180, 230, 130),
						EditorMode::Command => Color::Rgb(255, 180, 100),
						EditorMode::Search => Color::Rgb(200, 160, 255),
						EditorMode::Confirm => Color::Rgb(255, 120, 120),
						EditorMode::Visual { .. } => Color::Rgb(120, 200, 200),
					});

					let mut x = 0usize;
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

					if x < w {
						if let Some(cell) = buf.cell_mut((x as u16, bar_y)) {
							cell.set_char(' ').set_style(bar_bg);
						}
						x += 1;
					}

					let (file_name, file_sz, dirty) = current_viewport
						.as_ref()
						.map(|v| (v.file_name.as_deref(), v.file_size, v.is_dirty))
						.unwrap_or((None, 0, false));

					if matches!(current_mode, EditorMode::Command) {
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
					} else if matches!(current_mode, EditorMode::Search) {
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
					} else if matches!(current_mode, EditorMode::Confirm) {
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

					let (cursor_line, cursor_col) = current_viewport
						.as_ref()
						.map(|v| {
							(
								v.cursor_abs_pos.line.get() + 1,
								v.cursor_abs_pos.col.get() + 1,
							)
						})
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
			})
			.map(|_| ());

		let show_result = {
			let backend = self.terminal.backend_mut();
			match backend.show_cursor() {
				Ok(()) => Backend::flush(backend),
				Err(err) => Err(err),
			}
		};

		match (draw_result, show_result) {
			(Err(err), _) => Err(err),
			(Ok(_), Err(err)) => Err(err),
			(Ok(_), Ok(())) => Ok(()),
		}
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
