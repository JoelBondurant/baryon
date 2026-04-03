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

		let (map_name, reverse) = if let Some(stripped) = name.strip_suffix("_r") {
			(stripped, true)
		} else {
			(name, false)
		};

		for (i, &cat) in categories.iter().enumerate() {
			let mut t = i as f64 / (categories.len() as f64 - 1.0);
			if reverse {
				t = 1.0 - t;
			}
			
			// Clipping: skip the dark end of the ramp
			// We'll map [0, 1] to [0.2, 1.0] to skip the deep purples
			let t_clipped = 0.2 + (t * 0.8);

			let color = match map_name {
				"cividis" => cividis(t_clipped),
				"magma" => magma(t_clipped),
				"inferno" => inferno(t_clipped),
				"plasma" => plasma(t_clipped),
				_ => viridis(t_clipped), // default to viridis
			};
			colors.insert(cat, color);
		}

		Self { syntax_colors: colors }
	}
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
	a + (b - a) * t
}

fn interpolate(colors: &[[f64; 3]], t: f64) -> Color {
	let t = t.clamp(0.0, 1.0);
	let n = colors.len() - 1;
	let scaled_t = t * n as f64;
	let idx = scaled_t.floor() as usize;
	let frac = scaled_t - idx as f64;

	if idx >= n {
		let [r, g, b] = colors[n];
		return Color::Rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8);
	}

	let [r1, g1, b1] = colors[idx];
	let [r2, g2, b2] = colors[idx + 1];

	Color::Rgb(
		(lerp(r1, r2, frac) * 255.0) as u8,
		(lerp(g1, g2, frac) * 255.0) as u8,
		(lerp(b1, b2, frac) * 255.0) as u8,
	)
}

fn viridis(t: f64) -> Color {
	let data = [
		[0.267004, 0.004874, 0.329415],
		[0.190631, 0.407061, 0.556089],
		[0.208625, 0.712158, 0.483037],
		[0.993248, 0.906157, 0.143936],
	];
	interpolate(&data, t)
}

fn cividis(t: f64) -> Color {
	let data = [
		[0.000000, 0.135112, 0.304751],
		[0.303115, 0.354115, 0.448625],
		[0.611115, 0.611115, 0.611115],
		[0.920221, 0.829555, 0.209851],
	];
	interpolate(&data, t)
}

fn magma(t: f64) -> Color {
	let data = [
		[0.001462, 0.000466, 0.013866],
		[0.316654, 0.071690, 0.485380],
		[0.724760, 0.191150, 0.526540],
		[0.987053, 0.645320, 0.439350],
		[0.986700, 0.999037, 0.616051],
	];
	interpolate(&data, t)
}

fn inferno(t: f64) -> Color {
	let data = [
		[0.001462, 0.000466, 0.013866],
		[0.338230, 0.070660, 0.462360],
		[0.735680, 0.160540, 0.447020],
		[0.964260, 0.589250, 0.274240],
		[0.988362, 0.998364, 0.644924],
	];
	interpolate(&data, t)
}

fn plasma(t: f64) -> Color {
	let data = [
		[0.050383, 0.029803, 0.527975],
		[0.493410, 0.011530, 0.657950],
		[0.796380, 0.278830, 0.471360],
		[0.953120, 0.675400, 0.242060],
		[0.940015, 0.975158, 0.131326],
	];
	interpolate(&data, t)
}
