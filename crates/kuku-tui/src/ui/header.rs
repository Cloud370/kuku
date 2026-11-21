use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, ServiceStatus, StartupSkills};
use crate::contextual::ContextControl;
use crate::keys::Shortcut;
use crate::markup::{render_literal, wrap};
use crate::text::{shorten, shorten_middle, single_line, single_line_path};
use crate::theme::{compact_padding, emphasis, surface, ACCENT, KUKU_MARK, MODEL, MUTED, WARNING};

const BRAND: &str = "kuku";
const SEPARATOR: &str = " · ";
const WELCOME_WIDTH: u16 = 56;
const WELCOME_HEIGHT: u16 = 9;
const WELCOME_MAX_WIDTH: u16 = 86;
const WELCOME_LABEL_WIDTH: usize = 11;
const WELCOME_SKILL_ROWS: usize = 2;
const SESSION_PREFIX_WIDTH: usize = 39;
const TITLE_RESERVE: usize = 9;

pub(super) fn render_session(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &mut App,
    following: bool,
    tool_hint: Option<&str>,
) -> Option<Rect> {
    let width = usize::from(area.width);
    let messages_label = if width >= 72 {
        " Messages "
    } else if width >= 28 {
        " Msg "
    } else {
        ""
    };
    let messages_width = messages_label.width();
    let messages_area = (!messages_label.is_empty()).then(|| {
        Rect::new(
            area.x + area.width - messages_width as u16,
            area.y,
            messages_width as u16,
            1,
        )
    });
    let mut right = messages_area.map_or(area.x.saturating_add(area.width), |messages| {
        messages.x.saturating_sub(1)
    });
    let controls = app
        .contextual_controls()
        .into_iter()
        .find(|control| matches!(control, ContextControl::PreviousAgent { .. }));
    for control in controls.iter().rev() {
        let chip_width = u16::try_from(super::contextual::label(control).width())
            .unwrap_or(u16::MAX)
            .min(right.saturating_sub(area.x));
        if chip_width == 0 {
            continue;
        }
        right = right.saturating_sub(chip_width);
        super::contextual::render_chip(
            frame,
            app,
            control,
            Rect::new(right, area.y, chip_width, area.height),
        );
        right = right.saturating_sub(1);
    }
    let title_area = Rect {
        width: right.saturating_sub(area.x),
        ..area
    };
    let content_width = usize::from(title_area.width);
    let mut line = wordmark(content_width >= SESSION_PREFIX_WIDTH);
    let id: String = single_line(&app.active.session).chars().take(8).collect();
    let identity = if content_width >= SESSION_PREFIX_WIDTH {
        format!("Session {id}")
    } else {
        id
    };
    append(
        &mut line,
        &identity,
        content_width,
        Style::default().fg(MUTED),
    );
    let title = single_line(&app.pane().title);
    let room = content_width.saturating_sub(line.width() + TITLE_RESERVE);
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
    let title_width = content_width - hint.as_ref().map_or(0, |hint| hint.width() + 2);
    append(&mut line, &title, title_width, emphasis(Color::Reset));
    if let Some(hint) = hint {
        line.push_span(" ".repeat(content_width - line.width() - hint.width()));
        line.push_span(Span::styled(hint, Style::default().fg(MUTED)));
    }
    frame.render_widget(Paragraph::new(line), title_area);
    if let Some(messages_area) = messages_area {
        frame.render_widget(
            Paragraph::new(Span::styled(messages_label, emphasis(ACCENT))),
            messages_area,
        );
    }
    messages_area
}

pub(super) fn render_welcome(frame: &mut Frame<'_>, area: Rect, app: &App, show_problem: bool) {
    let card = area.width >= WELCOME_WIDTH && area.height >= WELCOME_HEIGHT;
    let block = if card {
        Block::new()
            .style(surface())
            .borders(Borders::LEFT)
            .border_style(Style::new().fg(ACCENT))
            .padding(compact_padding(2, true))
    } else {
        Block::new()
    };
    let area = Rect {
        y: area.y + u16::from(card),
        height: area.height - u16::from(card),
        width: area.width.min(WELCOME_MAX_WIDTH),
        ..area
    };
    let inner = block.inner(area);
    let width = usize::from(inner.width);
    let mut identity = wordmark(width >= 25);
    append(&mut identity, "New session", width, emphasis(Color::Reset));
    let mut lines = vec![identity];
    if let Some(notice) = empty_notice(app, width, show_problem).into_iter().next() {
        lines.push(notice);
    }
    let mut action_lines =
        welcome_actions(width, usize::from(inner.height).saturating_sub(lines.len()));
    let remaining = usize::from(inner.height).saturating_sub(lines.len() + action_lines.len());
    let action_gap = u16::from(card && remaining >= 4);
    let details_rows = remaining.saturating_sub(usize::from(action_gap) + usize::from(card));
    if details_rows >= 2 && width >= 22 {
        if card {
            lines.push(Line::default());
        }
        lines.extend(welcome_details(app, width, details_rows, card));
    }
    let body = Rect {
        height: lines.len().min(usize::from(inner.height)) as u16,
        ..inner
    };
    let panel = Rect {
        height: body.bottom().saturating_sub(area.y),
        ..area
    };
    frame.render_widget(block, panel);
    frame.render_widget(Paragraph::new(lines), body);
    let y = body.bottom() + action_gap;
    let actions = Rect {
        y,
        height: area.bottom().saturating_sub(y),
        ..inner
    };
    if card && action_lines.len() < usize::from(actions.height) {
        let shortcuts = welcome_shortcuts();
        if shortcuts.width() <= width {
            action_lines.push(shortcuts);
        }
    }
    frame.render_widget(Paragraph::new(action_lines), actions);
}

