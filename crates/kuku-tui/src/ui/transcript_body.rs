use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use crate::theme::{TURN_RULE, transcript_background};
use crate::transcript::TurnSurface;

pub(super) fn render(
    frame: &mut Frame<'_>,
    body: Rect,
    lines: Vec<Line<'static>>,
    turn: TurnSurface,
) {
    frame.render_widget(Paragraph::new(lines), body);
    frame
        .buffer_mut()
        .set_style(body, Style::new().bg(Color::Reset));
    for rows in turn.rows {
        let area = Rect::new(
            body.x,
            body.y + rows.start as u16,
            body.width,
            rows.len() as u16,
        );
        frame.buffer_mut().set_style(area, transcript_background());
    }
    if let Some(column) = turn.connector_column {
        for row in turn.connectors {
            frame.buffer_mut()[(body.x + column as u16, body.y + row as u16)]
                .set_symbol("│")
                .set_fg(TURN_RULE);
        }
    }
}
