use crate::svp::sync::*;

#[test]
fn test_valid_boundaries() {
	let buf = b"skip\n\nvalid code\n\nmore skip";
	let bounds = find_safe_parse_boundaries(buf).unwrap();
	assert_eq!(bounds, (6, 16));
	assert_eq!(&buf[bounds.0..bounds.1], b"valid code");
}

#[test]
fn test_invalid_utf8() {
	let mut buf = b"skip\n\nvalid code\n\nmore skip".to_vec();
	buf[8] = 0xFF; // Inject invalid UTF-8
	let bounds = find_safe_parse_boundaries(&buf);
	assert_eq!(bounds, None);
}

#[test]
fn test_no_boundaries() {
	let buf = b"skip\nvalid code\nmore skip";
	let bounds = find_safe_parse_boundaries(buf);
	assert_eq!(bounds, Some((0, 25)));
}

#[test]
fn test_single_boundary() {
	let buf = b"skip\n\nvalid code\nmore skip";
	let bounds = find_safe_parse_boundaries(buf);
	assert_eq!(bounds, Some((0, 26)));
}
