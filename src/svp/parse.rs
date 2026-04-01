use crate::core::DocByte;
use crate::svp::sync::ViewportChunk;
use ra_ap_syntax::{AstNode, SourceFile, SyntaxNode, TextRange};

pub struct ViewportTree {
	pub global_chunk_offset: DocByte,
	pub fragment_start: usize,
	pub tree: SyntaxNode,
}

pub fn parse_fragment(chunk: &ViewportChunk, bounds: (usize, usize)) -> ViewportTree {
	let (start, end) = bounds;

	// Psycho-optimization: bypass secondary validation.
	// The bounds were already verified as valid UTF-8 in find_safe_parse_boundaries.
	let text = unsafe { std::str::from_utf8_unchecked(&chunk.buffer[start..end]) };

	let parse = SourceFile::parse(text, ra_ap_syntax::Edition::Edition2024);

	ViewportTree {
		global_chunk_offset: chunk.global_offset,
		fragment_start: start,
		tree: parse.tree().syntax().clone(),
	}
}

impl ViewportTree {
	pub fn local_to_global(&self, local_range: TextRange) -> (DocByte, DocByte) {
		let start: u32 = local_range.start().into();
		let end: u32 = local_range.end().into();

		let global_start = self
			.global_chunk_offset
			.saturating_add(self.fragment_start as u64)
			.saturating_add(start as u64);
		let global_end = self
			.global_chunk_offset
			.saturating_add(self.fragment_start as u64)
			.saturating_add(end as u64);

		(global_start, global_end)
	}
}
