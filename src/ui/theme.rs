use ratatui::style::Color;
use crate::svp::highlight::TokenCategory;
use std::collections::HashMap;

pub struct Theme {
	pub syntax_colors: HashMap<TokenCategory, Color>,
}

impl Theme {
	pub fn new(name: &str) -> Self {
		let mut colors = HashMap::new();
		let categories = [
			TokenCategory::Keyword,
			TokenCategory::String,
			TokenCategory::Comment,
			TokenCategory::Type,
			TokenCategory::Function,
			TokenCategory::Number,
			TokenCategory::Punctuation,
			TokenCategory::Variable,
			TokenCategory::Constant,
			TokenCategory::Macro,
			TokenCategory::Module,
			TokenCategory::Lifetime,
			TokenCategory::Attribute,
			TokenCategory::Operator,
			TokenCategory::SelfKeyword,
			TokenCategory::BuiltinType,
			TokenCategory::MutableVariable,
			TokenCategory::Method,
			TokenCategory::Crate,
			TokenCategory::Whitespace,
		];

		let (mut map_name, reverse) = if let Some(stripped) = name.strip_suffix("_r") {
			(stripped, true)
		} else {
			(name, false)
		};

		// Normalization for the provided reference names
		if map_name.is_empty() { map_name = "viridis"; }

		for (i, &cat) in categories.iter().enumerate() {
			let mut t = i as f32 / (categories.len() as f32 - 1.0);
			if reverse {
				t = 1.0 - t;
			}
			
			// Clipping: skip the dark end of the ramp
			// We'll map [0, 1] to [0.2, 1.0] to skip the deep purples/blacks
			let t_clipped = 0.2 + (t * 0.8);

			let color = match map_name.to_lowercase().as_str() {
				"black" => black(t_clipped),
				"bluegreen" => blue_green(t_clipped),
				"bluered" => blue_red(t_clipped),
				"bluewhitered" => blue_white_red(t_clipped),
				"blues" => blues(t_clipped),
				"cividis" => cividis(t_clipped),
				"greenblue" => green_blue(t_clipped),
				"greenpurples" => green_purples(t_clipped),
				"greenred" => green_red(t_clipped),
				"greens" => greens(t_clipped),
				"grey" | "gray" => grey(t_clipped),
				"greys" | "grays" => greys(t_clipped),
				"inferno" => inferno(t_clipped),
				"magma" => magma(t_clipped),
				"oranges" => oranges(t_clipped),
				"purplegreens" => purple_greens(t_clipped),
				"purples" => purples(t_clipped),
				"rainbow" => rainbow(t_clipped),
				"redblue" => red_blue(t_clipped),
				"redgreen" => red_green(t_clipped),
				"redwhiteblue" => red_white_blue(t_clipped),
				"reds" => reds(t_clipped),
				"viridis" => viridis(t_clipped),
				"white" => white(t_clipped),
				"yellows" => yellows(t_clipped),
				"plasma" => plasma(t_clipped), // Added from previous version
				_ => viridis(t_clipped), // default to viridis
			};
			colors.insert(cat, color);
		}

		Self { syntax_colors: colors }
	}
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
	a + (b - a) * t
}

fn map_color(t: f32, colors: &[(f32, f32, f32)]) -> Color {
	let t = t.clamp(0.0, 1.0) * (colors.len() - 1) as f32;
	let i = t.floor() as usize;
	let frac = t.fract();
	if i >= colors.len() - 1 {
		let (r, g, b) = colors[colors.len() - 1];
		return Color::Rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8);
	}
	let (r1, g1, b1) = colors[i];
	let (r2, g2, b2) = colors[i + 1];
	Color::Rgb(
		(lerp(r1, r2, frac) * 255.0) as u8,
		(lerp(g1, g2, frac) * 255.0) as u8,
		(lerp(b1, b2, frac) * 255.0) as u8,
	)
}

// --- Colormap Definitions from Polariz Reference ---

