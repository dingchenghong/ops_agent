//! 配色与常用样式

use ratatui::style::{Color, Modifier, Style};

pub const ACCENT: Color = Color::Rgb(98, 180, 255);
pub const ACCENT_SOFT: Color = Color::Rgb(60, 120, 170);
pub const USER: Color = Color::Rgb(130, 220, 160);
pub const ASSISTANT: Color = Color::Rgb(120, 200, 255);
pub const TOOL: Color = Color::Rgb(232, 190, 100);
pub const OK: Color = Color::Rgb(120, 210, 140);
pub const ERR: Color = Color::Rgb(240, 120, 120);
pub const WARN: Color = Color::Rgb(240, 190, 90);
pub const DIM: Color = Color::Rgb(128, 134, 146);
pub const TEXT: Color = Color::Rgb(216, 220, 228);
pub const CODE_BG: Color = Color::Rgb(32, 37, 46);

pub fn base() -> Style {
    Style::new().fg(TEXT)
}

pub fn dim() -> Style {
    Style::new().fg(DIM)
}

pub fn strong(color: Color) -> Style {
    Style::new().fg(color).add_modifier(Modifier::BOLD)
}

pub fn block_border(focused: bool) -> Style {
    if focused {
        Style::new().fg(ACCENT)
    } else {
        Style::new().fg(ACCENT_SOFT)
    }
}
