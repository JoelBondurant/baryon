use ratatui::style::Color;
use crate::svp::highlight::TokenCategory;
use std::collections::HashMap;

pub struct Theme {
	pub syntax_colors: HashMap<TokenCategory, Color>,
}

impl Theme {
	pub fn try_new(name: &str) -> Result<Self, String> {
		let mut colors = HashMap::new();
		let name_lower = name.to_lowercase();

		let (mut map_name, reverse) = if let Some(stripped) = name_lower.strip_suffix("_r") {
			(stripped, true)
		} else if let Some(stripped) = name_lower.strip_suffix("_rev") {
			(stripped, true)
		} else {
			(name_lower.as_str(), false)
		};

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

		// Handle Discrete Themes
		let mut is_discrete = true;
		match map_name {
			"classic" | "original" => Self::fill_classic(&mut colors),
			"pretty" => Self::fill_pretty(&mut colors),
			"gruvbox" => Self::fill_gruvbox(&mut colors),
			"nord" => Self::fill_nord(&mut colors),
			"solarized" | "solarized_dark" => Self::fill_solarized(&mut colors),
			"tab20" => Self::fill_tab20(&mut colors),
			"dark2" => Self::fill_dark2(&mut colors),
			"dracula" => Self::fill_dracula(&mut colors),
			"onedark" | "one_dark" => Self::fill_one_dark(&mut colors),
			"tokyonight" | "tokyo_night" => Self::fill_tokyo_night(&mut colors),
			"kanagawa" => Self::fill_kanagawa(&mut colors),
			_ => is_discrete = false,
		}

		if is_discrete {
			if reverse {
				let mut assigned_colors: Vec<Color> = categories.iter().filter_map(|c| colors.get(c).copied()).collect();
				assigned_colors.reverse();
				let assigned_cats: Vec<TokenCategory> = categories.iter().copied().filter(|c| colors.contains_key(c)).collect();
				for (i, cat) in assigned_cats.iter().enumerate() {
					colors.insert(*cat, assigned_colors[i]);
				}
			}
			return Ok(Self { syntax_colors: colors });
		}

		if map_name.is_empty() { map_name = "viridis"; }

		let valid_continuous = [
			"black", "bluegreen", "bluered", "bluewhitered", "blues", "cividis",
			"greenblue", "greenpurples", "greenred", "greens", "grey", "gray", "greys", "grays",
			"inferno", "magma", "oranges", "plasma", "purplegreens", "purples", "rainbow",
			"redblue", "redgreen", "reds", "redwhiteblue", "viridis", "white", "yellows",
		];
		
		if !valid_continuous.contains(&map_name) {
			return Err(format!("Unknown theme: {}", name));
		}

		for (i, &cat) in categories.iter().enumerate() {
			let mut t = i as f32 / (categories.len() as f32 - 1.0);
			if reverse {
				t = 1.0 - t;
			}
			
			let t_clipped = 0.2 + (t * 0.8);

			let color = match map_name {
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
				"plasma" => plasma(t_clipped),
				_ => viridis(t_clipped),
			};
			colors.insert(cat, color);
		}

		Ok(Self { syntax_colors: colors })
	}

