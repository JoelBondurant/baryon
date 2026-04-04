use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

use triomphe::Arc;

use ra_ap_base_db::{CrateGraphBuilder, CrateOrigin, CrateWorkspaceData, Env, SourceRoot};
use ra_ap_cfg::CfgOptions;
use ra_ap_ide::{
	Analysis, AnalysisHost, AssistResolveStrategy, Cancellable, DiagnosticsConfig, Highlight,
	HighlightConfig, HlMod, HlTag, SymbolKind,
};
use ra_ap_ide_db::{ChangeWithProcMacros, MiniCore, Severity};
use ra_ap_paths::{AbsPath, AbsPathBuf, Utf8PathBuf};
use ra_ap_project_model::{CargoConfig, ProjectManifest, ProjectWorkspace, RustLibSource};
use ra_ap_syntax::Edition;
use ra_ap_vfs::{FileId, Vfs, VfsPath, file_set::FileSet};
use rustc_hash::FxHashMap;

use crate::core::{DocByte, RequestId, StateId};
use crate::svp::diagnostic::{DiagnosticSeverity, DiagnosticSpan};
use crate::svp::highlight::{HighlightSpan, TokenCategory};

#[derive(Debug)]
pub struct SemanticRequest {
	pub content: String,
	pub global_offset: DocByte,
	pub file_path: String,
	pub state_id: StateId,
	pub request_id: RequestId,
}

#[derive(Debug)]
pub struct SemanticResponse {
	pub state_id: StateId,
	pub request_id: RequestId,
	pub highlights: Vec<HighlightSpan>,
	pub diagnostics: Vec<DiagnosticSpan>,
}

pub struct SemanticReactor {
	tx_in: mpsc::Sender<SemanticRequest>,
	rx_out: mpsc::Receiver<SemanticResponse>,
	_handle: thread::JoinHandle<()>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DiagnosticCollectionMode {
	Full,
	SyntaxOnly,
}

impl SemanticReactor {
	pub fn new(tx_cmd: mpsc::Sender<crate::engine::EditorCommand>) -> Self {
		let (tx_in, rx_worker) = mpsc::channel::<SemanticRequest>();
		let (tx_worker, rx_out) = mpsc::channel::<SemanticResponse>();

		let handle = thread::Builder::new()
			.name("semantic-reactor".into())
			.spawn(move || {
				Self::run_event_loop(rx_worker, tx_worker, tx_cmd);
			})
			.expect("failed to spawn semantic reactor thread");

		Self {
			tx_in,
			rx_out,
			_handle: handle,
		}
	}

	/// Push new file content to the reactor. Non-blocking.
	pub fn send(&self, request: SemanticRequest) {
		let _ = self.tx_in.send(request);
	}

	/// Try to receive semantic highlights. Non-blocking.
	pub fn try_recv(&self) -> Option<SemanticResponse> {
		self.rx_out.try_recv().ok()
	}

