use std::cmp::Reverse;
use std::path::{Path, MAIN_SEPARATOR};

use kuku_client::UsageTotals;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::app::{Activity, App, ServiceStatus, SubmissionStatus};
use crate::text::{
    compact_token_count, effort_label, shorten, shorten_middle, single_line, single_line_path,
    tail_start,
};
use crate::theme::{emphasis, ACCENT, ERROR, EXECUTION, MODEL, MUTED, USAGE, WARNING};

mod editor;

pub(crate) use editor::StatusLineEditor;

const SEPARATOR: &str = " · ";
const WIDE_SEPARATOR: &str = "  ";
const GROUP_GAP: &str = "   ";
const COMPACT_DETAILS_WIDTH: usize = 12;
const COMPACT_MODEL_WIDTH: usize = 28;
const MAX_WORKSPACE_WIDTH: usize = 24;
const MAX_EFFORT_WIDTH: usize = 12;
const COMPACT_STATUS_WIDTH: usize = 96;
const PHONE_WIDTH: usize = 60;
const TWO_ROW_MIN_HEIGHT: u16 = 10;
const DEFAULT_FIELDS: [Field; 9] = [
    Field::Model,
    Field::Agent,
    Field::Workspace,
    Field::Context,
    Field::Cache,
    Field::Throughput,
    Field::InputTokens,
    Field::OutputTokens,
    Field::Cost,
];
const OPTIONAL_FIELDS: [Field; 3] = [Field::Connection, Field::Execution, Field::Input];

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

    pub(crate) fn render_rows(
        &self,
        app: &App,
        width: usize,
        height: u16,
        activity: Activity,
    ) -> Vec<Line<'static>> {
        let usage = app
            .pane()
            .snapshot
            .as_ref()
            .map(|snapshot| &snapshot.agent.usage);
        let custom = self.fields.is_some();
        let narrow = width < COMPACT_STATUS_WIDTH;
        let compact = !custom && narrow;
        let mut segments: Vec<_> = self
            .fields()
            .iter()
            .filter(|field| custom || usage.is_some() || !field.is_measurement())
            .filter_map(|field| {
                field
                    .segment(app, usage, compact, custom, activity)
                    .map(|segment| (*field, segment))
            })
            .collect();
        if !custom {
            if let Some(notice) = connection(app, false) {
                segments.insert(1, (Field::Connection, notice));
            }
        }
        if !self.fields().contains(&Field::Cache) {
            if let Some(warning) = cache_warning(app) {
                segments.push((Field::Cache, warning));
            }
        }
        if width <= PHONE_WIDTH && height >= TWO_ROW_MIN_HEIGHT {
            let split = if custom {
                custom_row_break(&segments, width)
            } else {
                segments
                    .iter()
                    .position(|(field, _)| *field == Field::Workspace)
            };
            if let Some(split) = split {
                let mut first: Vec<_> = segments.into_iter().map(|(_, segment)| segment).collect();
                let second = first.split_off(split);
                return [first, second]
                    .into_iter()
                    .filter_map(|row| render_segments(row, width, SEPARATOR))
                    .collect();
            }
        }
        let separator = if custom || narrow {
            SEPARATOR
        } else {
            WIDE_SEPARATOR
        };
        let split = if narrow {
            None
        } else {
            segments
                .iter()
                .position(|(field, _)| field.is_measurement())
        };
        let mut segments = segments.into_iter().map(|(_, segment)| segment);
        let line = if let Some(split) = split {
            let left = segments.by_ref().take(split).collect();
            render_groups(left, segments.collect(), width, separator)
        } else {
            render_segments(segments.collect(), width, separator)
        };
        line.into_iter().collect()
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
        let input = (!input_in_footer)
            .then(|| input(app, notice.is_none()))
            .flatten();
        if let Some(input) = input
            .as_ref()
            .filter(|input| input.priority >= Priority::Submission)
        {
            return Some(Span::styled(input.fit(width), input.style));
        }
        if let Some(effort) = app.pane().next_thinking() {
            let label = format!("Next {}", effort_label(effort));
            return Some(Span::styled(
                shorten(&label, width),
                Style::new().fg(ACCENT),
            ));
        }
        if let Some(input) = input {
            return Some(Span::styled(input.fit(width), input.style));
        }
        notice.map(|notice| {
            Span::styled(
                shorten(&single_line(&notice.text), width),
                Style::default().fg(MUTED),
            )
        })
    }
}

