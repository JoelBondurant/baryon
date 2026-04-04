use super::Theme;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_THEME_NAME: &str = "onedark";
const DEFAULT_MINIMAP_ENABLED: bool = true;
const DEFAULT_WRAP_ENABLED: bool = true;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ParsedSettings {
	theme_name: Option<String>,
	minimap_enabled: Option<bool>,
	wrap_enabled: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoadedSettings {
	pub(crate) theme_name: String,
	pub(crate) minimap_enabled: bool,
	pub(crate) wrap_enabled: bool,
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
			minimap_enabled: DEFAULT_MINIMAP_ENABLED,
			wrap_enabled: DEFAULT_WRAP_ENABLED,
			settings_path: None,
			startup_status: None,
		};
	};

	match fs::read_to_string(&path) {
		Ok(contents) => match parse_settings(&contents) {
			Ok(parsed) => {
				let minimap_enabled = parsed.minimap_enabled.unwrap_or(DEFAULT_MINIMAP_ENABLED);
				let wrap_enabled = parsed.wrap_enabled.unwrap_or(DEFAULT_WRAP_ENABLED);
				match parsed.theme_name {
					Some(theme_name) => {
						if Theme::try_new(&theme_name).is_ok() {
							LoadedSettings {
								theme_name,
								minimap_enabled,
								wrap_enabled,
								settings_path: Some(path),
								startup_status: None,
							}
						} else {
							LoadedSettings {
								theme_name: DEFAULT_THEME_NAME.to_string(),
								minimap_enabled,
								wrap_enabled,
								settings_path: Some(path),
								startup_status: Some(format!(
									"Invalid theme in settings.toml; using {}",
									DEFAULT_THEME_NAME
								)),
							}
						}
					}
					None => LoadedSettings {
						theme_name: DEFAULT_THEME_NAME.to_string(),
						minimap_enabled,
						wrap_enabled,
						settings_path: Some(path),
						startup_status: None,
					},
				}
			}
			Err(err) => LoadedSettings {
				theme_name: DEFAULT_THEME_NAME.to_string(),
				minimap_enabled: DEFAULT_MINIMAP_ENABLED,
				wrap_enabled: DEFAULT_WRAP_ENABLED,
				settings_path: Some(path),
				startup_status: Some(format!("Settings error: {err}")),
			},
		},
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => LoadedSettings {
			theme_name: DEFAULT_THEME_NAME.to_string(),
			minimap_enabled: DEFAULT_MINIMAP_ENABLED,
			wrap_enabled: DEFAULT_WRAP_ENABLED,
			settings_path: Some(path),
			startup_status: None,
		},
		Err(err) => LoadedSettings {
			theme_name: DEFAULT_THEME_NAME.to_string(),
			minimap_enabled: DEFAULT_MINIMAP_ENABLED,
			wrap_enabled: DEFAULT_WRAP_ENABLED,
			settings_path: Some(path),
			startup_status: Some(format!("Settings error: {err}")),
		},
	}
}

pub(crate) fn persist_ui_settings(
	settings_path: Option<&Path>,
	theme_name: &str,
	minimap_enabled: bool,
	wrap_enabled: bool,
) -> Result<(), String> {
	let path = settings_path.ok_or_else(|| "No config directory available".to_string())?;
	if let Some(parent) = path.parent() {
		fs::create_dir_all(parent).map_err(|e| format!("Failed to create config dir: {e}"))?;
	}
	fs::write(
		path,
		render_settings(theme_name, minimap_enabled, wrap_enabled),
	)
	.map_err(|e| format!("Failed to write settings.toml: {e}"))
}

