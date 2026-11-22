//! Shared visual styles for the terminal workspace.
mod frame;
pub(crate) use frame::CardFrame;

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Padding;

pub(crate) const ACCENT: Color = Color::Rgb(70, 161, 163);
pub(crate) const MUTED: Color = Color::Rgb(133, 146, 157);
pub(crate) const SURFACE: Color = Color::Rgb(38, 48, 58);
pub(crate) const SUBAGENT_SURFACE: Color = Color::Rgb(30, 37, 50);
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

pub(crate) fn composer_surface(subagent: bool) -> Style {
    surface().bg(if subagent { SUBAGENT_SURFACE } else { SURFACE })
}

pub(crate) fn selection() -> Style {
    Style::default().add_modifier(Modifier::REVERSED)
}

pub(crate) const TRANSCRIPT_INDENT: u16 = 2;
pub(crate) const MIN_TRANSCRIPT_INDENT_WIDTH: usize = 12;

pub(crate) const MAX_DRAFT_ROWS: usize = 4;
pub(crate) const PICKER_WIDTH: u16 = 88;
pub(crate) const PICKER_HEIGHT: u16 = 25;
pub(crate) const SHORT_PROMPT_HEIGHT: u16 = 13;
pub(crate) const STATUS_EDITOR_HEIGHT: u16 = 19;
const SESSION_WIDTH: u16 = 120;
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

pub(crate) fn session_area(outer: Rect) -> Rect {
    let gutter = horizontal_gutter(outer.width);
    centered_width(
        outer,
        outer.width.saturating_sub(2 * gutter).min(SESSION_WIDTH),
    )
}

pub(crate) fn centered_width(outer: Rect, width: u16) -> Rect {
    let width = width.min(outer.width);
    Rect::new(
        outer.x + (outer.width - width) / 2,
        outer.y,
        width,
        outer.height,
    )
}

pub(crate) fn card_padding(width: usize) -> Padding {
    let sides = match width {
        0..=11 => 0,
        12..=31 => 1,
        32..=79 => 2,
        _ => 3,
    };
    compact_padding(sides, width >= 24)
}

pub(crate) fn composer_padding(area: Rect) -> Padding {
    if area.width < 4 {
        return Padding::ZERO;
    }
    compact_padding(horizontal_gutter(area.width).max(1), area.height >= 26)
}

pub(crate) fn panel_padding(area: Rect) -> Padding {
    let sides = if area.width >= 60 {
        3
    } else {
        u16::from(area.width >= 22)
    };
    compact_padding(sides, area.height >= 14)
}

pub(crate) fn compact_padding(sides: u16, vertical: bool) -> Padding {
    let sides = (2 * sides + 1) / 3;
    let vertical = u16::from(vertical);
    Padding::new(sides, sides, vertical, vertical)
}

pub(crate) const MODEL: Color = Color::Rgb(175, 165, 223);
pub(crate) const EXECUTION: Color = Color::Rgb(103, 184, 178);
pub(crate) const ACTIVITY_EXCERPT_CELLS: usize = 66;
pub(crate) const THINKING_GLINT: Color = Color::Rgb(161, 166, 188);
pub(crate) const TURN_RULE: Color = Color::Rgb(70, 85, 100);
pub(crate) const TURN_CARD: Color = Color::Rgb(34, 45, 57);
pub(crate) const USAGE: Color = Color::Rgb(153, 181, 214);

#[derive(Clone, Copy)]
pub(crate) enum TranscriptTone {
    Text,
    Label,
    Field,
    Type,
    Value,
    Success,
    Warning,
    Error,
}

impl TranscriptTone {
    pub(crate) fn color(self) -> Color {
        match self {
            Self::Text => SURFACE_TEXT,
            Self::Label => MUTED,
            Self::Field => CODE_NAME,
            Self::Type => MODEL,
            Self::Value => USAGE,
            Self::Success => SUCCESS,
            Self::Warning => WARNING,
            Self::Error => ERROR,
        }
    }

    pub(crate) fn style(self) -> Style {
        Style::new().fg(self.color())
    }

    pub(crate) fn emphasized(self) -> Style {
        self.style().add_modifier(Modifier::BOLD)
    }
}

pub(crate) fn transcript_background() -> Style {
    Style::new().bg(SURFACE)
}

pub(crate) fn tool_operation_color(name: &str) -> Color {
    match name {
        "write_file" | "edit_file" | "save_value" | "create_output" | "append_output"
        | "finish_output" => USAGE,
        "remove_file" => ERROR,
        "run_js" | "run_command" => MODEL,
        _ => ACCENT,
    }
}

pub(crate) fn tool_status(color: Color) -> Style {
    Style::new().fg(color)
}

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
