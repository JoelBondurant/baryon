use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

use triomphe::Arc;

use ra_ap_base_db::{CrateGraphBuilder, CrateOrigin, CrateWorkspaceData, Env, SourceRoot};
use ra_ap_cfg::CfgOptions;
use ra_ap_ide::{AnalysisHost, Highlight, HighlightConfig, HlMod, HlTag, SymbolKind};
use ra_ap_ide_db::{ChangeWithProcMacros, MiniCore};
use ra_ap_paths::{AbsPath, AbsPathBuf, Utf8PathBuf};
use ra_ap_project_model::{CargoConfig, ProjectManifest, ProjectWorkspace, RustLibSource};
use ra_ap_syntax::Edition;
use ra_ap_vfs::{FileId, Vfs, VfsPath, file_set::FileSet};
use rustc_hash::FxHashMap;

use crate::core::{DocByte, RequestId, StateId};
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
}

pub struct SemanticReactor {
	tx_in: mpsc::Sender<SemanticRequest>,
	rx_out: mpsc::Receiver<SemanticResponse>,
	_handle: thread::JoinHandle<()>,
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

		while let Ok(first) = rx.recv() {
			// Drain: collapse all pending messages, keep only the freshest.
			let mut request = first;
			while let Ok(newer) = rx.try_recv() {
				request = newer;
			}

			// (Re-)initialize when file changes or on first message.
			if host.is_none() || request.file_path != current_file_path {
				let (h, fid) = init_workspace(&request.file_path)
					.unwrap_or_else(|| init_single_file(&request.content));
				host = Some(h);
				active_file_id = fid;
				current_file_path = request.file_path.clone();
			}

			let h = host.as_mut().unwrap();

			// Apply the latest editor text.
			let mut change = ChangeWithProcMacros::default();
			change.change_file(active_file_id, Some(request.content.clone()));
			h.apply_change(change);

			let analysis = h.analysis();

			let config = HighlightConfig {
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
			};

			match analysis.highlight(config, active_file_id) {
				Ok(highlights) => {
					let mapped = highlights
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
						highlights: mapped,
					});
					let _ = tx_cmd.send(crate::engine::EditorCommand::InternalRefresh);
				}
				Err(_cancelled) => {
					// Salsa cancelled — will pick up next payload.
				}
			}
		}
	}
}

/// Load a full Cargo workspace so rust-analyzer can resolve std, deps, etc.
/// Blocks for 1-3s on first call (runs `cargo metadata`).
fn init_workspace(file_path: &str) -> Option<(AnalysisHost, FileId)> {
	let canonical = PathBuf::from(file_path).canonicalize().ok()?;
	let utf8 = Utf8PathBuf::try_from(canonical).ok()?;
	let abs_file = AbsPathBuf::assert(utf8);

	// Discover the nearest Cargo.toml.
	let manifest = ProjectManifest::discover_single(abs_file.parent()?).ok()?;

	// Load workspace with sysroot for std/core resolution.
	let cargo_config = CargoConfig {
		sysroot: Some(RustLibSource::Discover),
		..CargoConfig::default()
	};
	let ws = ProjectWorkspace::load(manifest, &cargo_config, &|_| {}).ok()?;

	// Build the crate graph. The file loader reads each source file into the VFS.
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

	// Determine the project root directory for source root partitioning.
	let project_root = abs_file.parent()?.to_path_buf();

	// Build source roots: local (project files) vs library (sysroot/deps).
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

	let mut roots = Vec::new();
	roots.push(SourceRoot::new_local(local_file_set));
	roots.push(SourceRoot::new_library(lib_file_set));

	// Drain VFS changes into the initial ChangeWithProcMacros.
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

	change.set_roots(roots);
	change.set_crate_graph(crate_graph);

	let mut host = AnalysisHost::new(None);
	host.apply_change(change);

	// Resolve the active file's FileId.
	let vfs_path = VfsPath::from(abs_file);
	let (file_id, _) = vfs.file_id(&vfs_path)?;

	Some((host, file_id))
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
		HlTag::Symbol(SymbolKind::Const | SymbolKind::Static | SymbolKind::ConstParam) => {
			TokenCategory::Constant
		}
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