	fn fill_gruvbox(map: &mut HashMap<TokenCategory, Color>) {
		let red = Color::Rgb(251, 73, 52);
		let green = Color::Rgb(184, 187, 38);
		let yellow = Color::Rgb(250, 189, 47);
		let blue = Color::Rgb(131, 165, 152);
		let purple = Color::Rgb(211, 134, 155);
		let aqua = Color::Rgb(142, 192, 124);
		let orange = Color::Rgb(254, 128, 25);
		let gray = Color::Rgb(146, 131, 116);
		let fg = Color::Rgb(235, 219, 178);

		map.insert(TokenCategory::Keyword, red);
		map.insert(TokenCategory::String, green);
		map.insert(TokenCategory::Comment, gray);
		map.insert(TokenCategory::Type, aqua);
		map.insert(TokenCategory::Function, yellow);
		map.insert(TokenCategory::Number, purple);
		map.insert(TokenCategory::Punctuation, fg);
		map.insert(TokenCategory::Variable, fg);
		map.insert(TokenCategory::Constant, purple);
		map.insert(TokenCategory::Macro, aqua);
		map.insert(TokenCategory::Module, blue);
		map.insert(TokenCategory::Lifetime, blue);
		map.insert(TokenCategory::Attribute, aqua);
		map.insert(TokenCategory::Operator, orange);
		map.insert(TokenCategory::SelfKeyword, red);
		map.insert(TokenCategory::BuiltinType, aqua);
		map.insert(TokenCategory::MutableVariable, orange);
		map.insert(TokenCategory::Method, yellow);
		map.insert(TokenCategory::Crate, blue);
		map.insert(TokenCategory::Whitespace, Color::Rgb(60, 56, 54));
	}

	fn fill_nord(map: &mut HashMap<TokenCategory, Color>) {
		let frost_1 = Color::Rgb(143, 188, 187);
		let frost_2 = Color::Rgb(136, 192, 208);
		let frost_3 = Color::Rgb(129, 161, 193);
		let frost_4 = Color::Rgb(94, 129, 172);
		let aurora_red = Color::Rgb(191, 97, 106);
		let aurora_orange = Color::Rgb(208, 135, 112);
		let aurora_yellow = Color::Rgb(235, 203, 139);
		let aurora_green = Color::Rgb(163, 190, 140);
		let aurora_purple = Color::Rgb(180, 142, 173);
		let snow_storm_1 = Color::Rgb(216, 222, 233);
		let polar_night_4 = Color::Rgb(76, 86, 106);

		map.insert(TokenCategory::Keyword, frost_3);
		map.insert(TokenCategory::String, aurora_green);
		map.insert(TokenCategory::Comment, polar_night_4);
		map.insert(TokenCategory::Type, frost_1);
		map.insert(TokenCategory::Function, aurora_yellow);
		map.insert(TokenCategory::Number, aurora_purple);
		map.insert(TokenCategory::Punctuation, snow_storm_1);
		map.insert(TokenCategory::Variable, snow_storm_1);
		map.insert(TokenCategory::Constant, aurora_purple);
		map.insert(TokenCategory::Macro, frost_4);
		map.insert(TokenCategory::Module, frost_3);
		map.insert(TokenCategory::Lifetime, aurora_red);
		map.insert(TokenCategory::Attribute, frost_1);
		map.insert(TokenCategory::Operator, frost_2);
		map.insert(TokenCategory::SelfKeyword, aurora_red);
		map.insert(TokenCategory::BuiltinType, frost_1);
		map.insert(TokenCategory::MutableVariable, aurora_orange);
		map.insert(TokenCategory::Method, frost_2);
		map.insert(TokenCategory::Crate, frost_3);
		map.insert(TokenCategory::Whitespace, Color::Rgb(59, 66, 82));
	}

	fn fill_solarized(map: &mut HashMap<TokenCategory, Color>) {
		let base01 = Color::Rgb(88, 110, 117);
		let base0 = Color::Rgb(131, 148, 150);
		let yellow = Color::Rgb(181, 137, 0);
		let orange = Color::Rgb(203, 75, 22);
		let red = Color::Rgb(220, 50, 47);
		let magenta = Color::Rgb(211, 54, 130);
		let violet = Color::Rgb(108, 113, 196);
		let blue = Color::Rgb(38, 139, 210);
		let cyan = Color::Rgb(42, 161, 152);
		let green = Color::Rgb(133, 153, 0);

		map.insert(TokenCategory::Keyword, green);
		map.insert(TokenCategory::String, cyan);
		map.insert(TokenCategory::Comment, base01);
		map.insert(TokenCategory::Type, yellow);
		map.insert(TokenCategory::Function, blue);
		map.insert(TokenCategory::Number, magenta);
		map.insert(TokenCategory::Punctuation, base0);
		map.insert(TokenCategory::Variable, base0);
		map.insert(TokenCategory::Constant, magenta);
		map.insert(TokenCategory::Macro, blue);
		map.insert(TokenCategory::Module, violet);
		map.insert(TokenCategory::Lifetime, violet);
		map.insert(TokenCategory::Attribute, yellow);
		map.insert(TokenCategory::Operator, orange);
		map.insert(TokenCategory::SelfKeyword, red);
		map.insert(TokenCategory::BuiltinType, yellow);
		map.insert(TokenCategory::MutableVariable, orange);
		map.insert(TokenCategory::Method, blue);
		map.insert(TokenCategory::Crate, violet);
		map.insert(TokenCategory::Whitespace, Color::Rgb(7, 54, 66));
	}

