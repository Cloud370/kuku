//! Shared visual styles for the terminal workspace.
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Padding;

pub(crate) const ACCENT: Color = Color::Rgb(70, 161, 163);
pub(crate) const MUTED: Color = Color::Rgb(133, 146, 157);
pub(crate) const SURFACE: Color = Color::Rgb(38, 48, 58);
pub(crate) const SURFACE_TEXT: Color = Color::Rgb(229, 236, 240);
pub(crate) const SUCCESS: Color = Color::Rgb(152, 202, 162);
pub(crate) const WARNING: Color = Color::Rgb(232, 191, 122);
pub(crate) const ERROR: Color = Color::Rgb(243, 145, 145);
pub(crate) const KUKU_MARK: &str = "⢎⡱";
pub(crate) const ACTIVITY_MINT: Color = Color::Rgb(146, 213, 202);

pub(crate) fn emphasis(color: Color) -> Style {
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

pub(crate) fn surface() -> Style {
    Style::default().fg(SURFACE_TEXT).bg(SURFACE)
}

pub(crate) fn selection() -> Style {
    Style::default().add_modifier(Modifier::REVERSED)
}

pub(crate) const TRANSCRIPT_INDENT: u16 = 2;
pub(crate) const MIN_TRANSCRIPT_INDENT_WIDTH: usize = 12;
pub(crate) const USER_LEFT_EDGE: &str = "│ ";
pub(crate) const MAX_DRAFT_ROWS: usize = 4;
pub(crate) const PICKER_WIDTH: u16 = 88;
pub(crate) const PICKER_HEIGHT: u16 = 25;
pub(crate) const SHORT_PROMPT_HEIGHT: u16 = 13;
pub(crate) const STATUS_EDITOR_HEIGHT: u16 = 19;
pub(crate) const OUTPUT_WIDTH: u16 = 112;
const PANEL: Color = Color::Rgb(28, 35, 44);
const FOCUS: Color = Color::Rgb(53, 73, 85);

pub(crate) fn panel() -> Style {
    Style::new().fg(SURFACE_TEXT).bg(PANEL)
}

pub(crate) fn backdrop() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

pub(crate) fn focused_row() -> Style {
    panel().bg(FOCUS)
}

pub(crate) fn horizontal_gutter(width: u16) -> u16 {
    match width {
        0..=23 => 0,
        24..=71 => 1,
        72..=109 => 2,
        _ => 3,
    }
}

pub(crate) fn composer_padding(area: Rect) -> Padding {
    if area.width < 4 {
        return Padding::ZERO;
    }
    let sides = horizontal_gutter(area.width).max(1);
    Padding::new(
        sides,
        sides,
        u16::from(area.height >= 26),
        u16::from(area.height >= 26),
    )
}

pub(crate) fn panel_padding(area: Rect) -> Padding {
    let sides = if area.width >= 60 {
        3
    } else {
        u16::from(area.width >= 22)
    };
    let vertical = u16::from(area.height >= 14);
    Padding::new(sides, sides, vertical, vertical)
}

pub(crate) const MODEL: Color = Color::Rgb(175, 165, 223);
pub(crate) const EXECUTION: Color = Color::Rgb(103, 184, 178);
pub(crate) const USAGE: Color = Color::Rgb(153, 181, 214);

pub(crate) const CODE_BACKGROUND: Color = Color::Rgb(29, 38, 46);
pub(crate) const CODE_TEXT: Color = Color::Rgb(224, 232, 239);
pub(crate) const CODE_COMMENT: Color = Color::Rgb(151, 167, 178);
pub(crate) const CODE_KEYWORD: Color = Color::Rgb(204, 168, 229);
pub(crate) const CODE_STRING: Color = Color::Rgb(155, 204, 161);
pub(crate) const CODE_LITERAL: Color = Color::Rgb(234, 188, 126);
pub(crate) const CODE_NAME: Color = Color::Rgb(135, 200, 217);

pub(crate) fn code_surface() -> Style {
    Style::new().fg(CODE_TEXT).bg(CODE_BACKGROUND)
}
