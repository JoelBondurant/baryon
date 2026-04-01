use memchr::memmem;

pub struct ViewportChunk {
	pub global_offset: u64,
	pub buffer: Vec<u8>,
}

pub fn find_safe_parse_boundaries(buffer: &[u8]) -> Option<(usize, usize)> {
	let first_opt = memmem::find(buffer, b"\n\n");
	let last_opt = memmem::rfind(buffer, b"\n\n");

	let start = first_opt.map(|f| f + 2).unwrap_or(0);
	let end = last_opt.unwrap_or(buffer.len());

	if start >= end {
		// Fallback: if we don't have distinct boundaries, try parsing the whole buffer
		if std::str::from_utf8(buffer).is_ok() {
			return Some((0, buffer.len()));
		}
		return None;
	}

	if std::str::from_utf8(&buffer[start..end]).is_ok() {
		Some((start, end))
	} else {
		// If the inner slice is invalid, try the whole buffer as a last resort
		if std::str::from_utf8(buffer).is_ok() {
			Some((0, buffer.len()))
		} else {
			None
		}
	}
}
