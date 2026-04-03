use crate::core::DocByte;
use crate::svp::highlight::{HighlightSpan, TokenCategory};
use std::collections::HashMap;
use ratatui::style::Color;

pub struct HighlightProjector {
	spans: Vec<HighlightSpan>,
	theme_colors: HashMap<TokenCategory, Color>,
}

impl HighlightProjector {
	pub fn new(mut spans: Vec<HighlightSpan>, theme_colors: HashMap<TokenCategory, Color>) -> Self {
		spans.sort_by_key(|span| span.start);
		Self { spans, theme_colors }
	}

	pub fn style_for_byte(&self, byte_offset: DocByte) -> Option<Color> {
		let idx = self.spans.partition_point(|span| span.start <= byte_offset);
		if idx > 0 {
			let span = &self.spans[idx - 1];
			if byte_offset < span.end {
				return self.theme_colors.get(&span.category).copied();
			}
		}
		None
	}
}
