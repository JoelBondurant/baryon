use std::path::{Path, PathBuf};

/// Expands leading tildes (~) and handles relative paths.
pub fn expand_path(path: &str) -> PathBuf {
	if let Some(stripped) = path.strip_prefix("~/") {
		if let Ok(home) = std::env::var("HOME") {
			return Path::new(&home).join(stripped);
		}
	} else if path == "~" {
		if let Ok(home) = std::env::var("HOME") {
			return PathBuf::from(home);
		}
	}

	// Canonicalize handles ".." and "." but requires the path to exist.
	// For simplicity in this prototype, we'll just use PathBuf::from
	// which handles ".." correctly during standard file operations.
	PathBuf::from(path)
}
