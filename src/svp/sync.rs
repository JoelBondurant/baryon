use memchr::memmem;

pub struct ViewportChunk {
	pub global_offset: u64,
	pub buffer: Vec<u8>,
}

pub fn find_safe_parse_boundaries(buffer: &[u8]) -> Option<(usize, usize)> {
    let first_opt = memmem::find(buffer, b"\n\n");
    let start = first_opt.map(|f| f + 2).unwrap_or(0);
    let mut end = buffer.len();

    // UTF-8 Safety: An io_uring chunk might slice a multi-byte character exactly in half 
    // at the very end of the buffer. Retract end boundary until it's valid UTF-8.
    while end > start && std::str::from_utf8(&buffer[start..end]).is_err() {
        end -= 1;
    }

    if start >= end {
        return None;
    }

    Some((start, end))
}
