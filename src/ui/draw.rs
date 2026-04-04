use super::Frontend;
use super::minimap::render_byte_fallback_snapshot;
use crate::core::TAB_SIZE;
use crate::engine::{EditorMode, VisualKind};
use crate::svp::diagnostic::DiagnosticSeverity;
use crate::svp::projector::{DiagnosticProjector, HighlightProjector};
use crate::uast::MinimapMode;
use crate::uast::kind::SemanticKind;
use crate::ui::*;
use ratatui::{
	backend::Backend,
	buffer::Buffer,
	layout::Rect,
	style::{Modifier, Style},
};
use regex_automata::meta::Regex;
use regex_automata::util::syntax;
use std::io;

fn draw_gutter_line_number(
	buf: &mut Buffer,
	gutter_width: u16,
	y: usize,
	line_index: u32,
	total_newlines: u32,
	gutter_style: Style,
) {
	if y > u16::MAX as usize || line_index > total_newlines {
		return;
	}

	let line_str = (line_index + 1).to_string();
	if line_str.len() >= gutter_width as usize {
		return;
	}

	let start_x = (gutter_width - 1).saturating_sub(line_str.len() as u16);
	for (i, c) in line_str.chars().enumerate() {
		if let Some(cell) = buf.cell_mut((start_x + i as u16, y as u16)) {
			cell.set_char(c).set_style(gutter_style);
		}
	}
}

fn paint_wrapped_text(
	buf: &mut Buffer,
	x: &mut usize,
	y: &mut usize,
	text_left: usize,
	text_right: usize,
	render_height: usize,
	text: &str,
	style: Style,
) -> bool {
	if text_right <= text_left {
		return *y < render_height;
	}

	for c in text.chars() {
		if *x >= text_right {
			*y += 1;
			*x = text_left;
		}
		if *y >= render_height {
			return false;
		}
		if let Some(cell) = buf.cell_mut((*x as u16, *y as u16)) {
			cell.set_char(c).set_style(style);
		}
		*x += 1;
	}

	*y < render_height
}

fn next_utf8_chunk(bytes: &[u8]) -> Option<(char, usize)> {
	let max_len = bytes.len().min(4);
	for len in 1..=max_len {
		let prefix = &bytes[..len];
		if let Ok(text) = std::str::from_utf8(prefix) {
			if let Some(ch) = text.chars().next() {
				if ch.len_utf8() == len {
					return Some((ch, len));
				}
			}
		}
	}
	None
}

fn segment_overlaps(ranges: &[(usize, usize)], start: usize, len: usize) -> bool {
	let end = start.saturating_add(len);
	ranges
		.iter()
		.any(|&(range_start, range_end)| range_start < end && range_end > start)
}

fn apply_diagnostic_style(style: Style, severity: Option<DiagnosticSeverity>) -> Style {
	match severity {
		Some(DiagnosticSeverity::Error) => style
			.add_modifier(Modifier::UNDERLINED)
			.underline_color(DIAGNOSTIC_ERROR_UNDERLINE),
		Some(_) | None => style,
	}
}