pub fn rainbow(t: f32) -> Color {
	let h = t.clamp(0.0, 1.0) * 5.0;
	let x = 1.0 - (h % 2.0 - 1.0).abs();
	let (r, g, b) = match h as i32 {
		0 => (1.0, x, 0.0),
		1 => (x, 1.0, 0.0),
		2 => (0.0, 1.0, x),
		3 => (0.0, x, 1.0),
		_ => (x, 0.0, 1.0),
	};
	Color::Rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

pub fn greys(t: f32) -> Color {
	let v = (t * 255.0) as u8;
	Color::Rgb(v, v, v)
}

pub fn black(_t: f32) -> Color {
	Color::Rgb(0, 0, 0)
}

pub fn grey(_t: f32) -> Color {
	Color::Rgb(128, 128, 128)
}

pub fn white(_t: f32) -> Color {
	Color::Rgb(255, 255, 255)
}

pub fn viridis(t: f32) -> Color {
	map_color(
		t,
		&[
			(0.267, 0.004, 0.329),
			(0.231, 0.322, 0.545),
			(0.129, 0.569, 0.553),
			(0.369, 0.789, 0.384),
			(0.993, 0.906, 0.145),
		],
	)
}

pub fn inferno(t: f32) -> Color {
	map_color(
		t,
		&[
			(0.001, 0.005, 0.022),
			(0.337, 0.070, 0.463),
			(0.733, 0.203, 0.333),
			(0.976, 0.588, 0.133),
			(0.988, 0.996, 0.643),
		],
	)
}

pub fn magma(t: f32) -> Color {
	map_color(
		t,
		&[
			(0.00, 0.00, 0.02),
			(0.31, 0.07, 0.48),
			(0.71, 0.18, 0.50),
			(0.98, 0.56, 0.45),
			(0.99, 0.99, 0.75),
		],
	)
}

pub fn plasma(t: f32) -> Color {
	map_color(
		t,
		&[
			(0.050, 0.030, 0.528),
			(0.493, 0.012, 0.658),
			(0.796, 0.279, 0.471),
			(0.953, 0.675, 0.242),
			(0.940, 0.975, 0.131),
		],
	)
}

pub fn cividis(t: f32) -> Color {
	map_color(
		t,
		&[
			(0.00, 0.13, 0.30),
			(0.30, 0.40, 0.48),
			(0.48, 0.48, 0.48),
			(0.73, 0.70, 0.30),
			(0.99, 0.99, 0.25),
		],
	)
}

pub fn reds(t: f32) -> Color {
	map_color(
		t,
		&[(0.99, 0.88, 0.82), (0.98, 0.38, 0.28), (0.40, 0.00, 0.05)],
	)
}

pub fn yellows(t: f32) -> Color {
	map_color(
		t,
		&[(0.99, 0.99, 0.80), (0.99, 0.85, 0.20), (0.60, 0.40, 0.00)],
	)
}

pub fn blues(t: f32) -> Color {
	map_color(
		t,
		&[(0.93, 0.95, 0.98), (0.42, 0.68, 0.84), (0.03, 0.19, 0.42)],
	)
}

pub fn oranges(t: f32) -> Color {
	map_color(
		t,
		&[(0.99, 0.91, 0.80), (0.99, 0.55, 0.15), (0.50, 0.15, 0.00)],
	)
}

pub fn purples(t: f32) -> Color {
	map_color(
		t,
		&[(0.95, 0.94, 0.97), (0.62, 0.57, 0.76), (0.25, 0.00, 0.49)],
	)
}

pub fn greens(t: f32) -> Color {
	map_color(
		t,
		&[(0.93, 0.98, 0.90), (0.25, 0.70, 0.35), (0.00, 0.27, 0.08)],
	)
}

pub fn red_blue(t: f32) -> Color {
	map_color(t, &[(1.0, 0.0, 0.0), (0.6, 0.2, 0.8), (0.0, 0.0, 1.0)])
}

pub fn green_blue(t: f32) -> Color {
	map_color(t, &[(0.0, 1.0, 0.0), (0.0, 1.0, 1.0), (0.0, 0.0, 1.0)])
}

pub fn red_green(t: f32) -> Color {
	map_color(t, &[(1.0, 0.0, 0.0), (1.0, 1.0, 0.0), (0.0, 1.0, 0.0)])
}

pub fn red_white_blue(t: f32) -> Color {
	map_color(
		t,
		&[(0.70, 0.01, 0.15), (1.00, 1.00, 1.00), (0.02, 0.19, 0.46)],
	)
}

pub fn green_purples(t: f32) -> Color {
	map_color(
		t,
		&[(0.15, 0.44, 0.12), (1.00, 1.00, 1.00), (0.35, 0.11, 0.48)],
	)
}

pub fn blue_green(t: f32) -> Color {
	map_color(t, &[(0.0, 0.0, 1.0), (0.0, 1.0, 1.0), (0.0, 1.0, 0.0)])
}

pub fn blue_red(t: f32) -> Color {
	map_color(t, &[(0.0, 0.0, 1.0), (0.6, 0.2, 0.8), (1.0, 0.0, 0.0)])
}

pub fn blue_white_red(t: f32) -> Color {
	map_color(
		t,
		&[(0.02, 0.19, 0.46), (1.00, 1.00, 1.00), (0.70, 0.01, 0.15)],
	)
}

pub fn green_red(t: f32) -> Color {
	map_color(t, &[(0.0, 1.0, 0.0), (1.0, 1.0, 0.0), (1.0, 0.0, 0.0)])
}

pub fn purple_greens(t: f32) -> Color {
	map_color(
		t,
		&[(0.35, 0.11, 0.48), (1.00, 1.00, 1.00), (0.15, 0.44, 0.12)],
	)
}