	fn run_event_loop(
		rx: mpsc::Receiver<SemanticRequest>,
		tx: mpsc::Sender<SemanticResponse>,
		tx_cmd: mpsc::Sender<crate::engine::EditorCommand>,
	) {
		let mut host: Option<AnalysisHost> = None;
		let mut active_file_id = FileId::from_raw(0);
		let mut current_file_path = String::new();
		let diagnostics_config = diagnostics_config();
		let mut diagnostic_mode = DiagnosticCollectionMode::SyntaxOnly;

		while let Ok(first) = rx.recv() {
			// Drain: collapse all pending messages, keep only the freshest.
			let mut request = first;
			while let Ok(newer) = rx.try_recv() {
				request = newer;
			}

			// (Re-)initialize when file changes or on first message.
			if host.is_none() || request.file_path != current_file_path {
				let (h, fid, mode) = match init_workspace(&request.file_path) {
					Some((host, file_id)) => (host, file_id, DiagnosticCollectionMode::Full),
					None => {
						let (host, file_id) = init_single_file(&request.content);
						(host, file_id, DiagnosticCollectionMode::SyntaxOnly)
					}
				};
				host = Some(h);
				active_file_id = fid;
				diagnostic_mode = mode;
				current_file_path = request.file_path.clone();
			}

			let h = host.as_mut().unwrap();

			// Apply the latest editor text.
			let mut change = ChangeWithProcMacros::default();
			change.change_file(active_file_id, Some(request.content.clone()));
			h.apply_change(change);

			let analysis = h.analysis();

			match (
				analysis.highlight(highlight_config(), active_file_id),
				collect_error_diagnostics(
					&analysis,
					active_file_id,
					&request.content,
					request.global_offset,
					&diagnostics_config,
					diagnostic_mode,
				),
			) {
				(Ok(highlights), Ok(diagnostics)) => {
					let mapped_highlights = highlights
						.into_iter()
						.filter_map(|hl| {
							let cat = map_hl_tag(hl.highlight);
							if cat == TokenCategory::Unclassified {
								return None;
							}
							let start = request
								.global_offset
								.saturating_add(u64::from(u32::from(hl.range.start())));
							let end = request
								.global_offset
								.saturating_add(u64::from(u32::from(hl.range.end())));
							Some(HighlightSpan::new(start, end, cat))
						})
						.collect();
					let _ = tx.send(SemanticResponse {
						state_id: request.state_id,
						request_id: request.request_id,
						highlights: mapped_highlights,
						diagnostics,
					});
					let _ = tx_cmd.send(crate::engine::EditorCommand::InternalRefresh);
				}
				(Err(_cancelled), _) | (_, Err(_cancelled)) => {
					// Salsa cancelled — will pick up next payload.
				}
			}
		}
	}
}

fn diagnostics_config() -> DiagnosticsConfig {
	let mut config = DiagnosticsConfig::test_sample();
	config.proc_macros_enabled = true;
	config.proc_attr_macros_enabled = true;
	config
}

fn highlight_config() -> HighlightConfig<'static> {
	HighlightConfig {
		strings: true,
		comments: true,
		punctuation: true,
		specialize_punctuation: false,
		operator: true,
		specialize_operator: false,
		inject_doc_comment: false,
		macro_bang: true,
		syntactic_name_ref_highlighting: true,
		minicore: MiniCore::default(),
	}
}

fn collect_error_diagnostics(
	analysis: &Analysis,
	active_file_id: FileId,
	content: &str,
	global_offset: DocByte,
	config: &DiagnosticsConfig,
	mode: DiagnosticCollectionMode,
) -> Cancellable<Vec<DiagnosticSpan>> {
	let diagnostics = match mode {
		DiagnosticCollectionMode::Full => {
			analysis.full_diagnostics(config, AssistResolveStrategy::None, active_file_id)?
		}
		DiagnosticCollectionMode::SyntaxOnly => {
			analysis.syntax_diagnostics(config, active_file_id)?
		}
	};
	Ok(diagnostics
		.into_iter()
		.filter_map(|diagnostic| {
			if diagnostic.range.file_id != active_file_id || diagnostic.severity != Severity::Error
			{
				return None;
			}

			let start = u32::from(diagnostic.range.range.start()) as usize;
			let end = u32::from(diagnostic.range.range.end()) as usize;
			let (start, end) = normalize_diagnostic_range(content, start, end)?;
			Some(DiagnosticSpan::new(
				global_offset.saturating_add(start as u64),
				global_offset.saturating_add(end as u64),
				DiagnosticSeverity::Error,
			))
		})
		.collect())
}

fn normalize_diagnostic_range(content: &str, start: usize, end: usize) -> Option<(usize, usize)> {
	let len = content.len();
	let start = start.min(len);
	let end = end.min(len);
	if end > start {
		return Some((start, end));
	}

	if start < len {
		let next = start + content[start..].chars().next()?.len_utf8();
		return Some((start, next));
	}

	if start > 0 {
		let prev = content[..start].char_indices().next_back()?.0;
		return Some((prev, start));
	}

	None
}

/// Load a full Cargo workspace so rust-analyzer can resolve std, deps, etc.
/// Blocks for 1-3s on first call (runs `cargo metadata`).
fn init_workspace(file_path: &str) -> Option<(AnalysisHost, FileId)> {
	try_init_workspace(file_path).ok()
}