fn custom_row_break(segments: &[(Field, Segment)], width: usize) -> Option<usize> {
    let gap = SEPARATOR.width();
    let total = segments
        .iter()
        .map(|(_, segment)| segment.compact_width(width) + gap)
        .sum::<usize>()
        .saturating_sub(gap);
    if total <= width || segments.len() < 2 {
        return None;
    }
    segments[..segments.len() - 1]
        .iter()
        .enumerate()
        .scan(0, |prefix, (index, (_, segment))| {
            *prefix += segment.compact_width(width) + gap;
            Some((index + 1, (*prefix - gap).abs_diff(total - *prefix)))
        })
        .min_by_key(|(_, difference)| *difference)
        .map(|(split, _)| split)
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
    Cost,
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
            Self::Cost => "cost",
            Self::Throughput => "tps",
        }
    }

    fn is_measurement(self) -> bool {
        matches!(
            self,
            Self::Context
                | Self::InputTokens
                | Self::OutputTokens
                | Self::Cache
                | Self::Cost
                | Self::Throughput
        )
    }

    fn segment(
        self,
        app: &App,
        usage: Option<&UsageTotals>,
        compact: bool,
        custom: bool,
        activity: Activity,
    ) -> Option<Segment> {
        let segment = match self {
            Self::Connection => return connection(app, custom),
            Self::Execution => return execution(app, activity),
            Self::Input => return input(app, true),
            Self::Agent => Segment::details(
                format!("Agent {}", single_line(&app.active.agent)),
                Priority::Agent,
            ),
            Self::Workspace => Segment::workspace(app.resolved_workspace()),
            Self::Model => {
                let pane = app.pane();
                if let Some((id, connection, effort)) = pane.effective_model_identity() {
                    let effort = effort
                        .filter(|value| {
                            *value != kuku::config::ThinkLevel::ProviderDefault.as_str()
                        })
                        .unwrap_or("Default");
                    let effort = shorten(&single_line(effort), MAX_EFFORT_WIDTH);
                    let resolved = pane.effective_model_settings();
                    let service_tier = resolved.and_then(|(settings, _)| {
                        settings.service_tier.as_deref().map(|tier| {
                            single_line(settings.requested_service_tier.as_deref().unwrap_or(tier))
                        })
                    });
                    let slot = resolved
                        .and_then(|(_, caps)| {
                            caps.selectable_reasoning_efforts()
                                .map(|level| single_line(level).width().min(MAX_EFFORT_WIDTH))
                                .max()
                        })
                        .map_or(effort.width(), |width| {
                            width.max(effort.width()).max("Default".width())
                        });
                    let show_connection = custom
                        && resolved.is_none_or(|(_, caps)| caps.subscription_channel.is_none());
                    Segment::model(
                        single_line(id),
                        show_connection.then(|| single_line(connection)),
                        Some(effort),
                        service_tier,
                        slot,
                    )
                } else {
                    let label = pane.effective_model_label().unwrap_or_else(|| {
                        if pane.model_problem.is_some() {
                            "Model unavailable".into()
                        } else {
                            "Model pending".into()
                        }
                    });
                    Segment::model(single_line(&label), None, None, None, 0)
                }
            }
            Self::Context => context(app.pane().current_request_usage(), compact, custom),
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
            Self::Cache => cache_warning(app).unwrap_or_else(|| {
                let (label, missing) = if compact {
                    ("Cache", "Cache —")
                } else {
                    ("Cache hit", "Cache hit —")
                };
                Segment::measurement(
                    usage.and_then(UsageTotals::cache_hit_percent),
                    missing,
                    |percent| format!("{label} {percent:.0}%"),
                )
            }),
            Self::Cost => {
                let session_costs = app
                    .pane()
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.session_costs.as_ref());
                let (reported, estimated, unknown) = if let Some(costs) = session_costs {
                    (
                        costs.reported_nanodollars,
                        costs.estimated_nanodollars,
                        costs.unknown_requests,
                    )
                } else {
                    let usage = usage?;
                    (
                        usage.costs.reported_nanodollars,
                        usage.costs.estimated_nanodollars,
                        usage.unknown_cost_requests(),
                    )
                };
                Segment::metric(compact_cost(reported, estimated, unknown), "—")
            }
            Self::Throughput => Segment::measurement(
                usage.and_then(UsageTotals::average_output_tokens_per_second),
                "— TPS",
                tokens_per_second,
            ),
        };
        Some(segment)
    }
}

