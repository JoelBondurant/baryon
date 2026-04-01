use std::sync::mpsc;
use std::thread;

use triomphe::Arc;

use ra_ap_base_db::{CrateGraphBuilder, CrateOrigin, CrateWorkspaceData, Env, SourceRoot};
use ra_ap_cfg::CfgOptions;
use ra_ap_ide::{AnalysisHost, HighlightConfig, HlTag, SymbolKind};
use ra_ap_ide_db::{ChangeWithProcMacros, MiniCore};
use ra_ap_paths::AbsPathBuf;
use ra_ap_syntax::Edition;
use ra_ap_vfs::{FileId, VfsPath, file_set::FileSet};

use crate::svp::highlight::TokenCategory;

pub struct SemanticReactor {
	tx_in: mpsc::Sender<(String, u64)>,
	rx_out: mpsc::Receiver<Vec<(u64, u64, TokenCategory)>>,
	_handle: thread::JoinHandle<()>,
}

impl SemanticReactor {
	pub fn new() -> Self {
		let (tx_in, rx_worker) = mpsc::channel::<(String, u64)>();
		let (tx_worker, rx_out) = mpsc::channel::<Vec<(u64, u64, TokenCategory)>>();

		let handle = thread::Builder::new()
			.name("semantic-reactor".into())
			.spawn(move || {
				Self::run_event_loop(rx_worker, tx_worker);
			})
			.expect("failed to spawn semantic reactor thread");

		Self {
			tx_in,
			rx_out,
			_handle: handle,
		}
	}

	/// Push new file content to the reactor. Non-blocking.
	pub fn send(&self, content: String, global_offset: u64) {
		let _ = self.tx_in.send((content, global_offset));
	}

	/// Try to receive semantic highlights. Non-blocking.
	pub fn try_recv(&self) -> Option<Vec<(u64, u64, TokenCategory)>> {
		self.rx_out.try_recv().ok()
	}

	fn run_event_loop(
		rx: mpsc::Receiver<(String, u64)>,
		tx: mpsc::Sender<Vec<(u64, u64, TokenCategory)>>,
	) {
		let file_id = FileId::from_raw(0);
		let mut host = Self::init_host(file_id, "");

		while let Ok((text, global_offset)) = rx.recv() {
			// Apply the new file content.
			let mut change = ChangeWithProcMacros::default();
			change.change_file(file_id, Some(text));
			host.apply_change(change);

			let analysis = host.analysis();

			let config = HighlightConfig {
				strings: true,
				comments: true,
				punctuation: true,
				specialize_punctuation: false,
				operator: true,
				specialize_operator: false,
				inject_doc_comment: false,
				macro_bang: true,
				syntactic_name_ref_highlighting: false,
				minicore: MiniCore::default(),
			};

			match analysis.highlight(config, file_id) {
				Ok(highlights) => {
					let mapped = highlights
						.into_iter()
						.filter_map(|hl| {
							let cat = map_hl_tag(hl.highlight.tag);
							if cat == TokenCategory::Unclassified {
								return None;
							}
							let start =
								global_offset + u64::from(u32::from(hl.range.start()));
							let end =
								global_offset + u64::from(u32::from(hl.range.end()));
							Some((start, end, cat))
						})
						.collect();
					let _ = tx.send(mapped);
				}
				Err(_cancelled) => {
					// Salsa cancelled the query because the database was mutated
					// mid-analysis. This is expected; we'll pick up the next payload.
				}
			}
		}
	}

	fn init_host(file_id: FileId, initial_text: &str) -> AnalysisHost {
		let mut host = AnalysisHost::new(None);

		// Build a FileSet containing our single file.
		let mut file_set = FileSet::default();
		file_set.insert(
			file_id,
			VfsPath::new_virtual_path("/baryon/active.rs".into()),
		);
		let source_root = SourceRoot::new_local(file_set);

		// Build a minimal CrateGraph with one crate.
		let mut crate_graph = CrateGraphBuilder::default();

		let proc_macro_cwd = Arc::new(AbsPathBuf::assert("/tmp".into()));
		let ws_data = Arc::new(CrateWorkspaceData {
			target: Err("not loaded".into()),
			toolchain: None,
		});

		crate_graph.add_crate_root(
			file_id,
			Edition::Edition2024,
			None,                // display_name
			None,                // version
			CfgOptions::default(),
			None,                // potential_cfg_options
			Env::default(),
			CrateOrigin::Local {
				repo: None,
				name: None,
			},
			Vec::new(),          // crate_attrs
			false,               // is_proc_macro
			proc_macro_cwd,
			ws_data,
		);

		// Assemble the initial change.
		let mut change = ChangeWithProcMacros::default();
		change.set_roots(vec![source_root]);
		change.set_crate_graph(crate_graph);
		change.change_file(file_id, Some(initial_text.to_owned()));
		host.apply_change(change);

		host
	}
}

fn map_hl_tag(tag: HlTag) -> TokenCategory {
	match tag {
		HlTag::Symbol(SymbolKind::Function | SymbolKind::Method) => TokenCategory::Function,
		HlTag::Symbol(
			SymbolKind::Struct
			| SymbolKind::Enum
			| SymbolKind::Union
			| SymbolKind::TypeAlias
			| SymbolKind::TypeParam
			| SymbolKind::Trait
			| SymbolKind::SelfType,
		) => TokenCategory::Type,
		HlTag::Symbol(
			SymbolKind::Local | SymbolKind::SelfParam | SymbolKind::ValueParam | SymbolKind::Field,
		) => TokenCategory::Variable,
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
		HlTag::BuiltinType => TokenCategory::Type,
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
