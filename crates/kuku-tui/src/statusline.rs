use std::cmp::Reverse;

use kuku_client::UsageTotals;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::app::{Activity, App, ServiceStatus, SubmissionStatus};
use crate::text::{compact_token_count, shorten, single_line, single_line_path};
use crate::theme::{emphasis, ERROR, EXECUTION, MODEL, MUTED, USAGE, WARNING};

mod editor;

pub(crate) use editor::StatusLineEditor;

const SEPARATOR: &str = " · ";
const COMPACT_DETAILS_WIDTH: usize = 12;
const COMPACT_MODEL_WIDTH: usize = 28;
const COMPACT_STATUS_WIDTH: usize = 96;
const DEFAULT_FIELDS: [Field; 6] = [
    Field::Model,
    Field::Connection,
    Field::Agent,
    Field::Workspace,
    Field::Context,
    Field::Throughput,
];
const OPTIONAL_FIELDS: [Field; 5] = [
    Field::Execution,
    Field::InputTokens,
    Field::OutputTokens,
    Field::Cache,
    Field::Input,
];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StatusLine {
    fields: Option<Vec<Field>>,
}

impl StatusLine {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value.trim() {
            "" => Ok(Self::default()),
            "none" => Self::from_fields([]),
            value => Self::from_fields(value.split(',')),
        }
    }

    pub(crate) fn from_fields<'a>(
        names: impl IntoIterator<Item = &'a str>,
    ) -> Result<Self, String> {
        let mut fields = Vec::new();
        for name in names {
            let name = name.trim();
            let field = Field::all()
                .find(|field| field.name() == name)
                .ok_or_else(|| {
                    let name = shorten(&single_line(name), 24);
                    let valid = Field::all().map(Field::name).collect::<Vec<_>>().join(", ");
                    format!("unknown field {name:?}. Valid fields are {valid}.")
                })?;
            if fields.contains(&field) {
                return Err(format!("Field list repeats {name}."));
            }
            fields.push(field);
        }
        Ok(Self {
            fields: Some(fields),
        })
    }

    pub(crate) fn configured_names(&self) -> Option<Vec<&'static str>> {
        self.fields
            .as_ref()
            .map(|fields| fields.iter().copied().map(Field::name).collect())
    }

    pub(crate) fn render_line(&self, app: &App, width: usize) -> Option<Line<'static>> {
        let usage = app
            .pane()
            .snapshot
            .as_ref()
            .map(|snapshot| &snapshot.agent.usage);
        let custom = self.fields.is_some();
        let compact = !custom && width < COMPACT_STATUS_WIDTH;
        let segments = self
            .fields()
            .iter()
            .filter(|field| {
                custom || usage.is_some() || !matches!(field, Field::Context | Field::Throughput)
            })
            .filter_map(|field| field.segment(app, usage, compact, custom))
            .collect();
        render_segments(segments, width)
    }

    fn fields(&self) -> &[Field] {
        self.fields.as_deref().unwrap_or(&DEFAULT_FIELDS)
    }

    pub(crate) fn composer_feedback(&self, app: &App, width: usize) -> Option<Span<'static>> {
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
        DEFAULT_FIELDS.into_iter().chain(OPTIONAL_FIELDS)
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

    fn segment(
        self,
        app: &App,
        usage: Option<&UsageTotals>,
        compact: bool,
        custom: bool,
    ) -> Option<Segment> {
        let segment = match self {
            Self::Connection => connection(app, custom),
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
            Self::Model => {
                let pane = app.pane();
                let (id, route) = match pane.effective_model_identity() {
                    Some((id, connection, effort)) => {
                        let id = single_line(id);
                        let route = custom.then(|| {
                            let connection = single_line(connection);
                            match effort {
                                Some(effort) => {
                                    format!("{id} ({connection} · {})", single_line(effort))
                                }
                                None => format!("{id} ({connection})"),
                            }
                        });
                        (id, route)
                    }
                    None => {
                        let label = pane.effective_model_label().unwrap_or_else(|| {
                            if pane.model_problem.is_some() {
                                "Model unavailable".into()
                            } else {
                                "Model pending".into()
                            }
                        });
                        (single_line(&label), None)
                    }
                };
                Segment::model(id, route)
            }
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
    Metric,
    Workspace,
    Agent,
    Connected,
    Ready,
    Model,
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
    Model { id: String, route: Option<String> },
}

struct Segment {
    text: SegmentText,
    style: Style,
    priority: Priority,
}

impl Segment {
    fn model(id: String, route: Option<String>) -> Self {
        Self {
            text: SegmentText::Model { id, route },
            style: Style::default().fg(MODEL),
            priority: Priority::Model,
        }
    }
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
            Some(label) => (label, USAGE, Priority::Metric),
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
            SegmentText::Model { id, .. } => id.width().min(COMPACT_MODEL_WIDTH),
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
            SegmentText::Model { id, route } => route
                .as_ref()
                .filter(|route| route.width() <= width)
                .cloned()
                .unwrap_or_else(|| shorten_middle(id, width)),
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

fn connection(app: &App, show_endpoint: bool) -> Segment {
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
    let mut labels = vec![status.label().to_owned()];
    if status == ServiceStatus::UpdatesUnavailable {
        labels.insert(0, "No updates".into());
    }
    if show_endpoint {
        let kind = if app.remote { "Remote" } else { "Local" };
        let location = format!("{} {kind}", status.label());
        if let Some(endpoint) = &app.endpoint {
            labels.push(location.clone());
            labels.push(format!("{location} {}", single_line(endpoint)));
        } else {
            labels.push(location);
        }
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
        Activity::Ready | Activity::Finished => (EXECUTION, Priority::Ready),
        Activity::Opening
        | Activity::Running
        | Activity::WaitingForModel
        | Activity::RetryingModel
        | Activity::Thinking
        | Activity::Streaming
        | Activity::ToolArguments
        | Activity::RunningTools
        | Activity::Stopped => (EXECUTION, Priority::Active),
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
mod tests;