	fn fill_pretty(map: &mut HashMap<TokenCategory, Color>) {
		let fg = Color::Rgb(206, 206, 238);
		let comment = Color::Rgb(106, 106, 106);
		let constant = Color::Rgb(255, 204, 0);
		let number = Color::Rgb(255, 128, 0);
		let keyword = Color::Rgb(255, 0, 255);
		let type_col = Color::Rgb(0, 255, 128);
		let function = Color::Rgb(255, 255, 0);
		let whitespace = Color::Rgb(68, 68, 68);

		map.insert(TokenCategory::Keyword, keyword);
		map.insert(TokenCategory::String, constant);
		map.insert(TokenCategory::Comment, comment);
		map.insert(TokenCategory::Type, type_col);
		map.insert(TokenCategory::Function, function);
		map.insert(TokenCategory::Number, number);
		map.insert(TokenCategory::Punctuation, fg);
		map.insert(TokenCategory::Variable, type_col); // Green!
		map.insert(TokenCategory::Constant, constant);
		map.insert(TokenCategory::Macro, function);
		map.insert(TokenCategory::Module, type_col); // Green!
		map.insert(TokenCategory::Lifetime, keyword);
		map.insert(TokenCategory::Attribute, type_col);
		map.insert(TokenCategory::Operator, fg);
		map.insert(TokenCategory::SelfKeyword, type_col);
		map.insert(TokenCategory::BuiltinType, type_col);
		map.insert(TokenCategory::MutableVariable, type_col);
		map.insert(TokenCategory::Method, function);
		map.insert(TokenCategory::Crate, type_col); // Green!
		map.insert(TokenCategory::Whitespace, whitespace);
	}

	fn fill_classic(map: &mut HashMap<TokenCategory, Color>) {
		let fg = Color::Rgb(206, 206, 238);         // Normal / Punctuation
		let comment = Color::Rgb(106, 106, 106);
		let constant = Color::Rgb(255, 204, 0);
		let number = Color::Rgb(255, 128, 0);
		let keyword = Color::Rgb(0, 255, 255);      // Cyan for Keywords (User's original)
		let type_col = Color::Rgb(0, 255, 128);     // Green for Types and Identifiers
		let function = Color::Rgb(255, 255, 0);     // Yellow for Functions
		let whitespace = Color::Rgb(68, 68, 68);

		map.insert(TokenCategory::Keyword, keyword);
		map.insert(TokenCategory::String, constant);
		map.insert(TokenCategory::Comment, comment);
		map.insert(TokenCategory::Type, type_col);
		map.insert(TokenCategory::Function, function);
		map.insert(TokenCategory::Number, number);
		map.insert(TokenCategory::Punctuation, fg);
		map.insert(TokenCategory::Variable, type_col); // NeoVim Identifier is Green
		map.insert(TokenCategory::Constant, constant);
		map.insert(TokenCategory::Macro, function);
		map.insert(TokenCategory::Module, type_col); // NeoVim Identifier
		map.insert(TokenCategory::Lifetime, keyword);
		map.insert(TokenCategory::Attribute, type_col);
		map.insert(TokenCategory::Operator, fg);
		map.insert(TokenCategory::SelfKeyword, type_col);
		map.insert(TokenCategory::BuiltinType, type_col);
		map.insert(TokenCategory::MutableVariable, type_col);
		map.insert(TokenCategory::Method, function);
		map.insert(TokenCategory::Crate, type_col);
		map.insert(TokenCategory::Whitespace, whitespace);
	}