fn try_init_workspace(file_path: &str) -> Result<(AnalysisHost, FileId), String> {
	let canonical = PathBuf::from(file_path)
		.canonicalize()
		.map_err(|e| format!("canonicalize failed: {e}"))?;
	let utf8 = Utf8PathBuf::try_from(canonical).map_err(|_| "non-utf8 path".to_string())?;
	let abs_file = AbsPathBuf::assert(utf8);
	let manifest = if let Ok(manifest) = ProjectManifest::discover_single(abs_file.as_ref()) {
		manifest
	} else if let Some(parent) = abs_file.parent() {
		ProjectManifest::discover_single(parent)
			.map_err(|e| format!("manifest discovery failed: {e}"))?
	} else {
		return Err("manifest discovery failed: path has no parent".to_string());
	};

	let load_workspace = |cargo_config: &CargoConfig| {
		ProjectWorkspace::load(manifest.clone(), cargo_config, &|_| {}).ok()
	};
	let discover_sysroot = CargoConfig {
		sysroot: Some(RustLibSource::Discover),
		..CargoConfig::default()
	};
	let ws = load_workspace(&discover_sysroot)
		.or_else(|| load_workspace(&CargoConfig::default()))
		.ok_or_else(|| "workspace load failed".to_string())?;

	let mut vfs = Vfs::default();
	let extra_env: FxHashMap<String, Option<String>> = FxHashMap::default();
	let (crate_graph, _proc_macros) = ws.to_crate_graph(
		&mut |path: &AbsPath| {
			let contents = std::fs::read(path).ok();
			let vfs_path = VfsPath::from(path.to_path_buf());
			vfs.set_file_contents(vfs_path.clone(), contents);
			vfs.file_id(&vfs_path)
				.and_then(|(fid, exc)| (exc == ra_ap_vfs::FileExcluded::No).then_some(fid))
		},
		&extra_env,
	);

	let active_vfs_path = VfsPath::from(abs_file.clone());
	if vfs.file_id(&active_vfs_path).is_none() {
		vfs.set_file_contents(active_vfs_path.clone(), std::fs::read(&abs_file).ok());
	}

	let project_root = manifest.manifest_path().parent().to_path_buf();
	let mut local_file_set = FileSet::default();
	let mut lib_file_set = FileSet::default();

	for (fid, vfs_path) in vfs.iter() {
		let is_local = vfs_path
			.as_path()
			.map(|p| p.starts_with(&project_root))
			.unwrap_or(false);
		if is_local {
			local_file_set.insert(fid, vfs_path.clone());
		} else {
			lib_file_set.insert(fid, vfs_path.clone());
		}
	}

	let mut change = ChangeWithProcMacros::default();
	for (_, changed) in vfs.take_changes() {
		match changed.change {
			ra_ap_vfs::Change::Create(contents, _) | ra_ap_vfs::Change::Modify(contents, _) => {
				if let Ok(text) = String::from_utf8(contents) {
					change.change_file(changed.file_id, Some(text));
				}
			}
			ra_ap_vfs::Change::Delete => {}
		}
	}

	change.set_roots(vec![
		SourceRoot::new_local(local_file_set),
		SourceRoot::new_library(lib_file_set),
	]);
	change.set_crate_graph(crate_graph);

	let mut host = AnalysisHost::new(None);
	host.apply_change(change);

	let (file_id, _) = vfs
		.file_id(&active_vfs_path)
		.ok_or_else(|| "active file missing from workspace vfs".to_string())?;

	Ok((host, file_id))
}

