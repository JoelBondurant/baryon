use crate::ecs::registry::UastRegistry;
use crate::engine::{EditorCommand, Engine};
use crate::svp::resolver::SvpResolver;
use crate::uast::projection::Viewport;
use crate::ui::Frontend;
use crossterm::{
	execute,
	terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::backend::CrosstermBackend;
use std::io;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;

pub struct App;

impl App {
	pub fn new() -> Self {
		Self
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
		let engine = Engine::new(registry.clone(), resolver.clone(), rx_cmd, tx_view);
		thread::spawn(move || {
			engine.run();
		});

		// --- INITIAL LOAD ---
		if let Some(path) = initial_file {
			let expanded = crate::core::path::expand_path(&path);
			let _ = tx_cmd.send(EditorCommand::LoadFile(expanded.to_string_lossy().to_string()));
		}

		// 3. Setup Frontend
		enable_raw_mode()?;
		let mut stdout = io::stdout();
		execute!(stdout, EnterAlternateScreen)?;
		let backend = CrosstermBackend::new(stdout);
		let terminal = ratatui::Terminal::new(backend)?;

		let mut frontend = Frontend::new(terminal, tx_cmd, rx_view);

		let result = frontend.run();

		// 4. Teardown
		disable_raw_mode()?;
		let mut term = frontend.release_terminal();
		execute!(term.backend_mut(), LeaveAlternateScreen)?;
		term.show_cursor()?;

		result
	}
}