	fn fill_tab20(map: &mut HashMap<TokenCategory, Color>) {
		// Matplotlib tab20 palette
		let c01 = Color::Rgb(31, 119, 180);  // Blue
		let c02 = Color::Rgb(174, 199, 232); // Light Blue
		let c03 = Color::Rgb(255, 127, 14);  // Orange
		let c04 = Color::Rgb(255, 187, 120); // Light Orange
		let c05 = Color::Rgb(44, 160, 44);   // Green
		let c06 = Color::Rgb(152, 223, 138); // Light Green
		let c07 = Color::Rgb(214, 39, 40);   // Red
		let c08 = Color::Rgb(255, 152, 150); // Light Red
		let c09 = Color::Rgb(148, 103, 189); // Purple
		let c10 = Color::Rgb(197, 176, 213); // Light Purple
		let c11 = Color::Rgb(140, 86, 75);   // Brown
		let c12 = Color::Rgb(196, 156, 148); // Light Brown
		let c13 = Color::Rgb(227, 119, 194); // Pink
		let c14 = Color::Rgb(247, 182, 210); // Light Pink
		let c15 = Color::Rgb(127, 127, 127); // Gray
		let c16 = Color::Rgb(199, 199, 199); // Light Gray
		let c17 = Color::Rgb(188, 189, 34);  // Olive
		let c18 = Color::Rgb(219, 219, 141); // Light Olive
		let c19 = Color::Rgb(23, 190, 207);  // Cyan
		let c20 = Color::Rgb(158, 218, 229); // Light Cyan

		map.insert(TokenCategory::Keyword, c01);
		map.insert(TokenCategory::String, c02);
		map.insert(TokenCategory::Comment, c15); // Use Gray for comments
		map.insert(TokenCategory::Type, c03);
		map.insert(TokenCategory::Function, c04);
		map.insert(TokenCategory::Number, c05);
		map.insert(TokenCategory::Punctuation, c16); // Light Gray
		map.insert(TokenCategory::Variable, c06);
		map.insert(TokenCategory::Constant, c07);
		map.insert(TokenCategory::Macro, c08);
		map.insert(TokenCategory::Module, c09);
		map.insert(TokenCategory::Lifetime, c10);
		map.insert(TokenCategory::Attribute, c11);
		map.insert(TokenCategory::Operator, c12);
		map.insert(TokenCategory::SelfKeyword, c13);
		map.insert(TokenCategory::BuiltinType, c14);
		map.insert(TokenCategory::MutableVariable, c17);
		map.insert(TokenCategory::Method, c18);
		map.insert(TokenCategory::Crate, c19);
		map.insert(TokenCategory::Whitespace, c20);
	}

	fn fill_dark2(map: &mut HashMap<TokenCategory, Color>) {
		// ColorBrewer Dark2 palette
		let green = Color::Rgb(27, 158, 119);
		let orange = Color::Rgb(217, 95, 2);
		let purple = Color::Rgb(117, 112, 179);
		let pink = Color::Rgb(231, 41, 138);
		let lime = Color::Rgb(102, 166, 30);
		let yellow = Color::Rgb(230, 171, 2);
		let brown = Color::Rgb(166, 118, 29);
		let gray = Color::Rgb(102, 102, 102);

		map.insert(TokenCategory::String, green);
		
		map.insert(TokenCategory::Variable, orange);
		map.insert(TokenCategory::MutableVariable, orange);
		map.insert(TokenCategory::SelfKeyword, orange);

		map.insert(TokenCategory::Number, purple);
		map.insert(TokenCategory::Constant, purple);

		map.insert(TokenCategory::Keyword, pink);
		map.insert(TokenCategory::Operator, pink);
		map.insert(TokenCategory::Macro, pink);

		map.insert(TokenCategory::Type, lime);
		map.insert(TokenCategory::BuiltinType, lime);
		
		map.insert(TokenCategory::Function, yellow);
		map.insert(TokenCategory::Method, yellow);

		map.insert(TokenCategory::Module, brown);
		map.insert(TokenCategory::Crate, brown);
		map.insert(TokenCategory::Lifetime, brown);
		map.insert(TokenCategory::Attribute, brown);

		map.insert(TokenCategory::Comment, gray);
		map.insert(TokenCategory::Punctuation, gray);
		map.insert(TokenCategory::Whitespace, Color::Rgb(60, 60, 60)); // slightly darker gray
	}

