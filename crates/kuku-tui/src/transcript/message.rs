use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::markup::{render_literal, wrap_bounded};
use crate::text::shorten;
use crate::theme::{card_padding, emphasis, surface, ACCENT, MUTED, WARNING};

use super::window::TurnHeading;
use super::{render_rows, Row, MAX_VISUAL_ROWS};

pub(super) fn render_user(title: &str, text: &str, width: usize) -> Vec<Row> {
    let padding = card_padding(width);
    let edge = user_edge(width);
    let inset = edge.width() as u16;
    let style = surface();
    let content_width = width.saturating_sub(usize::from(inset + padding.right));
    let decorate = |mut line: Line<'static>| {
        line.style = style;
        if inset > 0 {
            line.spans
                .insert(0, Span::styled(edge, Style::new().fg(ACCENT)));
        }
        Row {
            line,
            inset,
            links: Vec::new(),
        }
    };
    let spacer = || Row::padding(decorate(Line::default()).line, width);
    let mut rows = vec![decorate(Line::styled(
        shorten(title, content_width),
        Style::new().add_modifier(Modifier::BOLD),
    ))];
    if padding.top > 0 {
        rows.push(spacer());
    }
    rows.extend(
        wrap_bounded(
            render_literal(text, Style::new()),
            content_width,
            MAX_VISUAL_ROWS,
        )
        .into_iter()
        .map(decorate),
    );
    if padding.bottom > 0 {
        rows.push(spacer());
    }
    rows.push(Row::plain(Line::default()));
    rows
}

fn user_edge(width: usize) -> &'static str {
    match card_padding(width).left {
        0 => "",
        1 => "│ ",
        2 => "│  ",
        _ => "│   ",
    }
}

pub(super) fn render_assistant_source(text: &str, width: usize, interrupted: bool) -> Vec<Row> {
    let (title, short_title, color) = if interrupted {
        ("Interrupted · source", "Interrupted", WARNING)
    } else {
        ("Assistant · source", "Source", MUTED)
    };
    let inset = super::body_inset(width);
    render_record(
        if width >= title.width() {
            title
        } else {
            short_title
        },
        color,
        render_literal(text, Style::default()),
        width,
        inset,
    )
}

pub(super) fn render_markdown_record(
    title: &str,
    color: Color,
    text: &str,
    width: usize,
) -> Vec<Row> {
    let (lines, links) = crate::markup::linked_markdown(text, width, MAX_VISUAL_ROWS);
    let mut rows = render_record(title, color, lines, width, 0);
    for row in &mut rows {
        row.links = links.resolve(&mut row.line);
    }
    rows
}

pub(super) fn render_record(
    title: &str,
    color: Color,
    mut lines: Vec<Line<'static>>,
    width: usize,
    inset: u16,
) -> Vec<Row> {
    let title = Line::styled(title.to_owned(), emphasis(color));
    if lines.last().is_some_and(|line| !line.spans.is_empty()) {
        lines.push(Line::default());
    }
    let mut rows: Vec<_> = render_rows(vec![title], width, MAX_VISUAL_ROWS, 0).collect();
    rows.extend(render_rows(lines, width, MAX_VISUAL_ROWS, inset));
    rows
}

pub(super) fn padded_panel(
    width: usize,
    style: Style,
    render: impl FnOnce(usize) -> Vec<Row>,
) -> Vec<Row> {
    let padding = card_padding(width);
    let inner = width.saturating_sub(usize::from(padding.left + padding.right));
    let mut rows = render(inner);
    let end = rows.len() - usize::from(rows.last().is_some_and(|row| row.line.spans.is_empty()));
    if end == 0 {
        return rows;
    }
    for row in &mut rows[..end] {
        row.line.style = style.patch(row.line.style);
        if padding.left > 0 {
            row.line.spans.insert(
                0,
                Span::styled(" ".repeat(usize::from(padding.left)), style),
            );
            row.inset += padding.left;
        }
    }
    if padding.top > 0 {
        let spacer = Row::padding(Line::styled(" ", style), width);
        rows.insert(end, spacer.clone());
        rows.insert(0, spacer);
    }
    rows
}

pub(super) fn paint_card(rows: &mut [Row], width: usize, color: Color) {
    let background = Style::new().bg(color);
    for row in rows {
        let padding = width.saturating_sub(row.line.width());
        for span in &mut row.line.spans {
            span.style = background.patch(span.style);
        }
        if padding > 0 {
            row.line
                .push_span(Span::styled(" ".repeat(padding), background));
        }
    }
}

pub(super) fn render_turn(turn: TurnHeading, width: usize, user: bool) -> Line<'static> {
    let label = match turn {
        TurnHeading::Numbered(number) if width < 6 => format!("T{number}"),
        TurnHeading::Numbered(number) => format!("Turn {number}"),
        TurnHeading::Unnumbered => "Turn".to_owned(),
    };
    if user {
        let edge = user_edge(width);
        let separator = if width >= edge.width() + "You   /   ".len() + label.width() {
            "   /   "
        } else {
            " / "
        };
        let mut header = Line::from(vec![
            Span::styled(edge, emphasis(ACCENT)),
            Span::styled("You", emphasis(crate::theme::SURFACE_TEXT)),
            Span::styled(separator, Style::new().fg(crate::theme::MUTED)),
        ]);
        header.push_span(Span::styled(
            shorten(&label, width.saturating_sub(header.width())),
            emphasis(crate::theme::USAGE),
        ));
        header.style = surface();
        return header;
    }
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

pub(super) fn turn_gap(width: usize) -> Line<'static> {
    if width < 24 {
        return Line::default();
    }
    let inset = usize::from(super::body_inset(width));
    let rule = width.saturating_sub(inset).min(40);
    Line::styled(
        format!("{}{}", " ".repeat(inset), "─".repeat(rule)),
        Style::new().fg(crate::theme::TURN_RULE),
    )
}
