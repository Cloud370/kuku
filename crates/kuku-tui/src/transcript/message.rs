use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::markup::{render_literal, wrap_bounded};
use crate::theme::{emphasis, surface, ACCENT, USER_LEFT_EDGE};

use super::{body_inset, render_rows, Row, MAX_VISUAL_ROWS};

pub(super) fn render_user(title: &str, text: &str, width: usize) -> Vec<Row> {
    let inset = body_inset(width);
    let surface = surface();
    let mut lines = vec![Line::styled(
        title.to_owned(),
        Style::new().add_modifier(Modifier::BOLD),
    )];
    lines.extend(render_literal(text, Style::new()));
    let mut rows: Vec<_> = wrap_bounded(
        lines,
        width.saturating_sub(usize::from(inset)),
        MAX_VISUAL_ROWS,
    )
    .into_iter()
    .map(|mut line| {
        line.style = surface;
        if inset > 0 {
            line.spans
                .insert(0, Span::styled(USER_LEFT_EDGE, Style::new().fg(ACCENT)));
        }
        Row { line, inset }
    })
    .collect();
    rows.push(Row::plain(Line::default()));
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