	fn fill_dracula(map: &mut HashMap<TokenCategory, Color>) {
		let fg = Color::Rgb(248, 248, 242);
		let comment = Color::Rgb(98, 114, 164);
		let red = Color::Rgb(255, 85, 85);
		let orange = Color::Rgb(255, 184, 108);
		let yellow = Color::Rgb(241, 250, 140);
		let green = Color::Rgb(80, 250, 123);
		let purple = Color::Rgb(189, 147, 249);
		let cyan = Color::Rgb(139, 233, 253);
		let pink = Color::Rgb(255, 121, 198);
		let bg = Color::Rgb(40, 42, 54);

		map.insert(TokenCategory::Keyword, pink);
		map.insert(TokenCategory::String, yellow);
		map.insert(TokenCategory::Comment, comment);
		map.insert(TokenCategory::Type, cyan);
		map.insert(TokenCategory::Function, green);
		map.insert(TokenCategory::Number, purple);
		map.insert(TokenCategory::Punctuation, fg);
		map.insert(TokenCategory::Variable, fg);
		map.insert(TokenCategory::Constant, purple);
		map.insert(TokenCategory::Macro, cyan);
		map.insert(TokenCategory::Module, cyan);
		map.insert(TokenCategory::Lifetime, red);
		map.insert(TokenCategory::Attribute, green);
		map.insert(TokenCategory::Operator, pink);
		map.insert(TokenCategory::SelfKeyword, red);
		map.insert(TokenCategory::BuiltinType, cyan);
		map.insert(TokenCategory::MutableVariable, orange);
		map.insert(TokenCategory::Method, green);
		map.insert(TokenCategory::Crate, cyan);
		map.insert(TokenCategory::Whitespace, bg);
	}

	fn fill_one_dark(map: &mut HashMap<TokenCategory, Color>) {
		let red = Color::Rgb(224, 108, 117);
		let green = Color::Rgb(152, 195, 121);
		let yellow = Color::Rgb(229, 192, 123);
		let blue = Color::Rgb(97, 175, 239);
		let magenta = Color::Rgb(198, 120, 221);
		let cyan = Color::Rgb(86, 182, 194);
		let white = Color::Rgb(171, 178, 191);
		let comment = Color::Rgb(92, 99, 112);
		let orange = Color::Rgb(209, 154, 102);

		map.insert(TokenCategory::Keyword, magenta);
		map.insert(TokenCategory::String, green);
		map.insert(TokenCategory::Comment, comment);
		map.insert(TokenCategory::Type, yellow);
		map.insert(TokenCategory::Function, blue);
		map.insert(TokenCategory::Number, orange);
		map.insert(TokenCategory::Punctuation, white);
		map.insert(TokenCategory::Variable, red);
		map.insert(TokenCategory::Constant, orange);
		map.insert(TokenCategory::Macro, cyan);
		map.insert(TokenCategory::Module, blue);
		map.insert(TokenCategory::Lifetime, magenta);
		map.insert(TokenCategory::Attribute, white);
		map.insert(TokenCategory::Operator, cyan);
		map.insert(TokenCategory::SelfKeyword, red);
		map.insert(TokenCategory::BuiltinType, cyan);
		map.insert(TokenCategory::MutableVariable, orange);
		map.insert(TokenCategory::Method, blue);
		map.insert(TokenCategory::Crate, cyan);
		map.insert(TokenCategory::Whitespace, Color::Rgb(40, 44, 52));
	}