fn render_settings(theme_name: &str, minimap_enabled: bool, wrap_enabled: bool) -> String {
	format!(
		"[ui]\ntheme = \"{}\"\nminimap = {}\nwrap = {}\n",
		escape_toml_string(theme_name),
		if minimap_enabled { "true" } else { "false" },
		if wrap_enabled { "true" } else { "false" }
	)
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

fn parse_settings(contents: &str) -> Result<ParsedSettings, String> {
	let mut in_ui_section = false;
	let mut parsed = ParsedSettings::default();

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
		match key.trim() {
			"theme" => {
				parsed.theme_name = Some(parse_toml_string(value.trim(), line_no)?);
			}
			"minimap" => {
				parsed.minimap_enabled = Some(parse_toml_bool(value.trim(), line_no)?);
			}
			"wrap" => {
				parsed.wrap_enabled = Some(parse_toml_bool(value.trim(), line_no)?);
			}
			_ => {}
		}
	}

	Ok(parsed)
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

fn parse_toml_bool(value: &str, line_no: usize) -> Result<bool, String> {
	match value {
		"true" => Ok(true),
		"false" => Ok(false),
		_ => Err(format!("line {line_no}: minimap must be true or false")),
	}
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
		DEFAULT_MINIMAP_ENABLED, DEFAULT_THEME_NAME, DEFAULT_WRAP_ENABLED,
		default_settings_path_from_env, load_settings_from_path, parse_settings,
		persist_ui_settings,
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
		assert_eq!(
			path,
			PathBuf::from("/home/test/.config/baryon/settings.toml")
		);
	}

	#[test]
	fn parse_settings_reads_ui_theme_with_comments() {
		let settings = parse_settings(
			r#"
				# comment
				[ui]
				theme = "gruvbox" # trailing
				minimap = false
				wrap = false
			"#,
		)
		.expect("parse settings");
		assert_eq!(settings.theme_name.as_deref(), Some("gruvbox"));
		assert_eq!(settings.minimap_enabled, Some(false));
		assert_eq!(settings.wrap_enabled, Some(false));
	}

	#[test]
	fn parse_settings_rejects_invalid_theme_value() {
		let err = parse_settings("[ui]\ntheme = onedark\n").expect_err("invalid parse");
		assert!(err.contains("quoted string"));
	}

	#[test]
	fn parse_settings_rejects_invalid_minimap_value() {
		let err = parse_settings("[ui]\nminimap = maybe\n").expect_err("invalid parse");
		assert!(err.contains("true or false"));
	}

	#[test]
	fn persist_ui_settings_writes_canonical_toml() {
		let path = temp_settings_path("persist");
		persist_ui_settings(Some(&path), "tokyonight", false, false).expect("persist settings");
		let written = std::fs::read_to_string(&path).expect("read settings");
		assert_eq!(
			written,
			"[ui]\ntheme = \"tokyonight\"\nminimap = false\nwrap = false\n"
		);
		let _ = std::fs::remove_file(path);
	}

	#[test]
	fn load_settings_falls_back_on_invalid_theme_name_and_keeps_preferences() {
		let path = temp_settings_path("invalid");
		persist_ui_settings(Some(&path), "onedark", false, false)
			.expect("write canonical settings");
		std::fs::write(
			&path,
			"[ui]\ntheme = \"not-a-theme\"\nminimap = false\nwrap = false\n",
		)
		.expect("write invalid settings");

		let loaded = load_settings_from_path(Some(path.clone()));
		assert_eq!(loaded.theme_name, DEFAULT_THEME_NAME);
		assert!(!loaded.minimap_enabled);
		assert!(!loaded.wrap_enabled);
		assert!(loaded.startup_status.is_some());

		let _ = std::fs::remove_file(path);
	}

	#[test]
	fn load_settings_defaults_bools_when_not_present() {
		let path = temp_settings_path("default-bools");
		std::fs::write(&path, "[ui]\ntheme = \"onedark\"\n").expect("write settings");

		let loaded = load_settings_from_path(Some(path.clone()));
		assert_eq!(loaded.theme_name, DEFAULT_THEME_NAME);
		assert_eq!(loaded.minimap_enabled, DEFAULT_MINIMAP_ENABLED);
		assert_eq!(loaded.wrap_enabled, DEFAULT_WRAP_ENABLED);

		let _ = std::fs::remove_file(path);
	}
}
