use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, ServiceStatus};
use crate::contextual::ContextControl;
use crate::keys::Shortcut;
use crate::markup::{render_literal, wrap};
use crate::text::{shorten, single_line};
use crate::theme::{emphasis, ACCENT, KUKU_MARK, MUTED, WARNING};

const BRAND: &str = "kuku";
const SEPARATOR: &str = " · ";
const WELCOME_WIDTH: u16 = 56;
const WELCOME_HEIGHT: u16 = 7;
const SESSION_PREFIX_WIDTH: usize = 39;
const TITLE_RESERVE: usize = 9;

pub(super) fn render_session(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &mut App,
    following: bool,
    tool_hint: Option<&str>,
) {
    let controls = app
        .contextual_controls()
        .into_iter()
        .find(|control| matches!(control, ContextControl::PreviousAgent { .. }));
    let chips = controls.into_iter().collect::<Vec<_>>();
    let mut right = area.x.saturating_add(area.width);
    for control in chips.iter().rev() {
        let width = u16::try_from(super::contextual::label(control).width())
            .unwrap_or(u16::MAX)
            .min(right.saturating_sub(area.x));
        if width == 0 {
            continue;
        }
        right = right.saturating_sub(width);
        super::contextual::render_chip(
            frame,
            app,
            control,
            Rect::new(right, area.y, width, area.height),
        );
        right = right.saturating_sub(1);
    }
    let title_area = Rect {
        width: right.saturating_sub(area.x),
        ..area
    };
    let width = usize::from(title_area.width);
    let mut line = wordmark(width >= SESSION_PREFIX_WIDTH);
    let id: String = single_line(&app.active.session).chars().take(8).collect();
    let identity = if width >= SESSION_PREFIX_WIDTH {
        format!("Session {id}")
    } else {
        id
    };
    append(&mut line, &identity, width, Style::default().fg(MUTED));
    let title = single_line(&app.pane().title);
    let room = width.saturating_sub(line.width() + TITLE_RESERVE);
    let primary_hint = following.then_some(tool_hint).flatten();
    let navigation = if app
        .pane()
        .snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.agents.len() > 1)
    {
        "Alt+←/→ Switch · /agents"
    } else {
        "/agents"
    };
    let hint = match primary_hint {
        Some(primary) => {
            let combined = format!("{primary}{SEPARATOR}{navigation}");
            if combined.width() <= room {
                combined
            } else {
                primary.to_owned()
            }
        }
        None => navigation.to_owned(),
    };
    let hint = (hint.width() <= room).then_some(hint);
    let title_width = width - hint.as_ref().map_or(0, |hint| hint.width() + 2);
    append(&mut line, &title, title_width, emphasis(Color::Reset));
    if let Some(hint) = hint {
        line.push_span(" ".repeat(width - line.width() - hint.width()));
        line.push_span(Span::styled(hint, Style::default().fg(MUTED)));
    }
    frame.render_widget(Paragraph::new(line), title_area);
}

pub(super) fn render_welcome(frame: &mut Frame<'_>, area: Rect, app: &App, show_problem: bool) {
    let spacious = area.width >= WELCOME_WIDTH && area.height >= WELCOME_HEIGHT;
    let (body, mut lines) = if spacious {
        let brand = wordmark(true);
        let text_column = (brand.width() - BRAND.width()) as u16;
        let heading = Rect::new(area.x, area.y + 1, area.width, 1);
        frame.render_widget(Paragraph::new(brand), heading);
        let body = Rect::new(
            area.x + text_column,
            area.y + 2,
            area.width - text_column,
            area.height - 2,
        );
        (
            body,
            vec![
                Line::styled("New session", emphasis(Color::Reset)),
                Line::default(),
            ],
        )
    } else {
        let mut identity = wordmark(area.width >= 25);
        append(
            &mut identity,
            "New session",
            usize::from(area.width),
            emphasis(Color::Reset),
        );
        (area, vec![identity])
    };
    let width = usize::from(body.width);
    if let Some(line) = empty_notice(app, width, show_problem).into_iter().next() {
        lines.push(line);
    }
    let remaining = usize::from(body.height).saturating_sub(lines.len());
    lines.extend(welcome_actions(width, remaining));
    frame.render_widget(Paragraph::new(lines), body);
}

pub(super) fn empty_notice(app: &App, width: usize, show_problem: bool) -> Vec<Line<'static>> {
    let (text, color) = if let Some(problem) = app.service_problem() {
        if !show_problem {
            return Vec::new();
        }
        (problem, WARNING)
    } else if app.pane().opening {
        ("Opening the conversation…", MUTED)
    } else {
        match app.service_status() {
            ServiceStatus::Connecting => ("Connecting to the Session service…", MUTED),
            ServiceStatus::Reconnecting => ("Reconnecting to the Session service…", MUTED),
            ServiceStatus::Restarting => ("Restarting the Session service…", MUTED),
            ServiceStatus::Connected
            | ServiceStatus::Offline
            | ServiceStatus::UpdatesUnavailable => return Vec::new(),
        }
    };
    wrap(render_literal(text, Style::default().fg(color)), width)
}

fn wordmark(mark: bool) -> Line<'static> {
    let mut line = Line::default();
    if mark {
        line.push_span(Span::styled(KUKU_MARK, Style::default().fg(ACCENT)));
        line.push_span("  ");
    }
    line.push_span(Span::styled(BRAND, emphasis(Color::Reset)));
    line
}

fn welcome_actions(width: usize, rows: usize) -> Vec<Line<'static>> {
    let muted = Style::default().fg(MUTED);
    let action = Style::default().fg(Color::Reset);
    let send = if width >= 31 {
        " sends your first prompt"
    } else {
        " to send"
    };
    let resume = if width >= 31 {
        " opens a saved session"
    } else {
        ""
    };
    let mut input = Line::from(vec![
        Span::styled(Shortcut::ENTER.label, action),
        Span::styled(send, muted),
    ]);
    let command = Span::styled("/resume", action);
    let caption = Span::styled(resume, muted);
    if input.width() + SEPARATOR.width() + command.width() + caption.width() <= width {
        input.push_span(Span::styled(SEPARATOR, muted));
        input.push_span(command);
        input.push_span(caption);
        return vec![input];
    }
    if rows <= 1 {
        return vec![Line::styled(
            shorten("Enter to send · /resume", width),
            muted,
        )];
    }
    vec![input, Line::from(vec![command, caption])]
}

fn append(line: &mut Line<'static>, label: &str, width: usize, style: Style) {
    let available = width.saturating_sub(line.width() + SEPARATOR.width());
    if !label.is_empty() && available > 0 {
        line.push_span(Span::styled(SEPARATOR, Style::default().fg(MUTED)));
        line.push_span(Span::styled(shorten(label, available), style));
    }
}