	fn fill_tokyo_night(map: &mut HashMap<TokenCategory, Color>) {
		let fg = Color::Rgb(192, 202, 245);
		let comment = Color::Rgb(86, 95, 137);
		let cyan = Color::Rgb(125, 207, 255);
		let blue = Color::Rgb(122, 162, 247);
		let magenta = Color::Rgb(187, 154, 247);
		let orange = Color::Rgb(255, 158, 100);
		let green = Color::Rgb(158, 206, 106);
		let red = Color::Rgb(247, 118, 142);
		let bg = Color::Rgb(26, 27, 38);

		map.insert(TokenCategory::Keyword, magenta);
		map.insert(TokenCategory::String, green);
		map.insert(TokenCategory::Comment, comment);
		map.insert(TokenCategory::Type, cyan);
		map.insert(TokenCategory::Function, blue);
		map.insert(TokenCategory::Number, orange);
		map.insert(TokenCategory::Punctuation, fg);
		map.insert(TokenCategory::Variable, fg);
		map.insert(TokenCategory::Constant, orange);
		map.insert(TokenCategory::Macro, blue);
		map.insert(TokenCategory::Module, blue);
		map.insert(TokenCategory::Lifetime, red);
		map.insert(TokenCategory::Attribute, cyan);
		map.insert(TokenCategory::Operator, fg);
		map.insert(TokenCategory::SelfKeyword, red);
		map.insert(TokenCategory::BuiltinType, cyan);
		map.insert(TokenCategory::MutableVariable, orange);
		map.insert(TokenCategory::Method, blue);
		map.insert(TokenCategory::Crate, blue);
		map.insert(TokenCategory::Whitespace, bg);
	}

	fn fill_kanagawa(map: &mut HashMap<TokenCategory, Color>) {
		let fuji_white = Color::Rgb(220, 215, 186);
		let fuji_gray = Color::Rgb(114, 113, 105);
		let oni_violet = Color::Rgb(149, 127, 184);
		let spring_green = Color::Rgb(152, 187, 108);
		let crystal_blue = Color::Rgb(126, 156, 216);
		let wave_aqua = Color::Rgb(122, 168, 159);
		let sakura_pink = Color::Rgb(210, 126, 153);
		let surimi_orange = Color::Rgb(255, 160, 102);
		let carp_yellow = Color::Rgb(192, 163, 110);
		let sumi_ink = Color::Rgb(84, 84, 100);

		map.insert(TokenCategory::Keyword, oni_violet);
		map.insert(TokenCategory::String, spring_green);
		map.insert(TokenCategory::Comment, fuji_gray);
		map.insert(TokenCategory::Type, wave_aqua);
		map.insert(TokenCategory::Function, crystal_blue);
		map.insert(TokenCategory::Number, sakura_pink);
		map.insert(TokenCategory::Punctuation, sumi_ink);
		map.insert(TokenCategory::Variable, fuji_white);
		map.insert(TokenCategory::Constant, surimi_orange);
		map.insert(TokenCategory::Macro, oni_violet);
		map.insert(TokenCategory::Module, crystal_blue);
		map.insert(TokenCategory::Lifetime, surimi_orange);
		map.insert(TokenCategory::Attribute, wave_aqua);
		map.insert(TokenCategory::Operator, carp_yellow);
		map.insert(TokenCategory::SelfKeyword, oni_violet);
		map.insert(TokenCategory::BuiltinType, wave_aqua);
		map.insert(TokenCategory::MutableVariable, surimi_orange);
		map.insert(TokenCategory::Method, crystal_blue);
		map.insert(TokenCategory::Crate, wave_aqua);
		map.insert(TokenCategory::Whitespace, Color::Rgb(31, 31, 40));
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