fn welcome_details(app: &App, width: usize, rows: usize, card: bool) -> Vec<Line<'static>> {
    let content_width = width.saturating_sub(WELCOME_LABEL_WIDTH);
    let workspace = single_line_path(&app.workspace);
    let mut details = vec![welcome_field(
        "Workspace",
        Line::raw(shorten_middle(&workspace, content_width)),
    )];
    let pane = app.pane();
    let model = pane
        .effective_model_identity()
        .map(|(model, connection, _)| format!("{connection}/{model}"))
        .or_else(|| pane.effective_model_label());
    if let Some(model) = model.filter(|_| rows >= 3) {
        details.push(welcome_field(
            "Model",
            Line::styled(
                shorten_middle(&single_line(&model), content_width),
                Style::new().fg(MODEL),
            ),
        ));
    }
    let separate_skills = card && rows.saturating_sub(details.len()) >= 3;
    if separate_skills {
        details.push(Line::default());
        let mut title = Line::styled("Skills", emphasis(ACCENT));
        if matches!(&app.startup_skills, StartupSkills::Ready(names) if !names.is_empty()) {
            title.push_span(Span::raw("   "));
            title.push_span(Span::styled("/skill:", emphasis(Color::Reset)));
            title.push_span(Span::styled(" Browse", Style::new().fg(MUTED)));
        }
        details.push(title);
    }
    let slots = rows.saturating_sub(details.len()).min(WELCOME_SKILL_ROWS);
    let names_width = if separate_skills {
        width
    } else {
        content_width
    };
    let skills = match &app.startup_skills {
        StartupSkills::Loading if app.available() || app.connecting => "Loading…".into(),
        StartupSkills::Loading | StartupSkills::Unavailable => "Unavailable".into(),
        StartupSkills::Ready(names) if names.is_empty() => "None found".into(),
        StartupSkills::Ready(names) => names
            .iter()
            .map(|name| single_line(name))
            .collect::<Vec<_>>()
            .join(SEPARATOR),
    };
    let mut skills = wrap(render_literal(&skills, Style::default()), names_width);
    if skills.len() > slots {
        skills.truncate(slots);
        if let Some(last) = skills.last_mut() {
            let suffix = if separate_skills { "" } else { " /skill:" };
            let text = shorten(
                &format!("{} …", last.to_string().trim_end()),
                names_width.saturating_sub(suffix.width()),
            );
            *last = Line::from(vec![
                Span::raw(text),
                Span::styled(suffix, Style::default().fg(MUTED)),
            ]);
        }
    }
    if separate_skills {
        details.extend(skills);
    } else {
        details.extend(
            skills
                .into_iter()
                .enumerate()
                .map(|(index, value)| welcome_field(if index == 0 { "Skills" } else { "" }, value)),
        );
    }
    details
}

fn welcome_field(label: &str, value: Line<'static>) -> Line<'static> {
    let label = Span::styled(
        format!("{label:<WELCOME_LABEL_WIDTH$}"),
        Style::default().fg(MUTED),
    );
    Line::from(
        std::iter::once(label)
            .chain(value.spans)
            .collect::<Vec<_>>(),
    )
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
            ServiceStatus::Connecting => ("Connecting to the kuku service…", MUTED),
            ServiceStatus::Reconnecting => ("Reconnecting to the kuku service…", MUTED),
            ServiceStatus::Restarting => ("Restarting the kuku service…", MUTED),
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
    if rows == 0 {
        return Vec::new();
    }
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
        vec![input]
    } else if rows <= 1 {
        vec![Line::styled(
            shorten("Enter to send · /resume", width),
            muted,
        )]
    } else {
        vec![input, Line::from(vec![command, caption])]
    }
}

fn welcome_shortcuts() -> Line<'static> {
    let muted = Style::new().fg(MUTED);
    let mut line = Line::default();
    for (key, action) in [
        ("/", "Actions"),
        (Shortcut::MODEL.label, "Model"),
        (Shortcut::CYCLE_THINKING.label, "Think"),
        (Shortcut::ACTIONS.label, "Help"),
    ] {
        if !line.spans.is_empty() {
            line.push_span(Span::styled(SEPARATOR, muted));
        }
        line.push_span(Span::styled(key, emphasis(Color::Reset)));
        line.push_span(Span::styled(format!(" {action}"), muted));
    }
    line
}

fn append(line: &mut Line<'static>, label: &str, width: usize, style: Style) {
    let available = width.saturating_sub(line.width() + SEPARATOR.width());
    if !label.is_empty() && available > 0 {
        line.push_span(Span::styled(SEPARATOR, Style::default().fg(MUTED)));
        line.push_span(Span::styled(shorten(label, available), style));
    }
}
