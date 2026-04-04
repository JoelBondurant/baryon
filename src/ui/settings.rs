use super::Theme;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_THEME_NAME: &str = "onedark";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoadedSettings {
	pub(crate) theme_name: String,
	pub(crate) settings_path: Option<PathBuf>,
	pub(crate) startup_status: Option<String>,
}

pub(crate) fn load_settings() -> LoadedSettings {
	load_settings_from_path(default_settings_path())
}

fn load_settings_from_path(settings_path: Option<PathBuf>) -> LoadedSettings {
	let Some(path) = settings_path.clone() else {
		return LoadedSettings {
			theme_name: DEFAULT_THEME_NAME.to_string(),
			settings_path: None,
			startup_status: None,
		};
	};

	match fs::read_to_string(&path) {
		Ok(contents) => match parse_settings(&contents) {
			Ok(Some(theme_name)) => {
				if Theme::try_new(&theme_name).is_ok() {
					LoadedSettings {
						theme_name,
						settings_path: Some(path),
						startup_status: None,
					}
				} else {
					LoadedSettings {
						theme_name: DEFAULT_THEME_NAME.to_string(),
						settings_path: Some(path),
						startup_status: Some(format!(
							"Invalid theme in settings.toml; using {}",
							DEFAULT_THEME_NAME
						)),
					}
				}
			}
			Ok(None) => LoadedSettings {
				theme_name: DEFAULT_THEME_NAME.to_string(),
				settings_path: Some(path),
				startup_status: None,
			},
			Err(err) => LoadedSettings {
				theme_name: DEFAULT_THEME_NAME.to_string(),
				settings_path: Some(path),
				startup_status: Some(format!("Settings error: {err}")),
			},
		},
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => LoadedSettings {
			theme_name: DEFAULT_THEME_NAME.to_string(),
			settings_path: Some(path),
			startup_status: None,
		},
		Err(err) => LoadedSettings {
			theme_name: DEFAULT_THEME_NAME.to_string(),
			settings_path: Some(path),
			startup_status: Some(format!("Settings error: {err}")),
		},
	}
}

pub(crate) fn persist_theme_name(settings_path: Option<&Path>, theme_name: &str) -> Result<(), String> {
	let path = settings_path.ok_or_else(|| "No config directory available".to_string())?;
	if let Some(parent) = path.parent() {
		fs::create_dir_all(parent).map_err(|e| format!("Failed to create config dir: {e}"))?;
	}
	fs::write(path, render_settings(theme_name))
		.map_err(|e| format!("Failed to write settings.toml: {e}"))
}

fn render_settings(theme_name: &str) -> String {
	format!("[ui]\ntheme = \"{}\"\n", escape_toml_string(theme_name))
}

fn default_settings_path() -> Option<PathBuf> {
	default_settings_path_from_env(env::var_os("XDG_CONFIG_HOME"), env::var_os("HOME"))
}

fn default_settings_path_from_env(
	xdg_config_home: Option<std::ffi::OsString>,
	home: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
	if let Some(xdg) = xdg_config_home.filter(|value| !value.is_empty()) {
		return Some(PathBuf::from(xdg).join("baryon").join("settings.toml"));
	}
	home.filter(|value| !value.is_empty())
		.map(PathBuf::from)
		.map(|home| home.join(".config").join("baryon").join("settings.toml"))
}

fn parse_settings(contents: &str) -> Result<Option<String>, String> {
	let mut in_ui_section = false;
	let mut theme_name = None;

	for (idx, raw_line) in contents.lines().enumerate() {
		let line_no = idx + 1;
		let line = strip_toml_comment(raw_line).trim();
		if line.is_empty() {
			continue;
		}

		if line.starts_with('[') {
			let section = line
				.strip_prefix('[')
				.and_then(|rest| rest.strip_suffix(']'))
				.ok_or_else(|| format!("line {line_no}: invalid section header"))?;
			in_ui_section = section.trim() == "ui";
			continue;
		}

		if !in_ui_section {
			continue;
		}

		let Some((key, value)) = line.split_once('=') else {
			return Err(format!("line {line_no}: expected key = value"));
		};
		if key.trim() != "theme" {
			continue;
		}

		theme_name = Some(parse_toml_string(value.trim(), line_no)?);
	}

	Ok(theme_name)
}

