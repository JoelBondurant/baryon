use crate::svp::highlight::{TokenCategory, highlight_viewport};
use crate::svp::parse::parse_fragment;
use crate::svp::sync::{ViewportChunk, find_safe_parse_boundaries};

pub struct SvpPipeline;

impl SvpPipeline {
	pub fn process_viewport(global_offset: u64, buffer: &[u8]) -> Vec<(u64, u64, TokenCategory)> {
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