/// Fallback: single-file universe with no dependency resolution.
fn init_single_file(initial_text: &str) -> (AnalysisHost, FileId) {
	let file_id = FileId::from_raw(0);
	let mut host = AnalysisHost::new(None);

	let mut file_set = FileSet::default();
	file_set.insert(
		file_id,
		VfsPath::new_virtual_path("/baryon/active.rs".into()),
	);
	let source_root = SourceRoot::new_local(file_set);

	let mut crate_graph = CrateGraphBuilder::default();
	let proc_macro_cwd = Arc::new(AbsPathBuf::assert("/tmp".into()));
	let ws_data = Arc::new(CrateWorkspaceData {
		target: Err("not loaded".into()),
		toolchain: None,
	});

	crate_graph.add_crate_root(
		file_id,
		Edition::Edition2024,
		None,
		None,
		CfgOptions::default(),
		None,
		Env::default(),
		CrateOrigin::Local {
			repo: None,
			name: None,
		},
		Vec::new(),
		false,
		proc_macro_cwd,
		ws_data,
	);

	let mut change = ChangeWithProcMacros::default();
	change.set_roots(vec![source_root]);
	change.set_crate_graph(crate_graph);
	change.change_file(file_id, Some(initial_text.to_owned()));
	host.apply_change(change);

	(host, file_id)
}

fn map_hl_tag(highlight: Highlight) -> TokenCategory {
	let mods = highlight.mods;
	let tag = highlight.tag;

	// Modifier-first checks: crate/library symbols and mutability.
	if mods.contains(HlMod::CrateRoot) || mods.contains(HlMod::Library) {
		if matches!(
			tag,
			HlTag::Symbol(SymbolKind::Module | SymbolKind::CrateRoot)
		) {
			return TokenCategory::Crate;
		}
	}

	if mods.contains(HlMod::Mutable)
		&& matches!(
			tag,
			HlTag::Symbol(SymbolKind::Local | SymbolKind::ValueParam | SymbolKind::Field)
		) {
		return TokenCategory::MutableVariable;
	}

	// Tag-based classification.
	match tag {
		HlTag::Symbol(SymbolKind::Method) => TokenCategory::Method,
		HlTag::Symbol(SymbolKind::Function) => TokenCategory::Function,
		HlTag::Symbol(
			SymbolKind::Struct
			| SymbolKind::Enum
			| SymbolKind::Union
			| SymbolKind::TypeAlias
			| SymbolKind::TypeParam
			| SymbolKind::Trait,
		) => TokenCategory::Type,
		HlTag::Symbol(SymbolKind::SelfParam | SymbolKind::SelfType) => TokenCategory::SelfKeyword,
		HlTag::Symbol(SymbolKind::Local | SymbolKind::ValueParam | SymbolKind::Field) => {
			TokenCategory::Variable
		}
		HlTag::Symbol(
			SymbolKind::Const | SymbolKind::Static | SymbolKind::ConstParam | SymbolKind::Variant,
		) => TokenCategory::Constant,
		HlTag::Symbol(SymbolKind::Macro | SymbolKind::ProcMacro) => TokenCategory::Macro,
		HlTag::Symbol(SymbolKind::Module | SymbolKind::CrateRoot | SymbolKind::ToolModule) => {
			TokenCategory::Module
		}
		HlTag::Symbol(SymbolKind::LifetimeParam) => TokenCategory::Lifetime,
		HlTag::Symbol(
			SymbolKind::Attribute
			| SymbolKind::BuiltinAttr
			| SymbolKind::Derive
			| SymbolKind::DeriveHelper,
		) => TokenCategory::Attribute,
		HlTag::Symbol(_) => TokenCategory::Unclassified,
		HlTag::BuiltinType => TokenCategory::BuiltinType,
		HlTag::Keyword | HlTag::BoolLiteral => TokenCategory::Keyword,
		HlTag::Comment => TokenCategory::Comment,
		HlTag::StringLiteral
		| HlTag::ByteLiteral
		| HlTag::CharLiteral
		| HlTag::EscapeSequence
		| HlTag::InvalidEscapeSequence
		| HlTag::FormatSpecifier => TokenCategory::String,
		HlTag::NumericLiteral => TokenCategory::Number,
		HlTag::Operator(_) => TokenCategory::Operator,
		HlTag::Punctuation(_) => TokenCategory::Punctuation,
		HlTag::AttributeBracket => TokenCategory::Attribute,
		HlTag::UnresolvedReference | HlTag::None => TokenCategory::Unclassified,
	}
}

