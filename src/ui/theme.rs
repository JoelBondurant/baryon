use crate::svp::highlight::{CATEGORY_COUNT, TokenCategory};
use ratatui::style::Color;

const THEMED_CATEGORIES: [TokenCategory; 20] = [
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

fn set_color(map: &mut [Option<Color>; CATEGORY_COUNT], category: TokenCategory, color: Color) {
	map[category as usize] = Some(color);
}

fn parse_theme_modifiers(name: &str) -> (&str, bool, bool, bool) {
	let Some((base, suffix)) = name.rsplit_once('_') else {
		return (name, false, false, false);
	};

	if suffix.is_empty() {
		return (name, false, false, false);
	}

	let mut reverse = false;
	let mut clip_low = false;
	let mut clip_high = false;
	for ch in suffix.chars() {
		match ch {
			'r' => reverse = true,
			'c' => clip_low = true,
			'C' => clip_high = true,
			_ => return (name, false, false, false),
		}
	}

	(base, reverse, clip_low, clip_high)
}

pub struct Theme {
	pub syntax_colors: [Option<Color>; CATEGORY_COUNT],
}

impl Theme {
	pub fn try_new(name: &str) -> Result<Self, String> {
		let mut colors = [None; CATEGORY_COUNT];
		let (raw_map_name, reverse, clip_low, clip_high) = parse_theme_modifiers(name);
		let mut map_name = raw_map_name.to_lowercase();

		let mut is_discrete = true;
		match map_name.as_str() {
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
				let assigned_cats: Vec<TokenCategory> = THEMED_CATEGORIES
					.iter()
					.copied()
					.filter(|&cat| colors[cat as usize].is_some())
					.collect();
				let mut assigned_colors: Vec<Color> = assigned_cats
					.iter()
					.filter_map(|&cat| colors[cat as usize])
					.collect();
				assigned_colors.reverse();
				for (cat, color) in assigned_cats.into_iter().zip(assigned_colors.into_iter()) {
					colors[cat as usize] = Some(color);
				}
			}
			return Ok(Self {
				syntax_colors: colors,
			});
		}

		if map_name.is_empty() {
			map_name.push_str("viridis");
		}

		let sampler: fn(f32) -> Color = match map_name.as_str() {
			"black" => black,
			"bluegreen" => blue_green,
			"bluered" => blue_red,
			"bluewhitered" => blue_white_red,
			"blues" => blues,
			"cividis" => cividis,
			"greenblue" => green_blue,
			"greenpurples" => green_purples,
			"greenred" => green_red,
			"greens" => greens,
			"grey" | "gray" => grey,
			"greys" | "grays" => greys,
			"inferno" => inferno,
			"magma" => magma,
			"oranges" => oranges,
			"plasma" => plasma,
			"purplegreens" => purple_greens,
			"purples" => purples,
			"rainbow" => rainbow,
			"redblue" => red_blue,
			"redgreen" => red_green,
			"reds" => reds,
			"redwhiteblue" => red_white_blue,
			"viridis" => viridis,
			"white" => white,
			"yellows" => yellows,
			_ => return Err(format!("Unknown theme: {}", name)),
		};

		let start = if clip_low { 0.2 } else { 0.0 };
		let end = if clip_high { 0.8 } else { 1.0 };
		let step_scale = end - start;
		let mut sampled_colors = [Color::Reset; THEMED_CATEGORIES.len()];

		for (i, color) in sampled_colors.iter_mut().enumerate() {
			let t = i as f32 / (THEMED_CATEGORIES.len() as f32 - 1.0);
			*color = sampler(start + (t * step_scale));
		}

		if reverse {
			sampled_colors.reverse();
		}

		for (cat, color) in THEMED_CATEGORIES
			.iter()
			.copied()
			.zip(sampled_colors.into_iter())
		{
			colors[cat as usize] = Some(color);
		}

		Ok(Self {
			syntax_colors: colors,
		})
	}

	fn fill_gruvbox(map: &mut [Option<Color>; CATEGORY_COUNT]) {
		let red = Color::Rgb(251, 73, 52);
		let green = Color::Rgb(184, 187, 38);
		let yellow = Color::Rgb(250, 189, 47);
		let blue = Color::Rgb(131, 165, 152);
		let purple = Color::Rgb(211, 134, 155);
		let aqua = Color::Rgb(142, 192, 124);
		let orange = Color::Rgb(254, 128, 25);
		let gray = Color::Rgb(146, 131, 116);
		let fg = Color::Rgb(235, 219, 178);

		set_color(map, TokenCategory::Keyword, red);
		set_color(map, TokenCategory::String, green);
		set_color(map, TokenCategory::Comment, gray);
		set_color(map, TokenCategory::Type, aqua);
		set_color(map, TokenCategory::Function, yellow);
		set_color(map, TokenCategory::Number, purple);
		set_color(map, TokenCategory::Punctuation, fg);
		set_color(map, TokenCategory::Variable, fg);
		set_color(map, TokenCategory::Constant, purple);
		set_color(map, TokenCategory::Macro, aqua);
		set_color(map, TokenCategory::Module, blue);
		set_color(map, TokenCategory::Lifetime, blue);
		set_color(map, TokenCategory::Attribute, aqua);
		set_color(map, TokenCategory::Operator, orange);
		set_color(map, TokenCategory::SelfKeyword, red);
		set_color(map, TokenCategory::BuiltinType, aqua);
		set_color(map, TokenCategory::MutableVariable, orange);
		set_color(map, TokenCategory::Method, yellow);
		set_color(map, TokenCategory::Crate, blue);
		set_color(map, TokenCategory::Whitespace, Color::Rgb(60, 56, 54));
	}

	fn fill_nord(map: &mut [Option<Color>; CATEGORY_COUNT]) {
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

		set_color(map, TokenCategory::Keyword, frost_3);
		set_color(map, TokenCategory::String, aurora_green);
		set_color(map, TokenCategory::Comment, polar_night_4);
		set_color(map, TokenCategory::Type, frost_1);
		set_color(map, TokenCategory::Function, aurora_yellow);
		set_color(map, TokenCategory::Number, aurora_purple);
		set_color(map, TokenCategory::Punctuation, snow_storm_1);
		set_color(map, TokenCategory::Variable, snow_storm_1);
		set_color(map, TokenCategory::Constant, aurora_purple);
		set_color(map, TokenCategory::Macro, frost_4);
		set_color(map, TokenCategory::Module, frost_3);
		set_color(map, TokenCategory::Lifetime, aurora_red);
		set_color(map, TokenCategory::Attribute, frost_1);
		set_color(map, TokenCategory::Operator, frost_2);
		set_color(map, TokenCategory::SelfKeyword, aurora_red);
		set_color(map, TokenCategory::BuiltinType, frost_1);
		set_color(map, TokenCategory::MutableVariable, aurora_orange);
		set_color(map, TokenCategory::Method, frost_2);
		set_color(map, TokenCategory::Crate, frost_3);
		set_color(map, TokenCategory::Whitespace, Color::Rgb(59, 66, 82));
	}

	fn fill_solarized(map: &mut [Option<Color>; CATEGORY_COUNT]) {
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

		set_color(map, TokenCategory::Keyword, green);
		set_color(map, TokenCategory::String, cyan);
		set_color(map, TokenCategory::Comment, base01);
		set_color(map, TokenCategory::Type, yellow);
		set_color(map, TokenCategory::Function, blue);
		set_color(map, TokenCategory::Number, magenta);
		set_color(map, TokenCategory::Punctuation, base0);
		set_color(map, TokenCategory::Variable, base0);
		set_color(map, TokenCategory::Constant, magenta);
		set_color(map, TokenCategory::Macro, blue);
		set_color(map, TokenCategory::Module, violet);
		set_color(map, TokenCategory::Lifetime, violet);
		set_color(map, TokenCategory::Attribute, yellow);
		set_color(map, TokenCategory::Operator, orange);
		set_color(map, TokenCategory::SelfKeyword, red);
		set_color(map, TokenCategory::BuiltinType, yellow);
		set_color(map, TokenCategory::MutableVariable, orange);
		set_color(map, TokenCategory::Method, blue);
		set_color(map, TokenCategory::Crate, violet);
		set_color(map, TokenCategory::Whitespace, Color::Rgb(7, 54, 66));
	}

	fn fill_pretty(map: &mut [Option<Color>; CATEGORY_COUNT]) {
		let fg = Color::Rgb(206, 206, 238);
		let comment = Color::Rgb(106, 106, 106);
		let constant = Color::Rgb(255, 204, 0);
		let number = Color::Rgb(255, 128, 0);
		let keyword = Color::Rgb(255, 0, 255);
		let type_col = Color::Rgb(0, 255, 128);
		let function = Color::Rgb(255, 255, 0);
		let whitespace = Color::Rgb(68, 68, 68);

		set_color(map, TokenCategory::Keyword, keyword);
		set_color(map, TokenCategory::String, constant);
		set_color(map, TokenCategory::Comment, comment);
		set_color(map, TokenCategory::Type, type_col);
		set_color(map, TokenCategory::Function, function);
		set_color(map, TokenCategory::Number, number);
		set_color(map, TokenCategory::Punctuation, fg);
		set_color(map, TokenCategory::Variable, type_col);
		set_color(map, TokenCategory::Constant, constant);
		set_color(map, TokenCategory::Macro, function);
		set_color(map, TokenCategory::Module, type_col);
		set_color(map, TokenCategory::Lifetime, keyword);
		set_color(map, TokenCategory::Attribute, type_col);
		set_color(map, TokenCategory::Operator, fg);
		set_color(map, TokenCategory::SelfKeyword, type_col);
		set_color(map, TokenCategory::BuiltinType, type_col);
		set_color(map, TokenCategory::MutableVariable, type_col);
		set_color(map, TokenCategory::Method, function);
		set_color(map, TokenCategory::Crate, type_col);
		set_color(map, TokenCategory::Whitespace, whitespace);
	}

	fn fill_classic(map: &mut [Option<Color>; CATEGORY_COUNT]) {
		let fg = Color::Rgb(206, 206, 238);
		let comment = Color::Rgb(106, 106, 106);
		let constant = Color::Rgb(255, 204, 0);
		let number = Color::Rgb(255, 128, 0);
		let keyword = Color::Rgb(0, 255, 255);
		let type_col = Color::Rgb(0, 255, 128);
		let function = Color::Rgb(255, 255, 0);
		let whitespace = Color::Rgb(68, 68, 68);

		set_color(map, TokenCategory::Keyword, keyword);
		set_color(map, TokenCategory::String, constant);
		set_color(map, TokenCategory::Comment, comment);
		set_color(map, TokenCategory::Type, type_col);
		set_color(map, TokenCategory::Function, function);
		set_color(map, TokenCategory::Number, number);
		set_color(map, TokenCategory::Punctuation, fg);
		set_color(map, TokenCategory::Variable, type_col);
		set_color(map, TokenCategory::Constant, constant);
		set_color(map, TokenCategory::Macro, function);
		set_color(map, TokenCategory::Module, type_col);
		set_color(map, TokenCategory::Lifetime, keyword);
		set_color(map, TokenCategory::Attribute, type_col);
		set_color(map, TokenCategory::Operator, fg);
		set_color(map, TokenCategory::SelfKeyword, type_col);
		set_color(map, TokenCategory::BuiltinType, type_col);
		set_color(map, TokenCategory::MutableVariable, type_col);
		set_color(map, TokenCategory::Method, function);
		set_color(map, TokenCategory::Crate, type_col);
		set_color(map, TokenCategory::Whitespace, whitespace);
	}

	fn fill_tab20(map: &mut [Option<Color>; CATEGORY_COUNT]) {
		let c01 = Color::Rgb(31, 119, 180);
		let c02 = Color::Rgb(174, 199, 232);
		let c03 = Color::Rgb(255, 127, 14);
		let c04 = Color::Rgb(255, 187, 120);
		let c05 = Color::Rgb(44, 160, 44);
		let c06 = Color::Rgb(152, 223, 138);
		let c07 = Color::Rgb(214, 39, 40);
		let c08 = Color::Rgb(255, 152, 150);
		let c09 = Color::Rgb(148, 103, 189);
		let c10 = Color::Rgb(197, 176, 213);
		let c11 = Color::Rgb(140, 86, 75);
		let c12 = Color::Rgb(196, 156, 148);
		let c13 = Color::Rgb(227, 119, 194);
		let c14 = Color::Rgb(247, 182, 210);
		let c15 = Color::Rgb(127, 127, 127);
		let c16 = Color::Rgb(199, 199, 199);
		let c17 = Color::Rgb(188, 189, 34);
		let c18 = Color::Rgb(219, 219, 141);
		let c19 = Color::Rgb(23, 190, 207);
		let c20 = Color::Rgb(158, 218, 229);

		set_color(map, TokenCategory::Keyword, c01);
		set_color(map, TokenCategory::String, c02);
		set_color(map, TokenCategory::Comment, c15);
		set_color(map, TokenCategory::Type, c03);
		set_color(map, TokenCategory::Function, c04);
		set_color(map, TokenCategory::Number, c05);
		set_color(map, TokenCategory::Punctuation, c16);
		set_color(map, TokenCategory::Variable, c06);
		set_color(map, TokenCategory::Constant, c07);
		set_color(map, TokenCategory::Macro, c08);
		set_color(map, TokenCategory::Module, c09);
		set_color(map, TokenCategory::Lifetime, c10);
		set_color(map, TokenCategory::Attribute, c11);
		set_color(map, TokenCategory::Operator, c12);
		set_color(map, TokenCategory::SelfKeyword, c13);
		set_color(map, TokenCategory::BuiltinType, c14);
		set_color(map, TokenCategory::MutableVariable, c17);
		set_color(map, TokenCategory::Method, c18);
		set_color(map, TokenCategory::Crate, c19);
		set_color(map, TokenCategory::Whitespace, c20);
	}

	fn fill_dark2(map: &mut [Option<Color>; CATEGORY_COUNT]) {
		let green = Color::Rgb(27, 158, 119);
		let orange = Color::Rgb(217, 95, 2);
		let purple = Color::Rgb(117, 112, 179);
		let pink = Color::Rgb(231, 41, 138);
		let lime = Color::Rgb(102, 166, 30);
		let yellow = Color::Rgb(230, 171, 2);
		let brown = Color::Rgb(166, 118, 29);
		let gray = Color::Rgb(102, 102, 102);

		set_color(map, TokenCategory::String, green);
		set_color(map, TokenCategory::Variable, orange);
		set_color(map, TokenCategory::MutableVariable, orange);
		set_color(map, TokenCategory::SelfKeyword, orange);
		set_color(map, TokenCategory::Number, purple);
		set_color(map, TokenCategory::Constant, purple);
		set_color(map, TokenCategory::Keyword, pink);
		set_color(map, TokenCategory::Operator, pink);
		set_color(map, TokenCategory::Macro, pink);
		set_color(map, TokenCategory::Type, lime);
		set_color(map, TokenCategory::BuiltinType, lime);
		set_color(map, TokenCategory::Function, yellow);
		set_color(map, TokenCategory::Method, yellow);
		set_color(map, TokenCategory::Module, brown);
		set_color(map, TokenCategory::Crate, brown);
		set_color(map, TokenCategory::Lifetime, brown);
		set_color(map, TokenCategory::Attribute, brown);
		set_color(map, TokenCategory::Comment, gray);
		set_color(map, TokenCategory::Punctuation, gray);
		set_color(map, TokenCategory::Whitespace, Color::Rgb(60, 60, 60));
	}

	fn fill_dracula(map: &mut [Option<Color>; CATEGORY_COUNT]) {
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

		set_color(map, TokenCategory::Keyword, pink);
		set_color(map, TokenCategory::String, yellow);
		set_color(map, TokenCategory::Comment, comment);
		set_color(map, TokenCategory::Type, cyan);
		set_color(map, TokenCategory::Function, green);
		set_color(map, TokenCategory::Number, purple);
		set_color(map, TokenCategory::Punctuation, fg);
		set_color(map, TokenCategory::Variable, fg);
		set_color(map, TokenCategory::Constant, purple);
		set_color(map, TokenCategory::Macro, cyan);
		set_color(map, TokenCategory::Module, cyan);
		set_color(map, TokenCategory::Lifetime, red);
		set_color(map, TokenCategory::Attribute, green);
		set_color(map, TokenCategory::Operator, pink);
		set_color(map, TokenCategory::SelfKeyword, red);
		set_color(map, TokenCategory::BuiltinType, cyan);
		set_color(map, TokenCategory::MutableVariable, orange);
		set_color(map, TokenCategory::Method, green);
		set_color(map, TokenCategory::Crate, cyan);
		set_color(map, TokenCategory::Whitespace, bg);
	}

	fn fill_one_dark(map: &mut [Option<Color>; CATEGORY_COUNT]) {
		let red = Color::Rgb(224, 108, 117);
		let green = Color::Rgb(152, 195, 121);
		let yellow = Color::Rgb(229, 192, 123);
		let blue = Color::Rgb(97, 175, 239);
		let magenta = Color::Rgb(198, 120, 221);
		let cyan = Color::Rgb(86, 182, 194);
		let white = Color::Rgb(171, 178, 191);
		let comment = Color::Rgb(92, 99, 112);
		let orange = Color::Rgb(209, 154, 102);

		set_color(map, TokenCategory::Keyword, magenta);
		set_color(map, TokenCategory::String, green);
		set_color(map, TokenCategory::Comment, comment);
		set_color(map, TokenCategory::Type, yellow);
		set_color(map, TokenCategory::Function, blue);
		set_color(map, TokenCategory::Number, orange);
		set_color(map, TokenCategory::Punctuation, white);
		set_color(map, TokenCategory::Variable, red);
		set_color(map, TokenCategory::Constant, orange);
		set_color(map, TokenCategory::Macro, cyan);
		set_color(map, TokenCategory::Module, blue);
		set_color(map, TokenCategory::Lifetime, magenta);
		set_color(map, TokenCategory::Attribute, white);
		set_color(map, TokenCategory::Operator, cyan);
		set_color(map, TokenCategory::SelfKeyword, red);
		set_color(map, TokenCategory::BuiltinType, cyan);
		set_color(map, TokenCategory::MutableVariable, orange);
		set_color(map, TokenCategory::Method, blue);
		set_color(map, TokenCategory::Crate, cyan);
		set_color(map, TokenCategory::Whitespace, Color::Rgb(40, 44, 52));
	}

	fn fill_tokyo_night(map: &mut [Option<Color>; CATEGORY_COUNT]) {
		let fg = Color::Rgb(192, 202, 245);
		let comment = Color::Rgb(86, 95, 137);
		let cyan = Color::Rgb(125, 207, 255);
		let blue = Color::Rgb(122, 162, 247);
		let magenta = Color::Rgb(187, 154, 247);
		let orange = Color::Rgb(255, 158, 100);
		let green = Color::Rgb(158, 206, 106);
		let red = Color::Rgb(247, 118, 142);
		let bg = Color::Rgb(26, 27, 38);

		set_color(map, TokenCategory::Keyword, magenta);
		set_color(map, TokenCategory::String, green);
		set_color(map, TokenCategory::Comment, comment);
		set_color(map, TokenCategory::Type, cyan);
		set_color(map, TokenCategory::Function, blue);
		set_color(map, TokenCategory::Number, orange);
		set_color(map, TokenCategory::Punctuation, fg);
		set_color(map, TokenCategory::Variable, fg);
		set_color(map, TokenCategory::Constant, orange);
		set_color(map, TokenCategory::Macro, blue);
		set_color(map, TokenCategory::Module, blue);
		set_color(map, TokenCategory::Lifetime, red);
		set_color(map, TokenCategory::Attribute, cyan);
		set_color(map, TokenCategory::Operator, fg);
		set_color(map, TokenCategory::SelfKeyword, red);
		set_color(map, TokenCategory::BuiltinType, cyan);
		set_color(map, TokenCategory::MutableVariable, orange);
		set_color(map, TokenCategory::Method, blue);
		set_color(map, TokenCategory::Crate, blue);
		set_color(map, TokenCategory::Whitespace, bg);
	}

	fn fill_kanagawa(map: &mut [Option<Color>; CATEGORY_COUNT]) {
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

		set_color(map, TokenCategory::Keyword, oni_violet);
		set_color(map, TokenCategory::String, spring_green);
		set_color(map, TokenCategory::Comment, fuji_gray);
		set_color(map, TokenCategory::Type, wave_aqua);
		set_color(map, TokenCategory::Function, crystal_blue);
		set_color(map, TokenCategory::Number, sakura_pink);
		set_color(map, TokenCategory::Punctuation, sumi_ink);
		set_color(map, TokenCategory::Variable, fuji_white);
		set_color(map, TokenCategory::Constant, surimi_orange);
		set_color(map, TokenCategory::Macro, oni_violet);
		set_color(map, TokenCategory::Module, crystal_blue);
		set_color(map, TokenCategory::Lifetime, surimi_orange);
		set_color(map, TokenCategory::Attribute, wave_aqua);
		set_color(map, TokenCategory::Operator, carp_yellow);
		set_color(map, TokenCategory::SelfKeyword, oni_violet);
		set_color(map, TokenCategory::BuiltinType, wave_aqua);
		set_color(map, TokenCategory::MutableVariable, surimi_orange);
		set_color(map, TokenCategory::Method, crystal_blue);
		set_color(map, TokenCategory::Crate, wave_aqua);
		set_color(map, TokenCategory::Whitespace, Color::Rgb(31, 31, 40));
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

#[cfg(test)]
mod tests {
	use super::{Theme, plasma, viridis};
	use crate::svp::highlight::TokenCategory;
	use ratatui::style::Color;

	fn color_for(theme: &Theme, category: TokenCategory) -> Color {
		theme.syntax_colors[category as usize].expect("themed color")
	}

	#[test]
	fn continuous_themes_default_to_full_domain() {
		let theme = Theme::try_new("plasma").expect("plasma theme");

		assert_eq!(color_for(&theme, TokenCategory::Keyword), plasma(0.0));
		assert_eq!(color_for(&theme, TokenCategory::Whitespace), plasma(1.0));
	}

	#[test]
	fn unordered_clip_and_reverse_modifiers_compose_after_sampling() {
		let theme = Theme::try_new("viridis_cCr").expect("viridis_cCr theme");
		let permuted = Theme::try_new("viridis_rcC").expect("viridis_rcC theme");

		assert_eq!(color_for(&theme, TokenCategory::Keyword), viridis(0.8));
		assert_eq!(color_for(&theme, TokenCategory::Whitespace), viridis(0.2));
		assert_eq!(theme.syntax_colors, permuted.syntax_colors);
	}

	#[test]
	fn discrete_themes_ignore_clip_modifiers_but_still_reverse() {
		let reversed = Theme::try_new("classic_r").expect("classic_r theme");
		let clipped_reversed = Theme::try_new("classic_rC").expect("classic_rC theme");

		assert_eq!(reversed.syntax_colors, clipped_reversed.syntax_colors);
	}

	#[test]
	fn rev_suffix_is_no_longer_supported() {
		assert!(Theme::try_new("viridis_rev").is_err());
	}
}
