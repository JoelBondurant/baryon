use ratatui::style::Color;

pub const BG: Color = Color::Rgb(18, 18, 18);
pub const GUTTER_BG: Color = Color::Rgb(18, 18, 18);
pub const GUTTER_FG: Color = Color::Indexed(242);
pub const SELECTION_BG: Color = Color::Rgb(62, 68, 82);
pub const MINIMAP_BG: Color = Color::Rgb(16, 20, 28);
pub const STATUS_BAR_BG: Color = Color::Rgb(18, 18, 18);
pub const STATUS_BAR_FG: Color = Color::White;

pub const MODE_NORMAL_BG: Color = Color::Rgb(130, 170, 255);
pub const MODE_INSERT_BG: Color = Color::Rgb(180, 230, 130);
pub const MODE_COMMAND_BG: Color = Color::Rgb(255, 180, 100);
pub const MODE_SEARCH_BG: Color = Color::Rgb(200, 160, 255);
pub const MODE_CONFIRM_BG: Color = Color::Rgb(255, 120, 120);
pub const MODE_VISUAL_BG: Color = Color::Rgb(120, 200, 200);

pub const MODE_TEXT_FG: Color = Color::Rgb(0, 0, 0);

pub const DIRTY_FG: Color = Color::Rgb(255, 200, 120);
pub const DIRTY_BULLET_FG: Color = Color::Rgb(255, 160, 80);

pub const SEARCH_INFO_FG: Color = Color::Rgb(200, 160, 255);
pub const SEARCH_DIM_FG: Color = Color::Indexed(242);

pub const SEARCH_MATCH_BG: Color = Color::Rgb(180, 140, 50);
pub const SEARCH_MATCH_FG: Color = Color::Black;
pub const DIAGNOSTIC_ERROR_UNDERLINE: Color = Color::Rgb(255, 92, 92);
pub const FOLDED_PLACEHOLDER_BG: Color = Color::Rgb(4, 2, 8);
pub const FOLDED_PLACEHOLDER_FG: Color = Color::Rgb(76, 255, 0);
pub const CURSOR_LINE_NUMBER: Color = FOLDED_PLACEHOLDER_FG;

pub const CURSOR_LINE_BG: Color = Color::Rgb(32, 32, 32);

pub const TEXT_FG: Color = Color::Indexed(253);

// Minimap colors (RGBA for image crate)
pub const MINIMAP_RGBA_BG: [u8; 4] = [18, 18, 18, 255];
pub const MINIMAP_RGBA_DENSITY: [u8; 4] = [110, 110, 110, 255];
pub const MINIMAP_RGBA_FALLBACK: [u8; 4] = [76, 76, 76, 255];
pub const MINIMAP_RGBA_SEARCH_ACTIVE: [u8; 4] = [224, 224, 224, 255];
pub const MINIMAP_RGBA_SEARCH_INACTIVE: [u8; 4] = [160, 160, 160, 255];
pub const MINIMAP_RGBA_FRAME: [u8; 4] = [186, 186, 186, 255];
pub const MINIMAP_RGBA_FRAME_SIDE: [u8; 4] = [162, 162, 162, 255];
pub const MINIMAP_RGBA_CURSOR: [u8; 4] = [218, 218, 218, 255];
pub const MINIMAP_CURSOR_LINE: Color = CURSOR_LINE_NUMBER;
pub const MINIMAP_RGBA_CURSOR_LINE: [u8; 4] = [76, 255, 0, 255];
pub const MINIMAP_VIEWPORT_FRAME: Color = CURSOR_LINE_NUMBER;
pub const MINIMAP_RGBA_VIEWPORT_FRAME: [u8; 4] = [76, 255, 0, 255];

// Viridis Syntax Theme
// Linearly interpolated across 20 categories
pub const SYNTAX_KEYWORD: Color = Color::Rgb(68, 1, 84);
pub const SYNTAX_STRING: Color = Color::Rgb(72, 22, 104);
pub const SYNTAX_COMMENT: Color = Color::Rgb(72, 40, 120);
pub const SYNTAX_TYPE: Color = Color::Rgb(68, 57, 131);
pub const SYNTAX_FUNCTION: Color = Color::Rgb(61, 74, 139);
pub const SYNTAX_NUMBER: Color = Color::Rgb(53, 89, 142);
pub const SYNTAX_PUNCTUATION: Color = Color::Rgb(46, 104, 142);
pub const SYNTAX_VARIABLE: Color = Color::Rgb(39, 117, 142);
pub const SYNTAX_CONSTANT: Color = Color::Rgb(33, 131, 141);
pub const SYNTAX_MACRO: Color = Color::Rgb(28, 144, 141);
pub const SYNTAX_MODULE: Color = Color::Rgb(31, 157, 137);
pub const SYNTAX_LIFETIME: Color = Color::Rgb(44, 171, 127);
pub const SYNTAX_ATTRIBUTE: Color = Color::Rgb(68, 184, 111);
pub const SYNTAX_OPERATOR: Color = Color::Rgb(98, 196, 92);
pub const SYNTAX_SELF_KEYWORD: Color = Color::Rgb(134, 207, 73);
pub const SYNTAX_BUILTIN_TYPE: Color = Color::Rgb(172, 217, 51);
pub const SYNTAX_MUTABLE_VARIABLE: Color = Color::Rgb(210, 225, 34);
pub const SYNTAX_METHOD: Color = Color::Rgb(253, 231, 37);
pub const SYNTAX_CRATE: Color = Color::Rgb(254, 239, 93);
pub const SYNTAX_WHITESPACE: Color = Color::Rgb(76, 86, 106); // Keep Nord-ish for contrast
