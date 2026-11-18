use std::cmp::Reverse;
use std::env;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::app::{Activity, App, ServiceStatus, SubmissionStatus};
use crate::markup::{ACCENT, MUTED};
use crate::text::{shorten, single_line};

const SETTING: &str = "KUKU_TUI_STATUSLINE";
const SEPARATOR: &str = " · ";
const COMPACT_CONTEXT_WIDTH: usize = 12;
const DEFAULT_FIELDS: [Field; 5] = [
    Field::Connection,
    Field::Execution,
    Field::Input,
    Field::Workspace,
    Field::Model,
];

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct StatusLine(Vec<Field>);

impl StatusLine {
    pub(crate) fn from_env() -> Result<Self, String> {
        match env::var(SETTING) {
            Ok(value) => Self::parse(&value),
            Err(env::VarError::NotPresent) => Ok(Self::default()),
            Err(env::VarError::NotUnicode(_)) => Err(format!(
                "{SETTING} must be Unicode. Using the default status line."
            )),
        }
    }

    pub(crate) fn render_line(&self, app: &App, width: usize) -> Option<Line<'static>> {
        let segments = self
            .0
            .iter()
            .filter_map(|field| field.segment(app))
            .collect();
        render_segments(segments, width)
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value.trim() {
            "" => Ok(Self::default()),
            "none" => Ok(Self(Vec::new())),
            value => {
                let mut fields = Vec::new();
                for name in value.split(',').map(str::trim) {
                    let field = DEFAULT_FIELDS
                        .into_iter()
                        .find(|field| field.name() == name)
                        .ok_or_else(|| {
                        let name = shorten(&single_line(name), 24);
                        let valid = DEFAULT_FIELDS.map(Field::name).join(",");
                        format!(
                            "{SETTING} has an unknown field {name:?}. Use {valid} or none. Using the default status line."
                        )
                    })?;
                    if fields.contains(&field) {
                        return Err(format!(
                            "{SETTING} repeats {name}. Using the default status line."
                        ));
                    }
                    fields.push(field);
                }
                Ok(Self(fields))
            }
        }
    }
}