#[cfg(test)]
mod tests {
	use super::{
		ChangeWithProcMacros, DiagnosticCollectionMode, TokenCategory, collect_error_diagnostics,
		diagnostics_config, highlight_config, init_single_file, map_hl_tag,
		normalize_diagnostic_range, try_init_workspace,
	};
	use crate::core::DocByte;
	use std::fs;
	use std::path::PathBuf;

	#[test]
	fn invalid_rust_produces_visible_error_diagnostic_spans() {
		let source = "fn main() {\n";
		let (host, file_id) = init_single_file(source);
		let diagnostics = collect_error_diagnostics(
			&host.analysis(),
			file_id,
			source,
			DocByte::ZERO,
			&diagnostics_config(),
			DiagnosticCollectionMode::SyntaxOnly,
		)
		.expect("diagnostics should resolve");

		assert!(!diagnostics.is_empty());
		assert!(diagnostics.iter().all(|diag| diag.start < diag.end));
	}

	#[test]
	fn valid_builtin_derives_do_not_produce_error_diagnostics() {
		let source = "#[derive(Debug, Clone, Copy, PartialEq, Eq)]\nstruct Sample;\n";
		let (host, file_id) = init_single_file(source);
		let diagnostics = collect_error_diagnostics(
			&host.analysis(),
			file_id,
			source,
			DocByte::ZERO,
			&diagnostics_config(),
			DiagnosticCollectionMode::SyntaxOnly,
		)
		.expect("diagnostics should resolve");

		assert!(
			diagnostics.is_empty(),
			"valid derive should not produce error diagnostics: {diagnostics:?}"
		);
	}

	#[test]
	fn empty_diagnostic_ranges_expand_to_a_visible_span() {
		assert_eq!(normalize_diagnostic_range("abc", 3, 3), Some((2, 3)));
		assert_eq!(normalize_diagnostic_range("abc", 1, 1), Some((1, 2)));
		assert_eq!(normalize_diagnostic_range("", 0, 0), None);
	}

	#[test]
	fn workspace_semantics_resolve_engine_helper_imports() {
		let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/engine/core.rs");
		let source = fs::read_to_string(&path).expect("core.rs should be readable");
		let path_str = path.to_string_lossy().into_owned();
		let (mut host, file_id) =
			try_init_workspace(&path_str).expect("workspace mode should load baryon");
		let mut change = ChangeWithProcMacros::default();
		change.change_file(file_id, Some(source.clone()));
		host.apply_change(change);
		let analysis = host.analysis();

		let diagnostics = collect_error_diagnostics(
			&analysis,
			file_id,
			&source,
			DocByte::ZERO,
			&diagnostics_config(),
			DiagnosticCollectionMode::Full,
		)
		.expect("workspace diagnostics should resolve");
		assert!(
			diagnostics.is_empty(),
			"workspace diagnostics should not flag engine helper imports: {diagnostics:?}"
		);

		let folding_import_start = source
			.find("use super::folding::{")
			.expect("folding import should exist");
		let folding_name_offset = folding_import_start
			+ source[folding_import_start..]
				.find("resolve_fold_boundary_at_cursor")
				.expect("folding helper import should exist");

		let layout_import_start = source
			.find("use super::layout::{")
			.expect("layout import should exist");
		let layout_name_offset = layout_import_start
			+ source[layout_import_start..]
				.find("viewport_geometry")
				.expect("layout helper import should exist");

		let highlights = analysis
			.highlight(highlight_config(), file_id)
			.expect("workspace highlights should resolve");
		let category_for = |offset: usize| {
			highlights
				.iter()
				.find(|hl| {
					let start = u32::from(hl.range.start()) as usize;
					let end = u32::from(hl.range.end()) as usize;
					start <= offset && offset < end
				})
				.map(|hl| map_hl_tag(hl.highlight))
		};

		assert!(matches!(
			category_for(folding_name_offset),
			Some(TokenCategory::Function | TokenCategory::Module)
		));
		assert!(matches!(
			category_for(layout_name_offset),
			Some(TokenCategory::Function | TokenCategory::Module)
		));
	}
}