fn strip_toml_comment(line: &str) -> &str {
	let mut in_string = false;
	let mut escaped = false;

	for (idx, ch) in line.char_indices() {
		if in_string {
			if escaped {
				escaped = false;
				continue;
			}
			match ch {
				'\\' => escaped = true,
				'"' => in_string = false,
				_ => {}
			}
			continue;
		}

		match ch {
			'"' => in_string = true,
			'#' => return &line[..idx],
			_ => {}
		}
	}

	line
}

fn parse_toml_string(value: &str, line_no: usize) -> Result<String, String> {
	let mut chars = value.char_indices();
	let Some((_, first)) = chars.next() else {
		return Err(format!("line {line_no}: theme must be a quoted string"));
	};
	if first != '"' {
		return Err(format!("line {line_no}: theme must be a quoted string"));
	}

	let mut result = String::new();
	let mut escaped = false;
	for (idx, ch) in chars {
		if escaped {
			match ch {
				'"' => result.push('"'),
				'\\' => result.push('\\'),
				'n' => result.push('\n'),
				'r' => result.push('\r'),
				't' => result.push('\t'),
				_ => return Err(format!("line {line_no}: unsupported escape \\{ch}")),
			}
			escaped = false;
			continue;
		}

		match ch {
			'\\' => escaped = true,
			'"' => {
				let remainder = value[idx + ch.len_utf8()..].trim();
				if !remainder.is_empty() {
					return Err(format!("line {line_no}: unexpected trailing content"));
				}
				return Ok(result);
			}
			_ => result.push(ch),
		}
	}

	Err(format!("line {line_no}: unterminated string"))
}

fn escape_toml_string(value: &str) -> String {
	let mut escaped = String::with_capacity(value.len());
	for ch in value.chars() {
		match ch {
			'"' => escaped.push_str("\\\""),
			'\\' => escaped.push_str("\\\\"),
			'\n' => escaped.push_str("\\n"),
			'\r' => escaped.push_str("\\r"),
			'\t' => escaped.push_str("\\t"),
			_ => escaped.push(ch),
		}
	}
	escaped
}

#[cfg(test)]
mod tests {
	use super::{
		DEFAULT_THEME_NAME, default_settings_path_from_env, load_settings_from_path,
		parse_settings,
		persist_theme_name,
	};
	use std::ffi::OsString;
	use std::path::PathBuf;

	fn temp_settings_path(name: &str) -> PathBuf {
		std::env::temp_dir().join(format!(
			"baryon-settings-{name}-{}-{}.toml",
			std::process::id(),
			std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.expect("clock")
				.as_nanos()
		))
	}

	#[test]
	fn xdg_settings_path_takes_precedence_over_home() {
		let path = default_settings_path_from_env(
			Some(OsString::from("/tmp/xdg-config")),
			Some(OsString::from("/home/test")),
		)
		.expect("settings path");
		assert_eq!(path, PathBuf::from("/tmp/xdg-config/baryon/settings.toml"));
	}

	#[test]
	fn home_settings_path_is_used_when_xdg_is_missing() {
		let path = default_settings_path_from_env(None, Some(OsString::from("/home/test")))
			.expect("settings path");
		assert_eq!(path, PathBuf::from("/home/test/.config/baryon/settings.toml"));
	}

	#[test]
	fn parse_settings_reads_ui_theme_with_comments() {
		let settings = parse_settings(
			r#"
				# comment
				[ui]
				theme = "gruvbox" # trailing
			"#,
		)
		.expect("parse settings");
		assert_eq!(settings.as_deref(), Some("gruvbox"));
	}

	#[test]
	fn parse_settings_rejects_invalid_theme_value() {
		let err = parse_settings("[ui]\ntheme = onedark\n").expect_err("invalid parse");
		assert!(err.contains("quoted string"));
	}

	#[test]
	fn persist_theme_name_writes_canonical_toml() {
		let path = temp_settings_path("persist");
		persist_theme_name(Some(&path), "tokyonight").expect("persist settings");
		let written = std::fs::read_to_string(&path).expect("read settings");
		assert_eq!(written, "[ui]\ntheme = \"tokyonight\"\n");
		let _ = std::fs::remove_file(path);
	}

	#[test]
	fn load_settings_falls_back_on_invalid_theme_name() {
		let path = temp_settings_path("invalid");
		persist_theme_name(Some(&path), "onedark").expect("write canonical settings");
		std::fs::write(&path, "[ui]\ntheme = \"not-a-theme\"\n").expect("write invalid settings");

		let loaded = load_settings_from_path(Some(path.clone()));
		assert_eq!(loaded.theme_name, DEFAULT_THEME_NAME);
		assert!(loaded.startup_status.is_some());

		let _ = std::fs::remove_file(path);
	}
}
