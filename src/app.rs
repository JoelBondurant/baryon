use crate::ecs::registry::UastRegistry;
use crate::engine::{EditorCommand, Engine};
use crate::svp::resolver::SvpResolver;
use crate::uast::projection::Viewport;
use crate::ui::{Frontend, settings::LoadedSettings};
use crossterm::{
	cursor::SetCursorStyle,
	event::{DisableMouseCapture, EnableMouseCapture},
	execute,
	terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::backend::CrosstermBackend;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;

pub struct App {
	initial_theme_name: String,
	settings_path: Option<PathBuf>,
	startup_status: Option<String>,
}

impl App {
	pub(crate) fn new(settings: LoadedSettings) -> Self {
		Self {
			initial_theme_name: settings.theme_name,
			settings_path: settings.settings_path,
			startup_status: settings.startup_status,
		}
	}

	pub fn run(&self, initial_file: Option<String>) -> Result<(), io::Error> {
		// 1. Setup Channels & Registry
		let (tx_cmd, rx_cmd) = mpsc::channel::<EditorCommand>();
		let (tx_view, rx_view) = mpsc::channel::<Viewport>();
		let (tx_io_notify, rx_io_notify) = mpsc::channel::<()>();

		let registry = Arc::new(UastRegistry::new(1_000_000));
		let resolver = Arc::new(SvpResolver::new(registry.clone(), tx_io_notify));

		// Bridge IO notifications to the Engine command loop
		let tx_cmd_bridge = tx_cmd.clone();
		thread::spawn(move || {
			while let Ok(_) = rx_io_notify.recv() {
				let _ = tx_cmd_bridge.send(EditorCommand::InternalRefresh);
			}
		});

		// 2. Spawn Engine Thread
		let engine = Engine::new(
			registry.clone(),
			resolver.clone(),
			rx_cmd,
			tx_cmd.clone(),
			tx_view,
			self.initial_theme_name.clone(),
			self.settings_path.clone(),
			self.startup_status.clone(),
		);
		thread::spawn(move || {
			engine.run();
		});

		// --- INITIAL LOAD ---
		if let Some(path) = initial_file {
			let expanded = crate::core::path::expand_path(&path);
			let _ = tx_cmd.send(EditorCommand::LoadFile(
				expanded.to_string_lossy().to_string(),
			));
		}

		// 3. Setup Frontend
		enable_raw_mode()?;
		let mut stdout = io::stdout();
		execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
		let backend = CrosstermBackend::new(stdout);
		let terminal = ratatui::Terminal::new(backend)?;

		let mut frontend = Frontend::new(terminal, tx_cmd, rx_view);

		let result = frontend.run();

		// 4. Teardown
		disable_raw_mode()?;
		let mut term = frontend.release_terminal();
		execute!(
			term.backend_mut(),
			SetCursorStyle::DefaultUserShape,
			DisableMouseCapture,
			LeaveAlternateScreen
		)?;
		term.show_cursor()?;

		result
	}
}