fn context(usage: Option<&UsageTotals>, compact: bool, custom: bool) -> Segment {
    let measure = usage.and_then(|usage| {
        let request = usage.latest_request.as_ref()?;
        Some((
            usage.context_percent()?,
            request.input_tokens?,
            request.context_window_tokens?,
        ))
    });
    let Some((percent, input, window)) = measure else {
        return Segment::metric(None, if compact { "Ctx —" } else { "Context —" });
    };
    let mut labels = vec![if compact {
        format!("Ctx {percent:.0}%")
    } else {
        format!("Context {percent:.0}% used")
    }];
    if !compact && !custom {
        labels.push(format!(
            "Context {}/{} ({percent:.0}%)",
            compact_token_count(input),
            compact_token_count(u64::from(window)),
        ));
    }
    Segment::labels(labels, USAGE, Priority::Metric)
}

fn cache_warning(app: &App) -> Option<Segment> {
    let warning = app.cache_warning()?;
    Some(Segment::labels(
        vec![
            format!("⚠ Cache hit {}%", warning.latest_percent),
            "⚠ Cache".into(),
        ],
        WARNING,
        Priority::Attention,
    ))
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
    Model {
        id: String,
        connection: Option<String>,
        effort: Option<String>,
        service_tier: Option<String>,
        effort_slot: usize,
    },
}

struct Segment {
    text: SegmentText,
    style: Style,
    priority: Priority,
}

