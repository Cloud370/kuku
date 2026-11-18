use std::cmp::Reverse;
use std::env;

use kuku_client::UsageTotals;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::app::{Activity, App, ServiceStatus, SubmissionStatus};
use crate::text::{shorten, single_line, single_line_path};
use crate::theme::{emphasis, ERROR, MUTED, WARNING};

const SETTING: &str = "KUKU_TUI_STATUSLINE";
const SEPARATOR: &str = " · ";
const COMPACT_DETAILS_WIDTH: usize = 12;
const COMPACT_STATUS_WIDTH: usize = 96;
const PRIMARY_FIELDS: [Field; 5] = [
    Field::Connection,
    Field::Execution,
    Field::Agent,
    Field::Workspace,
    Field::Model,
];
const METRIC_FIELDS: [Field; 5] = [
    Field::Context,
    Field::InputTokens,
    Field::OutputTokens,
    Field::Cache,
    Field::Throughput,
];
const TOKEN_UNITS: [(u64, &str); 3] = [(1_000_000_000, "B"), (1_000_000, "M"), (1_000, "K")];

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct StatusLine {
    fields: Option<Vec<Field>>,
}

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
        let usage = app
            .pane()
            .snapshot
            .as_ref()
            .map(|snapshot| &snapshot.agent.usage);
        let compact = self.fields.is_none() && width < COMPACT_STATUS_WIDTH;
        let (fields, metrics) = self.fields.as_deref().map_or_else(
            || {
                let metrics = usage.map_or(&[][..], |_| METRIC_FIELDS.as_slice());
                (PRIMARY_FIELDS.as_slice(), metrics)
            },
            |fields| (fields, &[][..]),
        );
        let segments = fields
            .iter()
            .chain(metrics)
            .filter_map(|field| field.segment(app, usage, compact))
            .collect();
        render_segments(segments, width)
    }

    pub(crate) fn render_composer_heading(&self, app: &App, width: usize) -> Line<'static> {
        let recipient = format!("To {}", single_line(&app.active.agent));
        let badge_width = width.saturating_sub(recipient.width().min(width / 2) + 1);
        let badge = self.composer_status(app, badge_width);
        let recipient_width = width - badge.as_ref().map_or(0, |badge| badge.width() + 1);
        let recipient = shorten(&recipient, recipient_width);
        let mut spans = vec![Span::styled(recipient, Style::default().fg(MUTED))];
        if let Some(badge) = badge {
            let gap = width - spans[0].width() - badge.width();
            spans.push(Span::raw(" ".repeat(gap)));
            spans.push(badge);
        }
        Line::from(spans)
    }

    fn composer_status(&self, app: &App, width: usize) -> Option<Span<'static>> {
        if width == 0 {
            return None;
        }
        let notice = app.notice.as_ref().filter(|notice| notice.transient());
        let input_in_footer = self
            .fields
            .as_ref()
            .is_some_and(|fields| fields.contains(&Field::Input));
        if !input_in_footer {
            if let Some(input) = input(app, notice.is_none()) {
                return Some(Span::styled(input.fit(width), input.style));
            }
        }
        notice.map(|notice| {
            Span::styled(
                shorten(&single_line(&notice.text), width),
                Style::default().fg(MUTED),
            )
        })
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value.trim() {
            "" => Ok(Self::default()),
            "none" => Ok(Self {
                fields: Some(Vec::new()),
            }),
            value => {
                let mut fields = Vec::new();
                for name in value.split(',').map(str::trim) {
                    let field = Field::all()
                        .find(|field| field.name() == name)
                        .ok_or_else(|| {
                            let name = shorten(&single_line(name), 24);
                            let valid = Field::all().map(Field::name).collect::<Vec<_>>().join(",");
                            format!(
                                "{SETTING} has an unknown field {name:?}. Using the default status line. Use {valid} or none."
                            )
                        })?;
                    if fields.contains(&field) {
                        return Err(format!(
                            "{SETTING} repeats {name}. Using the default status line."
                        ));
                    }
                    fields.push(field);
                }
                Ok(Self {
                    fields: Some(fields),
                })
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Connection,
    Execution,
    Input,
    Agent,
    Workspace,
    Model,
    Context,
    InputTokens,
    OutputTokens,
    Cache,
    Throughput,
}

impl Field {
    fn all() -> impl Iterator<Item = Self> {
        PRIMARY_FIELDS
            .into_iter()
            .chain(METRIC_FIELDS)
            .chain([Self::Input])
    }

    fn name(self) -> &'static str {
        match self {
            Self::Connection => "connection",
            Self::Execution => "execution",
            Self::Input => "input",
            Self::Agent => "agent",
            Self::Workspace => "workspace",
            Self::Model => "model",
            Self::Context => "context",
            Self::InputTokens => "tokens_in",
            Self::OutputTokens => "tokens_out",
            Self::Cache => "cache",
            Self::Throughput => "tps",
        }
    }

    fn segment(self, app: &App, usage: Option<&UsageTotals>, compact: bool) -> Option<Segment> {
        let segment = match self {
            Self::Connection => connection(app),
            Self::Execution => return execution(app),
            Self::Input => return input(app, true),
            Self::Agent => Segment::details(
                format!("Agent {}", single_line(&app.active.agent)),
                Priority::Agent,
            ),
            Self::Workspace => Segment::workspace(
                app.resolved_workspace()
                    .map(single_line_path)
                    .unwrap_or_else(|| "Workspace —".into()),
            ),
            Self::Model => Segment::details(
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
                Priority::Model,
            ),
            Self::Context => Segment::measurement(
                app.pane()
                    .current_request_usage()
                    .and_then(UsageTotals::context_percent),
                if compact { "Ctx —" } else { "Context —" },
                |percent| {
                    if compact {
                        format!("Ctx {percent:.0}%")
                    } else {
                        format!("Context {percent:.0}% used")
                    }
                },
            ),
            Self::InputTokens => tokens(
                usage.and_then(|usage| usage.input_tokens),
                usage.is_some_and(UsageTotals::input_tokens_complete),
                "in",
            ),
            Self::OutputTokens => tokens(
                usage.and_then(|usage| usage.output_tokens),
                usage.is_some_and(UsageTotals::output_tokens_complete),
                "out",
            ),
            Self::Cache => Segment::measurement(
                usage.and_then(UsageTotals::cache_hit_percent),
                "Cache hit —",
                |percent| format!("Cache hit {percent:.0}%"),
            ),
            Self::Throughput => Segment::measurement(
                usage.and_then(UsageTotals::average_output_tokens_per_second),
                "— TPS",
                tokens_per_second,
            ),
        };
        Some(segment)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Priority {
    MissingMetric,
    Model,
    Metric,
    Workspace,
    Agent,
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
    Details(String),
    Workspace(String),
}

struct Segment {
    text: SegmentText,
    style: Style,
    priority: Priority,
}

impl Segment {
    fn labels(labels: Vec<String>, color: Color, priority: Priority) -> Self {
        let style = if color == Color::Reset {
            Style::default()
        } else if color == ERROR || color == WARNING {
            emphasis(color)
        } else {
            Style::default().fg(color)
        };
        Self {
            text: SegmentText::Labels(labels),
            style,
            priority,
        }
    }

    fn details(text: String, priority: Priority) -> Self {
        Self {
            text: SegmentText::Details(text),
            style: Style::default(),
            priority,
        }
    }

    fn workspace(text: String) -> Self {
        Self {
            text: SegmentText::Workspace(text),
            style: Style::default(),
            priority: Priority::Workspace,
        }
    }

    fn measurement(
        value: Option<f64>,
        placeholder: &str,
        format: impl FnOnce(f64) -> String,
    ) -> Self {
        Self::metric(
            value
                .filter(|value| value.is_finite() && *value >= 0.0)
                .map(format),
            placeholder,
        )
    }

    fn metric(label: Option<String>, placeholder: &str) -> Self {
        let (label, color, priority) = match label {
            Some(label) => (label, Color::Reset, Priority::Metric),
            None => (placeholder.to_owned(), MUTED, Priority::MissingMetric),
        };
        Self {
            text: SegmentText::Labels(vec![label]),
            style: Style::default().fg(color),
            priority,
        }
    }

    fn compact_width(&self) -> usize {
        match &self.text {
            SegmentText::Labels(labels) => labels[0].width(),
            SegmentText::Details(text) | SegmentText::Workspace(text) => {
                text.width().min(COMPACT_DETAILS_WIDTH)
            }
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
            SegmentText::Details(text) => shorten_middle(text, width),
            SegmentText::Workspace(text) => {
                if text.width() <= width {
                    text.clone()
                } else {
                    let (start, _) = tail_start(text, width.saturating_sub(1));
                    format!("…{}", &text[start..])
                }
            }
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
        ServiceStatus::Connected => (Color::Reset, Priority::Connected),
        ServiceStatus::Connecting | ServiceStatus::Reconnecting | ServiceStatus::Restarting => {
            (MUTED, Priority::ConnectionProblem)
        }
        ServiceStatus::Offline | ServiceStatus::UpdatesUnavailable => {
            (ERROR, Priority::ConnectionProblem)
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
        Activity::Ready | Activity::Finished => (Color::Reset, Priority::Ready),
        Activity::Opening
        | Activity::Running
        | Activity::WaitingForModel
        | Activity::Streaming
        | Activity::RunningTools
        | Activity::Stopped => (Color::Reset, Priority::Active),
        Activity::Waiting | Activity::Stopping | Activity::Configuration => {
            (WARNING, Priority::Attention)
        }
        Activity::Interrupted | Activity::Failed => (ERROR, Priority::Attention),
    };
    let label = if connected || pane.snapshot.is_none() || activity == Activity::Opening {
        activity.label().into()
    } else {
        format!("Last seen {}", activity.label())
    };
    Some(Segment::labels(vec![label], color, priority))
}

fn input(app: &App, show_settled: bool) -> Option<Segment> {
    let input = app.input_status();
    let submission = input
        .submission
        .filter(|status| *status != SubmissionStatus::Queued || input.queued == 0);
    let (color, priority) = match submission {
        Some(
            SubmissionStatus::ReceiptMissing
            | SubmissionStatus::ReceiptUnconfirmed
            | SubmissionStatus::Rejected,
        ) => (WARNING, Priority::Attention),
        Some(
            SubmissionStatus::Saving
            | SubmissionStatus::AwaitingReceipt
            | SubmissionStatus::CheckingReceipt
            | SubmissionStatus::Discarding,
        ) => (Color::Reset, Priority::Submission),
        Some(
            SubmissionStatus::Accepted
            | SubmissionStatus::Queued
            | SubmissionStatus::Withdrawn
            | SubmissionStatus::Confirmed,
        )
        | None => (Color::Reset, Priority::Input),
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
    if let Some(submission) = submission.filter(|status| {
        priority == Priority::Input
            && (*status == SubmissionStatus::Queued
                || (show_settled && !input.draft && input.queued == 0))
    }) {
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

fn tokens(value: Option<u64>, complete: bool, direction: &str) -> Segment {
    let label = value.map(|count| {
        let bound = if complete { "" } else { "≥" };
        format!("{bound}{} {direction}", compact_token_count(count))
    });
    Segment::metric(label, &format!("— {direction}"))
}

fn compact_token_count(count: u64) -> String {
    let Some((unit, suffix)) = TOKEN_UNITS.into_iter().find(|(unit, _)| *unit <= count) else {
        return count.to_string();
    };
    let whole = count / unit;
    if whole >= 100 {
        return format!("{whole}{suffix}");
    }
    let (precision, scale) = if whole < 10 { (2, 100) } else { (1, 10) };
    let fractional = count % unit / (unit / scale);
    if fractional == 0 {
        return format!("{whole}{suffix}");
    }
    let decimals = format!("{fractional:0precision$}");
    format!("{whole}.{}{suffix}", decimals.trim_end_matches('0'))
}

fn tokens_per_second(rate: f64) -> String {
    if rate == 0.0 {
        "0 TPS".into()
    } else if rate < 0.1 {
        "<0.1 TPS".into()
    } else {
        format!("{rate:.1} TPS")
    }
}

fn shorten_middle(text: &str, width: usize) -> String {
    if text.width() <= width || width < 8 {
        return shorten(text, width);
    }
    let (tail, used) = tail_start(text, width / 2);
    format!("{}{}", shorten(&text[..tail], width - used), &text[tail..])
}

fn tail_start(text: &str, width: usize) -> (usize, usize) {
    let mut used = 0;
    let mut tail = text.len();
    for (offset, grapheme) in text.grapheme_indices(true).rev() {
        let cells = grapheme.width();
        if used + cells > width {
            break;
        }
        used += cells;
        tail = offset;
    }
    (tail, used)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use kuku_client::{
        Acknowledgement, Error, HistoryPage, InputStatus, Receipt, Snapshot, Timing, UsageTotals,
    };
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use serde_json::{json, Value};

    use super::{
        compact_token_count, render_segments, Field, Priority, Segment, StatusLine, SEPARATOR,
        SETTING,
    };
    use crate::app::{Address, App};
    use crate::jobs::{Job, Update};
    use crate::theme::{ERROR, WARNING};
    use crate::ui;

    fn connected_app() -> App {
        connected_workspace(PathBuf::from("/workspace"))
    }

    fn connected_workspace(workspace: PathBuf) -> App {
        let mut app = App::new(workspace.clone(), None);
        app.update(Update::Connected {
            workspace,
            endpoint: "http://127.0.0.1:6789".into(),
        });
        app
    }

    fn load_agent(app: &mut App, status: &str, queued: usize, question: Value, usage: UsageTotals) {
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
                "usage": usage,
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

    fn measured_usage() -> UsageTotals {
        serde_json::from_value(json!({
            "input_tokens": 129_000_000,
            "output_tokens": 210_000,
            "cache_read_input_tokens": 96_750_000,
            "model_requests": 3,
            "unreported_requests": 0,
            "input_reported_requests": 3,
            "output_reported_requests": 3,
            "cache_reported_requests": 3,
            "measured_output_tokens": 920,
            "measured_elapsed_us": 50_000_000,
            "latest_request": {
                "input_tokens": 89_000,
                "output_tokens": 400,
                "context_window_tokens": 100_000,
                "elapsed_us": 25_000_000,
            },
        }))
        .unwrap()
    }

    fn submit_draft(app: &mut App) -> String {
        app.send(Timing::NextTurn);
        let command = app
            .effects
            .iter()
            .rev()
            .find_map(|effect| match effect {
                Job::Save(command) => Some(command.clone()),
                _ => None,
            })
            .unwrap();
        let id = command.command_id.clone();
        app.update(Update::Saved(command, Ok(())));
        id
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
            StatusLine {
                fields: Some(vec![Field::Model, Field::Workspace, Field::Execution]),
            },
            StatusLine::parse(" model, workspace,execution ").unwrap()
        );
        assert_eq!(StatusLine::default(), StatusLine::parse("").unwrap());
        assert_eq!(
            StatusLine {
                fields: Some(Vec::new())
            },
            StatusLine::parse("none").unwrap()
        );
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
                Segment::workspace("/dir/开发👩‍💻/最后".into()),
                Segment::labels(vec!["Waiting".into()], WARNING, Priority::Attention),
                Segment::labels(vec!["Offline".into()], ERROR, Priority::ConnectionProblem),
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
            vec![Segment::workspace(
                "/dir/e\u{0301}e\u{0301}e\u{0301}/👩‍💻末".into(),
            )],
            10,
        )
        .unwrap();
        assert!(line.width() <= 10);
        assert!(line.spans[0].content.ends_with("/👩‍💻末"));
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
            UsageTotals::default(),
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
        load_agent(&mut app, "running", 2, Value::Null, UsageTotals::default());
        app.pane_mut().draft.insert("First input");
        submit_draft(&mut app);
        app.pane_mut().draft.insert("Next unsent draft");
        let statusline = StatusLine::parse("execution,input").unwrap();
        let rows = screen(&mut app, &statusline, 70, 16);
        assert_eq!(
            "Running · Awaiting receipt, 2 queued, Draft",
            rows.last().unwrap().trim()
        );
    }

    #[test]
    fn draft_receipt_and_copy_feedback_keep_the_footer_in_place() {
        let mut app = connected_app();
        load_agent(&mut app, "idle", 0, Value::Null, measured_usage());
        let statusline = StatusLine::default();
        let footer = screen(&mut app, &statusline, 140, 24).pop().unwrap();
        app.pane_mut().draft.insert("First input");
        let rows = screen(&mut app, &statusline, 140, 24);
        assert_eq!(footer, rows[23]);
        assert!(rows[21].ends_with("Draft  "));
        let id = submit_draft(&mut app);
        let rows = screen(&mut app, &statusline, 140, 24);
        assert_eq!(footer, rows[23]);
        assert!(rows[21].contains("Awaiting receipt"));
        app.update(Update::Sent(
            id.clone(),
            Ok(Acknowledgement {
                receipt: Receipt {
                    command_id: id,
                    event_id: 51,
                    agent: "main".into(),
                },
                replayed: false,
                status: Some(InputStatus::Accepted),
            }),
        ));
        let rows = screen(&mut app, &statusline, 140, 24);
        assert_eq!(footer, rows[23]);
        assert!(rows[21].contains("Input accepted"));
        app.confirmation("Copied to clipboard");
        let rows = screen(&mut app, &statusline, 140, 24);
        assert_eq!(footer, rows[23]);
        assert!(rows[21].contains("Copied to clipboard"));
        assert_eq!(
            1,
            rows.iter()
                .filter(|row| row.contains("Copied to clipboard"))
                .count()
        );
        app.pane_mut().draft.insert("Second input");
        let rows = screen(&mut app, &statusline, 140, 24);
        assert_eq!(footer, rows[23]);
        assert!(rows[21].contains("Draft") && !rows[21].contains("Input accepted"));
        assert!(!rows.iter().any(|row| row.contains("Copied to clipboard")));
    }

    #[test]
    fn composer_follows_wrapped_draft_rows_and_caps_the_visible_paste() {
        let mut app = connected_app();
        let statusline = StatusLine::default();
        let rows = screen(&mut app, &statusline, 80, 20);
        assert!(rows[17].contains("To main"));
        assert!(rows[18].contains("Type a prompt"));
        app.pane_mut().draft.insert(&"a".repeat(76));
        let rows = screen(&mut app, &statusline, 80, 20);
        assert!(rows[16].contains("To main") && rows[18].trim_end().ends_with('a'));
        app.pane_mut()
            .draft
            .set_text("line 1\nline 2\nline 3\nline 4\nline 5\n中文\nline 7".into());
        let rows = screen(&mut app, &statusline, 80, 20);
        assert!(rows[14].contains("To main"));
        assert!(rows[15].contains("line 4"));
        assert!(rows[17].contains("中") && rows[17].contains("文"));
        assert!(rows[18].contains("line 7"));
        let rows = screen(&mut app, &statusline, 29, 6);
        assert!(rows[2].contains("To main"));
        assert!(rows[4].contains("line 7") && rows[5].contains("Ready"));
    }

    #[test]
    fn compact_token_counts_use_decimal_units_with_integer_arithmetic() {
        assert_eq!("12.4K", compact_token_count(12_432));
        assert_eq!("1.25M", compact_token_count(1_250_000));
        assert_eq!("999K", compact_token_count(999_999));
        assert_eq!("1B", compact_token_count(1_000_000_000));
        assert_eq!("18446744073B", compact_token_count(u64::MAX));
    }

    #[test]
    fn measured_usage_and_workspace_share_one_line_with_compact_narrow_fields() {
        let workspace = PathBuf::from("/workspace")
            .join("archived-projects".repeat(12))
            .join("work-right");
        let mut app = connected_workspace(workspace.clone());
        load_agent(&mut app, "running", 2, Value::Null, measured_usage());
        app.pane_mut().draft.insert("Local draft");
        let statusline = StatusLine::default();
        let rows = screen(&mut app, &statusline, 140, 24);
        let footer = rows[23].trim();
        for label in [
            "Connected",
            "Running",
            "Agent main",
            "work-right",
            "Context 89% used",
            "129M in",
            "210K out",
            "Cache hit 75%",
            "18.4 TPS",
        ] {
            assert!(footer.contains(label), "missing {label:?} in {footer}");
        }
        assert!(!footer.contains(workspace.to_string_lossy().as_ref()));
        assert!(rows[21].contains("To main") && rows[21].contains("2 queued, Draft"));
        assert!(rows[22].contains("Local draft"));
        let rows = screen(&mut app, &statusline, 80, 24);
        assert!(rows[23].contains("Ctx 89%") && rows[23].contains("129M in"));
        assert!(rows[23].contains("work-right") && rows[22].contains("Local draft"));
        let rows = screen(&mut app, &statusline, 29, 24);
        assert!(rows[23].contains("Running") && rows[21].contains("2 queued"));
        let rows = screen(&mut app, &StatusLine::parse("none").unwrap(), 80, 24);
        assert!(
            rows[23].contains("Local draft"),
            "the status row should be reclaimed"
        );
    }

    #[test]
    fn selected_agent_metrics_change_with_the_address_and_custom_order() {
        let mut app = connected_app();
        load_agent(&mut app, "running", 0, Value::Null, measured_usage());
        let main = app.active.clone();
        let statusline =
            StatusLine::parse("agent,tps,context,tokens_out,cache,tokens_in,workspace").unwrap();
        let rows = screen(&mut app, &statusline, 54, 20);
        assert_eq!(
            "Agent main · 18.4 TPS · 210K out · /workspace",
            rows[19].trim()
        );
        assert!(rows[18].contains("Type a prompt"));

        app.select(
            Address::new(main.session.clone(), "reviewer".into()),
            true,
            "Example".into(),
        );
        let rows = screen(&mut app, &statusline, 120, 20);
        assert_eq!(
            "Agent reviewer · — TPS · Context — · — out · Cache hit — · — in · /workspace",
            rows[19].trim()
        );
        let reviewer: UsageTotals = serde_json::from_value(json!({
            "model_requests": 2,
            "unreported_requests": 1,
            "input_tokens": 1_204,
            "output_tokens": 0,
            "input_reported_requests": 1,
            "output_reported_requests": 2,
        }))
        .unwrap();
        load_agent(&mut app, "idle", 0, Value::Null, reviewer);
        let rows = screen(&mut app, &statusline, 120, 20);
        assert_eq!(
            "Agent reviewer · — TPS · Context — · 0 out · Cache hit — · ≥1.2K in · /workspace",
            rows[19].trim()
        );
        app.select(main, true, "Example".into());
        let rows = screen(&mut app, &statusline, 120, 20);
        assert_eq!(
            "Agent main · 18.4 TPS · Context 89% used · 210K out · Cache hit 75% · 129M in · /workspace",
            rows[19].trim()
        );
    }

    #[test]
    fn next_step_hides_stale_context_and_retains_average_throughput() {
        let mut app = connected_app();
        load_agent(&mut app, "running", 0, Value::Null, measured_usage());
        let statusline = StatusLine::parse("context,tokens_in,tokens_out,cache,tps").unwrap();
        let step = serde_json::from_value(json!({
            "type": "committed", "id": 51, "agent": "main", "kind": "runtime.step_prepared"
        }))
        .unwrap();
        app.update(Update::Observation(app.selection(), step));
        let rows = screen(&mut app, &statusline, 80, 20);
        assert_eq!(
            "Context — · 129M in · 210K out · Cache hit 75% · 18.4 TPS",
            rows[19].trim()
        );
    }

    #[test]
    fn known_zero_measurements_have_distinct_labels_from_missing_metrics() {
        let mut app = connected_app();
        let statusline = StatusLine::parse("context,tokens_out,cache,tps").unwrap();
        let rows = screen(&mut app, &statusline, 80, 20);
        assert_eq!("Context — · — out · Cache hit — · — TPS", rows[19].trim());
        let usage: UsageTotals = serde_json::from_value(json!({
            "input_tokens": 100,
            "output_tokens": 0,
            "cache_read_input_tokens": 0,
            "model_requests": 2,
            "unreported_requests": 0,
            "input_reported_requests": 2,
            "output_reported_requests": 2,
            "cache_reported_requests": 2,
            "measured_output_tokens": 0,
            "measured_elapsed_us": 1_000_000,
            "latest_request": {
                "input_tokens": 0,
                "output_tokens": 0,
                "context_window_tokens": 100_000,
                "elapsed_us": 1_000_000,
            },
        }))
        .unwrap();
        load_agent(&mut app, "idle", 0, Value::Null, usage);
        let rows = screen(&mut app, &statusline, 80, 20);
        assert_eq!(
            "Context 0% used · 0 out · Cache hit 0% · 0 TPS",
            rows[19].trim()
        );
    }
}
