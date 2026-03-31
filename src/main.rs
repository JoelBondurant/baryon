mod app;
mod core;
mod ecs;
mod engine;
mod svp;
mod uast;
mod ui;

fn main() -> std::io::Result<()> {
	let args: Vec<String> = std::env::args().collect();
	let initial_file = args.get(1).cloned();
	
	let app = app::App::new();
	app.run(initial_file)
}