fn folded_placeholder_style() -> Style {
	Style::default()
		.fg(FOLDED_PLACEHOLDER_FG)
		.bg(FOLDED_PLACEHOLDER_BG)
		.add_modifier(Modifier::BOLD)
}

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

						let digits = view.total_lines.max(1).ilog10() + 1;
						let gutter_width: u16 = digits as u16 + 1;
						let gutter_style = Style::default().bg(GUTTER_BG).fg(GUTTER_FG);
						let text_left = gutter_width as usize;
						let render_height = (max_height as usize).saturating_sub(1);

						for gy in 0..render_height as u16 {
							for gx in 0..gutter_width {
								if let Some(cell) = buf.cell_mut((gx, gy)) {
									cell.set_char(' ').set_style(gutter_style);
								}
							}
						}

						if minimap_width > 0 {
							let separator_x =
								max_width.saturating_sub(minimap_width + separator_width);
							for gy in 0..max_height.saturating_sub(1) {
								if let Some(cell) = buf.cell_mut((separator_x, gy)) {
									cell.set_char(' ')
										.set_style(Style::default().bg(MINIMAP_BG).fg(MINIMAP_BG));
								}
								for gx in minimap_area.x..(minimap_area.x + minimap_area.width) {
									if let Some(cell) = buf.cell_mut((gx, gy)) {
										cell.set_char(' ')
											.set_style(Style::default().bg(BG).fg(BG));
									}
								}
							}
						}

						let mut x = text_left;
						let mut y: usize = 0;
						let mut current_doc_line = view.viewport_start_line.get();
						if render_height > 0 {
							draw_gutter_line_number(
								buf,
								gutter_width,
								y,
								current_doc_line,
								view.total_lines,
								gutter_style,
							);
						}

						let search_pat = view.search_pattern.as_deref().unwrap_or("");
						let projector = HighlightProjector::new(
							view.highlights.clone(),
							view.theme_colors.clone(),
						);
						let diagnostic_projector =
							DiagnosticProjector::new(view.diagnostics.clone());
						let mut current_global_byte = view.global_start_byte;

						for token in &view.tokens {
							if x != text_left
								&& token.absolute_start_line.get() > current_doc_line
								&& y < render_height
							{
								y += 1;
								x = text_left;
							}
							if x == text_left && token.absolute_start_line.get() != current_doc_line
							{
								current_doc_line = token.absolute_start_line.get();
								if y < render_height {
									draw_gutter_line_number(
										buf,
										gutter_width,
										y,
										current_doc_line,
										view.total_lines,
										gutter_style,
									);
								}
							}

							let base_style = match token.kind {
								SemanticKind::Token => Style::default().fg(TEXT_FG),
								_ => Style::default().fg(TEXT_FG),
							};
							let virtual_style = base_style;
							let has_physical_bytes = !token.text.is_empty() && !token.is_folded;
							let text = if token.is_folded {
								token.text.as_slice()
							} else if has_physical_bytes {
								token.text.as_slice()
							} else {
								b"[DMA PENDING...]"
							};

							if token.is_folded {
								let mut style = folded_placeholder_style();
								let folded_start = token.absolute_start_byte;
								let folded_end_exclusive =
									folded_start.saturating_add(token.physical_byte_len as u64);
								let folded_end = folded_end_exclusive.saturating_sub(1);
								let in_selection =
									view.selection_ranges.iter().any(|(start, end)| {
										*start <= folded_end && *end >= folded_start
									});
								if in_selection {
									style = style.bg(SELECTION_BG);
								}
								if let Some((flash_start, flash_end)) = view.yank_flash {
									if flash_start <= folded_end && flash_end >= folded_start {
										style =
											Style::default().bg(MODE_NORMAL_BG).fg(MODE_TEXT_FG);
									}
								}
								style = apply_diagnostic_style(
									style,
									diagnostic_projector
										.severity_for_range(folded_start, folded_end_exclusive),
								);

								let display = std::str::from_utf8(text).unwrap_or("❯❯❯");
								if !paint_wrapped_text(
									buf,
									&mut x,
									&mut y,
									text_left,
									text_right,
									render_height,
									display,
									style,
								) {
									y = render_height;
								}
								current_global_byte = token
									.absolute_start_byte
									.saturating_add(token.physical_byte_len as u64);
								continue;
							}

							let highlight_style =
								Style::default().bg(SEARCH_MATCH_BG).fg(SEARCH_MATCH_FG);
							let search_ci = view.search_case_insensitive;
							let mut highlight_ranges: Vec<(usize, usize)> = Vec::new();
							if !search_pat.is_empty() && has_physical_bytes {
								if let Ok(re) = Regex::builder()
									.syntax(syntax::Config::new().case_insensitive(search_ci))
									.build(search_pat)
								{
									for m in re.find_iter(text) {
										highlight_ranges.push((m.start(), m.end()));
									}
								}
							}

							let mut byte_idx = 0usize;
							while byte_idx < text.len() {
								if y >= render_height {
									break;
								}

								let remaining = &text[byte_idx..];
								let (chunk_kind, chunk_len) =
									if let Some((ch, ch_len)) = next_utf8_chunk(remaining) {
										if ch == '\n' {
											(0u8, 1usize)
										} else if ch == '\t' {
											(1u8, 1usize)
										} else if ch.is_control() {
											(2u8, ch_len)
										} else {
											(3u8, ch_len)
										}
									} else {
										(2u8, 1usize)
									};

								let in_highlight =
									segment_overlaps(&highlight_ranges, byte_idx, chunk_len);
								let mut style = if in_highlight {
									highlight_style
								} else {
									virtual_style
								};

								if !in_highlight && has_physical_bytes {
									if let Some(fg) = projector.style_for_byte(current_global_byte)
									{
										style = style.fg(fg);
									}
								}

								if has_physical_bytes {
									let chunk_start_byte = current_global_byte;
									let chunk_end_exclusive =
										current_global_byte.saturating_add(chunk_len as u64);
									let chunk_end_byte = chunk_end_exclusive.saturating_sub(1);
									let in_selection =
										view.selection_ranges.iter().any(|(start, end)| {
											*start <= chunk_end_byte && *end >= chunk_start_byte
										});
									if in_selection {
										style = style.bg(SELECTION_BG);
									}

									if let Some((flash_start, flash_end)) = view.yank_flash {
										if flash_start <= chunk_end_byte
											&& flash_end >= chunk_start_byte
										{
											style = Style::default()
												.bg(MODE_NORMAL_BG)
												.fg(MODE_TEXT_FG);
										}
									}
									style = apply_diagnostic_style(
										style,
										diagnostic_projector.severity_for_range(
											chunk_start_byte,
											chunk_end_exclusive,
										),
									);
								}

								match chunk_kind {
									0 => {
										byte_idx += 1;
										if has_physical_bytes {
											current_global_byte =
												current_global_byte.saturating_add(1);
										}
										y += 1;
										x = text_left;
										current_doc_line = current_doc_line.saturating_add(1);
										if y < render_height {
											draw_gutter_line_number(
												buf,
												gutter_width,
												y,
												current_doc_line,
												view.total_lines,
												gutter_style,
											);
										}
									}
									1 => {
										let col = x.saturating_sub(text_left);
										let spaces_to_add =
											TAB_SIZE as usize - (col % TAB_SIZE as usize);
										let ws_style = style.fg(SYNTAX_WHITESPACE);
										if !paint_wrapped_text(
											buf,
											&mut x,
											&mut y,
											text_left,
											text_right,
											render_height,
											"\u{25B8}",
											ws_style,
										) {
											y = render_height;
										}
										for _ in 1..spaces_to_add {
											if !paint_wrapped_text(
												buf,
												&mut x,
												&mut y,
												text_left,
												text_right,
												render_height,
												" ",
												ws_style,
											) {
												y = render_height;
												break;
											}
										}
										byte_idx += 1;
										if has_physical_bytes {
											current_global_byte =
												current_global_byte.saturating_add(1);
										}
									}
									2 => {
										let ws_style = style.fg(SYNTAX_WHITESPACE);
										for &byte in &remaining[..chunk_len] {
											let hex = format!("<{:02X}>", byte);
											if !paint_wrapped_text(
												buf,
												&mut x,
												&mut y,
												text_left,
												text_right,
												render_height,
												&hex,
												ws_style,
											) {
												y = render_height;
												break;
											}
										}
										byte_idx += chunk_len;
										if has_physical_bytes {
											current_global_byte = current_global_byte
												.saturating_add(chunk_len as u64);
										}
									}
									_ => {
										let (ch, ch_len) = next_utf8_chunk(remaining)
											.expect("printable chunk must decode");
										let mut encoded = [0u8; 4];
										let display = ch.encode_utf8(&mut encoded);
										if !paint_wrapped_text(
											buf,
											&mut x,
											&mut y,
											text_left,
											text_right,
											render_height,
											display,
											style,
										) {
											y = render_height;
										}
										byte_idx += ch_len;
										if has_physical_bytes {
											current_global_byte =
												current_global_byte.saturating_add(ch_len as u64);
										}
									}
								}
							}
							if y >= render_height {
								break;
							}
						}

						let visual_cursor_y = view.cursor_visual_row as u16;
						let visual_cursor_x = (view.cursor_abs_pos.col.get() as u16)
							.checked_add(gutter_width)
							.unwrap_or(text_right as u16);
						if max_height > 1 && text_right > text_left {
							let max_cursor_y = max_height.saturating_sub(2);
							let max_cursor_x = text_right.saturating_sub(1) as u16;
							cursor_to_set = Some((
								visual_cursor_x.min(max_cursor_x),
								visual_cursor_y.min(max_cursor_y),
							));
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
					if snapshot.mode == MinimapMode::ByteFallback {
						let buf = f.buffer_mut();
						render_byte_fallback_snapshot(buf, minimap_area, snapshot);
					} else {
						minimap.request(snapshot, minimap_area);
						minimap.render(f, minimap_area);
					}
				}

				let bar_y = max_height.saturating_sub(1);
				let bar_bg = Style::default().bg(STATUS_BAR_BG).fg(STATUS_BAR_FG);
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
					let mode_style = bar_bg.fg(MODE_TEXT_FG).bg(match current_mode {
						EditorMode::Normal => MODE_NORMAL_BG,
						EditorMode::Insert => MODE_INSERT_BG,
						EditorMode::Command => MODE_COMMAND_BG,
						EditorMode::Search => MODE_SEARCH_BG,
						EditorMode::Confirm => MODE_CONFIRM_BG,
						EditorMode::Visual { .. } => MODE_VISUAL_BG,
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
						let name_style = if dirty { bar_bg.fg(DIRTY_FG) } else { bar_bg };
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
										.set_style(bar_bg.fg(DIRTY_BULLET_FG));
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
						let dim_style = bar_bg.fg(SEARCH_DIM_FG);
						let search_style = bar_bg.fg(SEARCH_INFO_FG);
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

				f.set_cursor_position(cursor_to_set.unwrap_or((0, 0)));
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

#[cfg(test)]
mod tests {
	use super::{apply_diagnostic_style, folded_placeholder_style};
	use crate::svp::diagnostic::DiagnosticSeverity;
	use crate::ui::{DIAGNOSTIC_ERROR_UNDERLINE, FOLDED_PLACEHOLDER_BG, FOLDED_PLACEHOLDER_FG};
	use ratatui::style::{Modifier, Style};

	#[test]
	fn error_diagnostics_apply_a_red_underline_overlay() {
		let style = apply_diagnostic_style(Style::default(), Some(DiagnosticSeverity::Error));
		assert!(style.add_modifier.contains(Modifier::UNDERLINED));
		assert_eq!(style.underline_color, Some(DIAGNOSTIC_ERROR_UNDERLINE));
	}

	#[test]
	fn folded_placeholder_uses_dedicated_palette() {
		let style = folded_placeholder_style();
		assert_eq!(style.fg, Some(FOLDED_PLACEHOLDER_FG));
		assert_eq!(style.bg, Some(FOLDED_PLACEHOLDER_BG));
		assert!(style.add_modifier.contains(Modifier::BOLD));
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