impl Default for StatusLine {
    fn default() -> Self {
        Self(DEFAULT_FIELDS.into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Connection,
    Execution,
    Input,
    Workspace,
    Model,
}

impl Field {
    fn name(self) -> &'static str {
        match self {
            Self::Connection => "connection",
            Self::Execution => "execution",
            Self::Input => "input",
            Self::Workspace => "workspace",
            Self::Model => "model",
        }
    }

    fn segment(self, app: &App) -> Option<Segment> {
        match self {
            Self::Connection => Some(connection(app)),
            Self::Execution => execution(app),
            Self::Input => input(app),
            Self::Workspace => {
                let path = single_line(&app.workspace.to_string_lossy());
                (!path.is_empty()).then(|| Segment::context(path))
            }
            Self::Model => Some(Segment::context(
                app.pane()
                    .effective_model_label()
                    .map(|label| single_line(&label))
                    .unwrap_or_else(|| {
                        if app.pane().model_problem.is_some() {
                            "Model unavailable".into()
                        } else {
                            "Model pending".into()
                        }
                    }),
            )),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Priority {
    Context,
    Connected,
    Ready,
    Input,
    Active,
    Submission,
    Attention,
    ConnectionProblem,
}

enum SegmentText {
    Labels(Vec<String>),
    Context(String),
}

struct Segment {
    text: SegmentText,
    style: Style,
    priority: Priority,
}

impl Segment {
    fn labels(labels: Vec<String>, color: Color, priority: Priority) -> Self {
        Self {
            text: SegmentText::Labels(labels),
            style: Style::default().fg(color).add_modifier(Modifier::BOLD),
            priority,
        }
    }

    fn context(text: String) -> Self {
        Self {
            text: SegmentText::Context(text),
            style: Style::default(),
            priority: Priority::Context,
        }
    }

    fn compact_width(&self) -> usize {
        match &self.text {
            SegmentText::Labels(labels) => labels[0].width(),
            SegmentText::Context(text) => text.width().min(COMPACT_CONTEXT_WIDTH),
        }
    }

    fn fit(&self, width: usize) -> String {
        match &self.text {
            SegmentText::Labels(labels) => labels
                .iter()
                .rev()
                .find(|label| label.width() <= width)
                .cloned()
                .unwrap_or_else(|| shorten(&labels[0], width)),
            SegmentText::Context(text) => shorten_middle(text, width),
        }
    }
}

fn render_segments(segments: Vec<Segment>, width: usize) -> Option<Line<'static>> {
    let mut by_priority: Vec<_> = (0..segments.len()).collect();
    by_priority.sort_by_key(|index| Reverse(segments[*index].priority));
    let mut reserved = vec![0; segments.len()];
    let mut remaining = width;
    for &index in &by_priority {
        let separator = if remaining == width {
            0
        } else {
            SEPARATOR.width()
        };
        let compact = segments[index].compact_width();
        if compact + separator <= remaining || separator == 0 {
            reserved[index] = compact.min(remaining);
            remaining -= reserved[index] + separator;
        }
    }

    let mut spans = vec![None; segments.len()];
    for index in by_priority {
        if reserved[index] > 0 {
            let available = reserved[index] + remaining;
            let text = segments[index].fit(available);
            remaining = available - text.width();
            spans[index] = Some(Span::styled(text, segments[index].style));
        }
    }
    let mut spans = spans.into_iter().flatten();
    let mut line = vec![spans.next()?];
    for span in spans {
        line.push(Span::styled(SEPARATOR, Style::default().fg(MUTED)));
        line.push(span);
    }
    Some(Line::from(line))
}

fn connection(app: &App) -> Segment {
    let status = app.service_status();
    let (color, priority) = match status {
        ServiceStatus::Connected => (ACCENT, Priority::Connected),
        ServiceStatus::Connecting | ServiceStatus::Reconnecting | ServiceStatus::Restarting => {
            (Color::Yellow, Priority::ConnectionProblem)
        }
        ServiceStatus::Offline | ServiceStatus::UpdatesUnavailable => {
            (Color::Red, Priority::ConnectionProblem)
        }
    };
    let kind = if app.remote { "Remote" } else { "Local" };
    let location = format!("{} {kind}", status.label());
    let mut labels = vec![status.label().to_owned()];
    if status == ServiceStatus::UpdatesUnavailable {
        labels.insert(0, "No updates".into());
    }
    labels.push(location);
    if let Some(endpoint) = &app.endpoint {
        labels.push(format!(
            "{} {kind} {}",
            status.label(),
            single_line(endpoint)
        ));
    }
    Segment::labels(labels, color, priority)
}

fn execution(app: &App) -> Option<Segment> {
    let pane = app.pane();
    let activity = pane.activity();
    let connected = app.service_status() == ServiceStatus::Connected;
    if !connected && pane.snapshot.is_none() && activity == Activity::Ready {
        return None;
    }
    let (color, priority) = match activity {
        Activity::Ready | Activity::Finished => (ACCENT, Priority::Ready),
        Activity::Opening => (ACCENT, Priority::Active),
        Activity::Running
        | Activity::WaitingForModel
        | Activity::Streaming
        | Activity::RunningTools => (Color::Green, Priority::Active),
        Activity::Stopped => (Color::Yellow, Priority::Active),
        Activity::Waiting | Activity::Stopping | Activity::Configuration => {
            (Color::Yellow, Priority::Attention)
        }
        Activity::Interrupted | Activity::Failed => (Color::Red, Priority::Attention),
    };
    let label = if connected || pane.snapshot.is_none() || activity == Activity::Opening {
        activity.label().into()
    } else {
        format!("Last seen {}", activity.label())
    };
    Some(Segment::labels(vec![label], color, priority))
}

fn input(app: &App) -> Option<Segment> {
    let input = app.input_status();
    let submission = input
        .submission
        .filter(|status| *status != SubmissionStatus::Queued || input.queued == 0);
    let (color, priority) = match submission {
        Some(
            SubmissionStatus::ReceiptMissing
            | SubmissionStatus::ReceiptUnconfirmed
            | SubmissionStatus::Rejected,
        ) => (Color::Yellow, Priority::Attention),
        Some(
            SubmissionStatus::Saving
            | SubmissionStatus::AwaitingReceipt
            | SubmissionStatus::CheckingReceipt
            | SubmissionStatus::Discarding,
        ) => (ACCENT, Priority::Submission),
        Some(
            SubmissionStatus::Accepted
            | SubmissionStatus::Queued
            | SubmissionStatus::Withdrawn
            | SubmissionStatus::Confirmed,
        )
        | None => (ACCENT, Priority::Input),
    };
    let mut facts = Vec::new();
    if let Some(submission) = submission.filter(|_| priority != Priority::Input) {
        facts.push(submission.label().to_owned());
    }
    if input.queued > 0 {
        facts.push(format!("{} queued", input.queued));
    }
    if input.draft {
        facts.push("Draft".into());
    }
    if let Some(submission) = submission.filter(|_| priority == Priority::Input) {
        facts.push(submission.label().to_owned());
    }
    if facts.is_empty() {
        return None;
    }
    let labels = (1..=facts.len())
        .map(|length| facts[..length].join(", "))
        .collect();
    Some(Segment::labels(labels, color, priority))
}

fn shorten_middle(text: &str, width: usize) -> String {
    if text.width() <= width || width < 8 {
        return shorten(text, width);
    }
    let mut used = 0;
    let mut tail = text.len();
    for (offset, grapheme) in text.grapheme_indices(true).rev() {
        let cells = grapheme.width();
        if used + cells > width / 2 {
            break;
        }
        used += cells;
        tail = offset;
    }
    format!("{}{}", shorten(&text[..tail], width - used), &text[tail..])
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use kuku_client::{Error, HistoryPage, Snapshot, Timing};
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    use ratatui::Terminal;
    use serde_json::{json, Value};

    use super::{render_segments, Field, Priority, Segment, StatusLine, SEPARATOR, SETTING};
    use crate::app::App;
    use crate::jobs::{Job, Update};
    use crate::ui;

    fn connected_app() -> App {
        let workspace = PathBuf::from("/workspace");
        let mut app = App::new(workspace.clone(), None);
        app.update(Update::Connected {
            workspace,
            endpoint: "http://127.0.0.1:6789".into(),
        });
        app
    }

    fn load_agent(app: &mut App, status: &str, queued: usize, question: Value) {
        app.select(app.active.clone(), true, "Example".into());
        let selection = app.selection();
        let snapshot: Snapshot = serde_json::from_value(json!({
            "session_id": app.active.session,
            "workspace": app.workspace,
            "cursor": 50,
            "pending": [],
            "agents": [],
            "agent": {
                "address": app.active.agent, "config": {}, "status": status,
                "pending_inputs": queued, "execution_id": 49, "question": question,
                "usage": {"model_requests": 0, "unreported_requests": 0},
            },
        }))
        .unwrap();
        app.update(Update::Loaded(
            selection.clone(),
            Box::new(snapshot),
            HistoryPage {
                entries: Vec::new(),
                next_before: None,
            },
        ));
        app.update(Update::WatchConnected(selection));
    }

    fn screen(app: &mut App, statusline: &StatusLine, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| ui::render(frame, app, statusline))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .chunks(usize::from(width))
            .map(|row| row.iter().map(|cell| cell.symbol()).collect())
            .collect()
    }

    #[test]
    fn status_line_setting_orders_omits_and_hides_fields() {
        assert_eq!(
            StatusLine(vec![Field::Model, Field::Workspace, Field::Execution]),
            StatusLine::parse(" model, workspace,execution ").unwrap()
        );
        assert_eq!(StatusLine::default(), StatusLine::parse("").unwrap());
        assert_eq!(StatusLine(Vec::new()), StatusLine::parse("none").unwrap());
    }

    #[test]
    fn status_line_setting_reports_duplicates_and_unknown_fields() {
        let duplicate = StatusLine::parse("workspace,workspace").unwrap_err();
        assert!(duplicate.contains(SETTING));
        assert!(duplicate.contains("repeats workspace"));
        let unknown = StatusLine::parse("model,network").unwrap_err();
        assert!(unknown.contains("unknown field \"network\""));
    }

    #[test]
    fn short_status_line_fits_complete_graphemes_and_keeps_urgent_segments() {
        let line = render_segments(
            vec![
                Segment::context("/dir/开发👩‍💻/最后".into()),
                Segment::labels(vec!["Waiting".into()], Color::Yellow, Priority::Attention),
                Segment::labels(
                    vec!["Offline".into()],
                    Color::Red,
                    Priority::ConnectionProblem,
                ),
            ],
            17,
        )
        .unwrap();
        assert_eq!(
            vec!["Waiting", SEPARATOR, "Offline"],
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<Vec<_>>()
        );
        let line = render_segments(
            vec![Segment::context(
                "/dir/e\u{0301}e\u{0301}e\u{0301}/👩‍💻末".into(),
            )],
            10,
        )
        .unwrap();
        assert!(line.width() <= 10);
        assert!(line.spans[0].content.ends_with("/👩‍💻末"));
    }

    #[test]
    fn status_line_configuration_changes_visible_order_and_can_reclaim_its_row() {
        let mut app = connected_app();
        app.pane_mut().initial_config.tier = Some("fixture".into());
        let custom = StatusLine::parse("workspace,model,execution").unwrap();
        let rows = screen(&mut app, &custom, 92, 18);
        assert_eq!(
            "/workspace · Tier fixture · Ready",
            rows.last().unwrap().trim()
        );
        let hidden = StatusLine::parse("none").unwrap();
        let rows = screen(&mut app, &hidden, 92, 18);
        assert!(
            rows.last().unwrap().contains('╰'),
            "the composer's bottom border should move into the reclaimed row"
        );
    }

    #[test]
    fn question_stays_visible_when_execution_is_hidden_or_the_service_is_offline() {
        let mut app = connected_app();
        load_agent(
            &mut app,
            "waiting",
            0,
            json!({
                "id": "q-1", "agent": "main", "turn": 1, "tool_call_id": "tool-1",
                "text": "Which test target?",
            }),
        );
        let custom = StatusLine::parse("workspace").unwrap();
        let rows = screen(&mut app, &custom, 66, 14);
        assert!(rows.iter().any(|row| row.contains("F2 Reply")));
        assert!(rows.iter().any(|row| row.contains("Which test target?")));
        assert_eq!("/workspace", rows.last().unwrap().trim());

        app.reconnect();
        app.update(Update::ConnectFailed(Error::Options(
            "fixture offline".into(),
        )));
        let rows = screen(&mut app, &StatusLine::default(), 29, 10);
        assert_eq!("Offline · Last seen Waiting", rows.last().unwrap().trim());
        assert!(rows.iter().any(|row| row.contains("Which test target?")));
    }

    #[test]
    fn status_line_distinguishes_execution_unconfirmed_input_queue_and_draft() {
        let mut app = connected_app();
        load_agent(&mut app, "running", 2, Value::Null);
        app.pane_mut().draft.insert("First input");
        app.send(Timing::NextTurn);
        let command = app
            .effects
            .iter()
            .find_map(|job| match job {
                Job::Save(command) => Some(command.clone()),
                _ => None,
            })
            .unwrap();
        app.update(Update::Saved(command, Ok(())));
        app.pane_mut().draft.insert("Next unsent draft");
        let statusline = StatusLine::parse("execution,input").unwrap();
        let rows = screen(&mut app, &statusline, 70, 16);
        assert_eq!(
            "Running · Awaiting receipt, 2 queued, Draft",
            rows.last().unwrap().trim()
        );
    }
}
