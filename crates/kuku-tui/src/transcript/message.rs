use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::markup::render_literal;
use crate::text::shorten;
use crate::theme::{emphasis, CardFrame, MUTED, SURFACE_TEXT, WARNING};

use super::window::TurnHeading;
use super::{render_rows, Row, MAX_VISUAL_ROWS};

pub(super) fn render_user(
    title: &str,
    text: &str,
    width: usize,
    turn: Option<TurnHeading>,
) -> Vec<Row> {
    let frame = CardFrame::timeline(width);
    let width = frame.content_width();
    let title = turn.map_or_else(
        || Line::styled(shorten(title, width), emphasis(SURFACE_TEXT)),
        |turn| user_turn_title(turn, width),
    );
    let mut rows = vec![Row::plain(title)];
    rows.extend(render_rows(
        render_literal(text, Style::new()),
        width,
        MAX_VISUAL_ROWS,
        0,
    ));
    Row::card(rows, &frame)
}

pub(super) fn render_assistant_source(text: &str, width: usize, interrupted: bool) -> Vec<Row> {
    let (title, short_title, color) = if interrupted {
        ("Interrupted · source", "Interrupted", WARNING)
    } else {
        ("Assistant · source", "Source", MUTED)
    };
    render_activity_record(
        if width >= title.width() + 2 {
            title
        } else {
            short_title
        },
        color,
        text,
        width,
    )
}

pub(super) fn render_markdown_record(
    title: &str,
    color: Color,
    text: &str,
    width: usize,
) -> Vec<Row> {
    let frame = CardFrame::timeline(width);
    let mut rows = vec![Row::caption(title, "", color, frame.content_width())];
    rows.extend(markdown_body(text, frame.content_width()));
    Row::card(rows, &frame)
}

pub(super) fn markdown_body(text: &str, width: usize) -> Vec<Row> {
    let (lines, links) = crate::markup::linked_markdown(text, width, MAX_VISUAL_ROWS);
    render_rows(lines, width, MAX_VISUAL_ROWS, 0)
        .map(|mut row| {
            row.links = links.resolve(&mut row.line);
            row
        })
        .collect()
}

pub(super) fn render_activity_record(
    title: &str,
    color: Color,
    text: &str,
    width: usize,
) -> Vec<Row> {
    let frame = CardFrame::timeline(width);
    Row::card(
        activity_rows(title, color, text, frame.content_width()),
        &frame,
    )
}

pub(super) fn activity_rows(title: &str, color: Color, text: &str, width: usize) -> Vec<Row> {
    let mut rows = vec![Row::caption(title, "", color, width)];
    if !text.is_empty() {
        rows.extend(render_rows(
            render_literal(text, Style::default()),
            width,
            MAX_VISUAL_ROWS,
            0,
        ));
    }
    rows
}

fn turn_label(turn: TurnHeading, width: usize) -> String {
    match turn {
        TurnHeading::Numbered(number) if width < 6 => format!("T{number}"),
        TurnHeading::Numbered(number) => format!("Turn {number}"),
        TurnHeading::Unnumbered => "Turn".to_owned(),
    }
}

fn user_turn_title(turn: TurnHeading, width: usize) -> Line<'static> {
    let label = turn_label(turn, width);
    let separator = if width >= "You  ·  ".width() + label.width() + 2 {
        "  ·  "
    } else {
        " · "
    };
    let mut title = Line::from(vec![
        Span::styled("You", emphasis(crate::theme::SURFACE_TEXT)),
        Span::styled(separator, Style::new().fg(MUTED)),
    ]);
    title.push_span(Span::styled(
        shorten(&label, width.saturating_sub(title.width())),
        emphasis(crate::theme::USAGE),
    ));
    title
}

pub(super) fn render_turn(turn: TurnHeading, width: usize) -> Line<'static> {
    let label = turn_label(turn, width);
    let mut line = Line::from(Span::styled(
        shorten(&label, width),
        emphasis(crate::theme::SURFACE_TEXT).bg(crate::theme::TURN_RULE),
    ));
    line.style = Style::new().bg(crate::theme::TURN_CARD);
    if width >= line.width() + 6 {
        let length = width - line.width() - 2;
        line.push_span(Span::styled(
            format!("  {}", "─".repeat(length)),
            Style::new().fg(crate::theme::TURN_RULE),
        ));
    }
    line
}