impl Segment {
    fn model(
        id: String,
        connection: Option<String>,
        effort: Option<String>,
        service_tier: Option<String>,
        effort_slot: usize,
    ) -> Self {
        Self {
            text: SegmentText::Model {
                id,
                connection,
                effort,
                service_tier,
                effort_slot,
            },
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

    fn workspace(workspace: Option<&Path>) -> Self {
        let text = workspace.map_or_else(
            || "Workspace —".into(),
            |path| {
                let full = single_line_path(path);
                path.file_name()
                    .filter(|_| full.width() > MAX_WORKSPACE_WIDTH)
                    .map_or(full, |name| {
                        format!("…{MAIN_SEPARATOR}{}", single_line(&name.to_string_lossy()))
                    })
            },
        );
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

    fn compact_width(&self, row_width: usize) -> usize {
        match &self.text {
            SegmentText::Labels(labels) => labels[0].width(),
            SegmentText::Model {
                id,
                effort,
                effort_slot,
                ..
            } => {
                let effort_width = effort.as_ref().map_or(0, |_| 1 + effort_slot);
                (id.width() + effort_width)
                    .min(COMPACT_MODEL_WIDTH)
                    .min((row_width / 3).max(effort_width + 1))
            }
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
            SegmentText::Model {
                id,
                connection,
                effort,
                service_tier,
                effort_slot,
            } => {
                let mut suffix = effort
                    .as_ref()
                    .map_or_else(String::new, |effort| format!(" {effort}"));
                let service_tier = service_tier
                    .as_ref()
                    .filter(|tier| id.width() + suffix.width() + 1 + tier.width() <= width);
                if let Some(tier) = service_tier {
                    suffix.push(' ');
                    suffix.push_str(tier);
                }
                let id_min = if service_tier.is_some() {
                    id.width()
                } else {
                    1
                };
                let pad = effort.as_ref().map_or(0, |effort| {
                    effort_slot
                        .saturating_sub(effort.width())
                        .min(width.saturating_sub(suffix.width() + id_min))
                });
                let available = width - pad;
                let mut text = if let Some(connection) = connection.as_ref().filter(|connection| {
                    id.width() + suffix.width() + 3 + connection.width() <= available
                }) {
                    format!("{id}{suffix} ({connection})")
                } else if available > suffix.width() {
                    format!("{}{suffix}", shorten_middle(id, available - suffix.width()))
                } else {
                    shorten_middle(id, available)
                };
                text.push_str(&" ".repeat(pad));
                text
            }
            SegmentText::Workspace(text) => {
                let width = width.min(MAX_WORKSPACE_WIDTH);
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

fn render_groups(
    left: Vec<Segment>,
    right: Vec<Segment>,
    width: usize,
    separator: &'static str,
) -> Option<Line<'static>> {
    let minimum_left = if left.is_empty() {
        0
    } else {
        left.iter()
            .map(|segment| segment.compact_width(width))
            .sum::<usize>()
            + separator.width() * (left.len() - 1)
            + GROUP_GAP.width()
    };
    let right_width = (width / 2).max(width.saturating_sub(minimum_left));
    let Some(right) = render_segments(right, right_width, separator) else {
        return render_segments(left, width, separator);
    };
    let left_width = width - right.width();
    let mut line = render_segments(
        left,
        left_width.saturating_sub(GROUP_GAP.width()),
        separator,
    )
    .unwrap_or_default();
    line.push_span(" ".repeat(left_width - line.width()));
    line.extend(right.spans);
    Some(line)
}

fn render_segments(
    segments: Vec<Segment>,
    width: usize,
    gap: &'static str,
) -> Option<Line<'static>> {
    let mut by_priority: Vec<_> = (0..segments.len()).collect();
    by_priority.sort_by_key(|index| Reverse(segments[*index].priority));
    let mut reserved = vec![0; segments.len()];
    let mut remaining = width;
    for &index in &by_priority {
        let separator = if remaining == width { 0 } else { gap.width() };
        let compact = segments[index].compact_width(width);
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
        line.push(Span::styled(gap, Style::default().fg(MUTED)));
        line.push(span);
    }
    Some(Line::from(line))
}

fn connection(app: &App, show_connected: bool) -> Option<Segment> {
    let status = app.service_status();
    if status == ServiceStatus::Connected && !show_connected {
        return None;
    }
    let (color, priority) = match status {
        ServiceStatus::Connected => (Color::Reset, Priority::Connected),
        ServiceStatus::Connecting | ServiceStatus::Reconnecting | ServiceStatus::Restarting => {
            (MUTED, Priority::ConnectionProblem)
        }
        ServiceStatus::Offline | ServiceStatus::UpdatesUnavailable => {
            (ERROR, Priority::ConnectionProblem)
        }
    };
    let mut labels = match status {
        ServiceStatus::Offline => vec![
            "kuku service offline".into(),
            "kuku service offline · Ctrl+R".into(),
        ],
        ServiceStatus::UpdatesUnavailable => vec![
            "kuku service updates unavailable".into(),
            "kuku service updates unavailable · Ctrl+R".into(),
        ],
        ServiceStatus::Reconnecting => vec!["Reconnecting to kuku service".into()],
        ServiceStatus::Connected | ServiceStatus::Connecting | ServiceStatus::Restarting => {
            vec![status.label().to_owned()]
        }
    };
    if show_connected {
        let kind = if app.remote { "Remote" } else { "Local" };
        labels.push(format!("{} {kind}", status.label()));
    }
    Some(Segment::labels(labels, color, priority))
}

fn execution(app: &App, activity: Activity) -> Option<Segment> {
    let pane = app.pane();
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

fn compact_cost(reported: u64, estimated: u64, unknown: u64) -> Option<String> {
    let total = u128::from(reported) + u128::from(estimated);
    if total == 0 && unknown != 0 {
        return None;
    }
    let prefix = if estimated != 0 {
        "~"
    } else if unknown != 0 {
        "≥"
    } else {
        ""
    };
    if (1..1_000).contains(&total) {
        return Some(
            match prefix {
                "~" => "~$0.000001",
                "≥" => ">$0",
                _ => "<$0.000001",
            }
            .into(),
        );
    }
    let precision = if total < 1_000_000 { 6 } else { 3 };
    let unit = 10u128.pow(9 - precision as u32);
    let total = if prefix == "≥" {
        total / unit * unit
    } else {
        total
    };
    let amount = format!("{:.precision$}", total as f64 / 1_000_000_000.0);
    Some(format!(
        "{prefix}${}",
        amount.trim_end_matches('0').trim_end_matches('.')
    ))
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

#[cfg(test)]
mod tests;
