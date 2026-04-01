use crate::svp::highlight::TokenCategory;
use ratatui::style::Color;

pub struct HighlightProjector {
	spans: Vec<(u64, u64, TokenCategory)>,
}

impl HighlightProjector {
	pub fn new(mut spans: Vec<(u64, u64, TokenCategory)>) -> Self {
		spans.sort_by_key(|span| span.0);
		Self { spans }
	}

	pub fn style_for_byte(&self, byte_offset: u64) -> Option<Color> {
		let idx = self.spans.partition_point(|span| span.0 <= byte_offset);
		if idx > 0 {
			let span = &self.spans[idx - 1];
			if byte_offset < span.1 {
				return Self::color_for_category(span.2);
			}
		}
		None
	}

	fn color_for_category(category: TokenCategory) -> Option<Color> {
		match category {
			TokenCategory::Keyword => Some(Color::Magenta),
			TokenCategory::String => Some(Color::Green),
			TokenCategory::Comment => Some(Color::DarkGray),
			TokenCategory::Number => Some(Color::Yellow),
			TokenCategory::Function => Some(Color::Blue),
			TokenCategory::Type => Some(Color::Cyan),
			TokenCategory::Punctuation => None,
			TokenCategory::Unclassified => None,
		}
	}
}
