use crate::svp::highlight::TokenCategory;
use ratatui::style::Color;

pub struct HighlightProjector {
	spans: Vec<(u64, u64, TokenCategory)>,
}

pub const WHITESPACE_COLOR: Color = Color::Rgb(76, 86, 106);

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
			TokenCategory::Keyword => Some(Color::Rgb(198, 120, 221)),
			TokenCategory::String => Some(Color::Rgb(152, 195, 121)),
			TokenCategory::Comment => Some(Color::Rgb(92, 99, 112)),
			TokenCategory::Number => Some(Color::Rgb(209, 154, 102)),
			TokenCategory::Function => Some(Color::Rgb(97, 175, 239)),
			TokenCategory::Type => Some(Color::Rgb(229, 192, 123)),
			TokenCategory::Variable => Some(Color::Rgb(224, 108, 117)),
			TokenCategory::Constant => Some(Color::Rgb(209, 154, 102)),
			TokenCategory::Macro => Some(Color::Rgb(86, 182, 194)),
			TokenCategory::Module => Some(Color::Rgb(97, 175, 239)),
			TokenCategory::Lifetime => Some(Color::Rgb(198, 120, 221)),
			TokenCategory::Attribute => Some(Color::Rgb(171, 178, 191)),
			TokenCategory::SelfKeyword => Some(Color::Rgb(224, 108, 117)),
			TokenCategory::BuiltinType => Some(Color::Rgb(86, 182, 194)),
			TokenCategory::MutableVariable => Some(Color::Rgb(209, 154, 102)),
			TokenCategory::Method => Some(Color::Rgb(97, 175, 239)),
			TokenCategory::Crate => Some(Color::Rgb(86, 182, 194)),
			TokenCategory::Whitespace => Some(Color::Rgb(76, 86, 106)),
			TokenCategory::Punctuation => Some(Color::Rgb(171, 178, 191)),
			TokenCategory::Operator => Some(Color::Rgb(86, 182, 194)),
			TokenCategory::Unclassified => None,
		}
	}
}
