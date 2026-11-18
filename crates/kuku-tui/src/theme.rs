//! Shared visual styles for the terminal workspace.
use ratatui::style::{Color, Modifier, Style};

pub(crate) const ACCENT: Color = Color::Rgb(70, 161, 163);
pub(crate) const MUTED: Color = Color::Rgb(133, 146, 157);
pub(crate) const SURFACE: Color = Color::Rgb(38, 48, 58);
pub(crate) const SURFACE_TEXT: Color = Color::Rgb(229, 236, 240);
pub(crate) const SUCCESS: Color = Color::Green;
pub(crate) const WARNING: Color = Color::Yellow;
pub(crate) const ERROR: Color = Color::Red;

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
pub(crate) const OUTPUT_WIDTH: u16 = 112;
pub(crate) const HORIZONTAL_GUTTER: u16 = 1;
