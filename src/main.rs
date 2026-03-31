mod app;
mod ecs;
mod engine;
mod svp;
mod uast;
mod ui;

fn main() -> std::io::Result<()> {
	let app = app::App::new();
	app.run()
}
