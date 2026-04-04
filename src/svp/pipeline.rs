use crate::core::DocByte;
use crate::svp::highlight::{HighlightSpan, highlight_viewport};
use crate::svp::parse::parse_fragment;
use crate::svp::sync::{ViewportChunk, find_safe_parse_boundaries};

pub struct SvpPipeline;

impl SvpPipeline {
	pub fn process_viewport(global_offset: DocByte, buffer: &[u8]) -> Vec<HighlightSpan> {
		match find_safe_parse_boundaries(buffer) {
			Some(bounds) => {
				let chunk = ViewportChunk {
					global_offset,
					buffer: buffer.to_vec(),
				};

				let tree = parse_fragment(&chunk, bounds);
				highlight_viewport(&tree)
			}
			None => Vec::new(),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::SvpPipeline;
	use crate::core::DocByte;
	use crate::svp::highlight::TokenCategory;

	#[test]
	fn process_viewport_keeps_leading_import_block_colored() {
		let highlights = SvpPipeline::process_viewport(
			DocByte::ZERO,
			b"use std::io;\nuse std::fs;\n\nfn main() {}\n",
		);

		assert!(highlights.iter().any(|span| {
			span.category == TokenCategory::Keyword
				&& span.start == DocByte::ZERO
				&& span.end == DocByte::new(3)
		}));
		assert!(
			highlights
				.iter()
				.any(|span| { span.category == TokenCategory::Keyword && span.start.get() > 20 })
		);
	}
}
