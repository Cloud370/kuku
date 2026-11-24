use std::ops::Range;
use std::time::Duration;

use kuku::config::ThinkLevel;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::app::{Activity, App, OutputTokens, ServiceStatus, SubmissionStatus};
use crate::keys::Shortcut;
use crate::text::{compact_token_count, format_elapsed, shorten, shorten_middle, single_line};
use crate::theme::{emphasis, ACCENT, ERROR, EXECUTION, MODEL, MUTED, SURFACE_TEXT, WARNING};

mod clock;
mod motion;

pub(crate) use clock::ActivityClock;
use motion::Ring;

const PHASE_SLOT: usize = 23;
const MIN_PHASE_SLOT: usize = 14;
const RECIPIENT_SLOT: usize = 18;
const MIN_RECIPIENT_SLOT: usize = 8;
const METER_SLOT: usize = 12;
const FIELD_GAP: &str = "  ";
const FEEDBACK_LIMIT: usize = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObservedActivity {
    Service(ServiceStatus),
    Agent(Activity),
    Sending,
    Compacting,
}

impl ObservedActivity {
    fn of(app: &App) -> Self {
        if app.setup_required {
            return Self::Agent(Activity::Configuration);
        }
        let service = app.service_status();
        if service == ServiceStatus::Connected {
            let activity = app.pane().composer_activity();
            if matches!(activity, Activity::Ready | Activity::Finished)
                && matches!(
                    app.input_status().submission,
                    Some(SubmissionStatus::Saving | SubmissionStatus::AwaitingReceipt)
                )
            {
                Self::Sending
            } else if activity == Activity::Running
                && app
                    .pane()
                    .transcript
                    .compacting(app.pane().turn_timing().map(|timing| timing.turn))
            {
                Self::Compacting
            } else {
                Self::Agent(activity)
            }
        } else {
            Self::Service(service)
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Service(service) => service.label(),
            Self::Sending => "Send",
            Self::Compacting => "Compacting context",
            Self::Agent(Activity::WaitingForModel) => "Waiting for model",
            Self::Agent(Activity::Streaming) => "Reply",
            Self::Agent(Activity::ToolArguments) => "Model tool arguments",
            Self::Agent(Activity::RetryingModel) => "Retrying",
            Self::Agent(activity) => activity.label(),
        }
    }

    fn short_label(self) -> &'static str {
        match self {
            Self::Agent(Activity::WaitingForModel) => "Model wait",
            Self::Agent(Activity::ToolArguments) => "Model args",
            Self::Agent(Activity::RunningTools) => "Tools running",
            Self::Compacting => "Compacting",
            status => status.label(),
        }
    }

    fn color(self) -> Color {
        match self {
            Self::Service(ServiceStatus::Offline | ServiceStatus::UpdatesUnavailable)
            | Self::Agent(Activity::Interrupted | Activity::Failed) => ERROR,
            Self::Service(ServiceStatus::Reconnecting)
            | Self::Agent(
                Activity::Waiting
                | Activity::Configuration
                | Activity::Stopping
                | Activity::Stopped,
            ) => WARNING,
            Self::Sending => EXECUTION,
            phase if phase.motion().is_some() => EXECUTION,
            _ => MUTED,
        }
    }

    fn motion(self) -> Option<Activity> {
        match self {
            Self::Compacting => Some(Activity::Running),
            Self::Agent(
                activity @ (Activity::Running
                | Activity::WaitingForModel
                | Activity::RetryingModel
                | Activity::Thinking
                | Activity::Streaming
                | Activity::ToolArguments
                | Activity::RunningTools),
            ) => Some(activity),
            _ => None,
        }
    }

    fn can_coalesce(self) -> bool {
        self.motion().is_some()
    }

    fn clock_runs(self) -> bool {
        self.motion().is_some()
            || matches!(self, Self::Agent(Activity::Waiting | Activity::Stopping))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActivityFrame {
    status: ObservedActivity,
    ring: Ring,
    text_step: Option<u128>,
    thinking: String,
    elapsed: Option<u64>,
    phase_elapsed: Option<u64>,
}

impl ActivityFrame {
    pub(crate) fn phase_changed(&self, previous: &Self) -> bool {
        self.status != previous.status
    }

    pub(crate) fn changes_with_time(&self) -> bool {
        self.status.motion().is_some() || (self.elapsed.is_some() && self.status.clock_runs())
    }

    #[cfg(test)]
    pub(super) fn test_frame(app: &App, step: Option<u128>) -> Self {
        let elapsed = app
            .pane()
            .turn_timing()
            .and_then(|timing| timing.elapsed_ms);
        Self::new(ObservedActivity::of(app), step, elapsed.map(|ms| ms / 1000))
    }

    fn new(status: ObservedActivity, step: Option<u128>, elapsed: Option<u64>) -> Self {
        let tone = match status.color() {
            color @ (ERROR | WARNING) => color,
            _ => ACCENT,
        };
        let ring = match status {
            ObservedActivity::Sending => Ring::phase("↑", EXECUTION, step),
            ObservedActivity::Agent(Activity::WaitingForModel) => Ring::phase("…", EXECUTION, step),
            ObservedActivity::Agent(Activity::Thinking) => {
                Ring::phase("◇", crate::theme::MODEL, step)
            }
            ObservedActivity::Agent(Activity::Streaming) => {
                Ring::phase("↓", crate::theme::ACTIVITY_MINT, step)
            }
            ObservedActivity::Agent(Activity::ToolArguments | Activity::RunningTools) => {
                Ring::phase("↳", crate::theme::USAGE, step)
            }
            _ => step
                .filter(|_| status.motion().is_some())
                .map_or_else(|| Ring::resting(tone), Ring::moving),
        };
        Self {
            status,
            ring,
            text_step: step,
            thinking: String::new(),
            elapsed,
            phase_elapsed: None,
        }
    }

    pub(super) fn thinking_line(&self, width: usize) -> Line<'static> {
        let phase_elapsed = if self.status == ObservedActivity::Agent(Activity::Thinking) {
            self.phase_elapsed
        } else {
            None
        };
        let elapsed = phase_elapsed
            .map(|seconds| format_elapsed(Duration::from_secs(seconds)))
            .filter(|elapsed| elapsed.width() + 1 < width);
        let left = elapsed
            .as_ref()
            .map_or(width, |elapsed| width - elapsed.width() - 1);
        let mut line = motion::thinking_label(&shorten("Thinking", left), self.text_step);
        let room = left.saturating_sub(line.width() + 2);
        if room >= 4 && !self.thinking.is_empty() {
            line.push_span(Span::styled(
                format!("  {}", motion::flow_excerpt(&self.thinking, room)),
                Style::new().fg(MUTED),
            ));
        }
        if let Some(elapsed) = elapsed {
            line.push_span(" ".repeat(width - line.width() - elapsed.width()));
            line.push_span(Span::styled(elapsed, Style::new().fg(MUTED)));
        }
        line
    }

    pub(super) fn still(self) -> Self {
        Self {
            phase_elapsed: self.phase_elapsed,
            ..Self::new(self.status, None, self.elapsed)
        }
    }

    pub(super) fn agent_activity(&self, app: &App) -> Activity {
        match self.status {
            ObservedActivity::Agent(activity) => activity,
            ObservedActivity::Service(_)
            | ObservedActivity::Sending
            | ObservedActivity::Compacting => app.pane().activity(),
        }
    }
}

#[cfg(test)]
#[test]
fn thinking_preview_yields_space_to_its_right_aligned_timer() {
    let mut frame = ActivityFrame::new(ObservedActivity::Agent(Activity::Thinking), None, None);
    frame.thinking = "Checking the totals and reconciling 中文 details".to_owned();
    frame.phase_elapsed = Some(18);
    for width in [12, 19, 64] {
        let line = frame.thinking_line(width);
        assert_eq!(line.width(), width);
        assert_eq!(line.spans.last().unwrap().content.as_ref(), "18s");
    }
    assert!(frame.thinking_line(2).width() <= 2);
    frame.status = ObservedActivity::Agent(Activity::RunningTools);
    assert!(frame
        .thinking_line(64)
        .spans
        .iter()
        .all(|span| span.content.as_ref() != "18s"));
}

pub(super) fn render_heading(
    app: &App,
    width: usize,
    frame: ActivityFrame,
) -> (Line<'static>, Option<Range<usize>>) {
    let muted = Style::default().fg(MUTED);
    let gap = FIELD_GAP.len();
    let recipient_room = (width / 3)
        .clamp(MIN_RECIPIENT_SLOT, RECIPIENT_SLOT)
        .min(width.saturating_sub(MIN_PHASE_SLOT + gap));
    let recipient = app
        .visible_recipient()
        .filter(|_| recipient_room >= MIN_RECIPIENT_SLOT);
    let recipient_width = recipient.map_or(0, |_| recipient_room + gap);
    let phase_width = (width - recipient_width).min(PHASE_SLOT);
    let core_width = phase_width + recipient_width;
    let feedback_limit = width
        .saturating_sub(core_width + 1)
        .min(width / 3)
        .min(FEEDBACK_LIMIT);
    let feedback = app
        .displayed_statusline()
        .composer_feedback(app, feedback_limit);
    let left_width = width - feedback.as_ref().map_or(0, |feedback| feedback.width() + 1);

    let key = Shortcut::CYCLE_THINKING.label;
    let action = " think";
    let key_width = gap + key.width() + action.width();
    let show_thinking = left_width >= core_width + key_width
        && app.modal.is_none()
        && app.available()
        && app
            .pane()
            .effective_model_settings()
            .is_some_and(|(_, caps)| {
                caps.selectable_reasoning_efforts()
                    .any(|level| level != ThinkLevel::ProviderDefault.as_str())
            });
    let thinking_width = if show_thinking { key_width } else { 0 };
    let render_metrics = left_width >= core_width + thinking_width + METER_SLOT + gap;
    let mut line = frame.ring.line(phase_width);
    if phase_width > line.width() {
        line.push_span(" ");
    }
    let label_room = phase_width.saturating_sub(line.width());
    let label = frame.status.label();
    let status = shorten(
        if label.width() <= label_room {
            label
        } else {
            frame.status.short_label()
        },
        label_room,
    );
    line.push_span(Span::styled(status, Style::new().fg(frame.status.color())));

    if recipient.is_some() || render_metrics || show_thinking {
        line.push_span(" ".repeat(phase_width - line.width()));
    }
    let recipient_hit = recipient.map(|name| {
        let name = single_line(name);
        let suffix = " ▾";
        let long_prefix = "Send to ";
        let prefix = if long_prefix.len() + name.width() + suffix.width() <= recipient_room {
            long_prefix
        } else {
            "To "
        };
        let name = shorten_middle(&name, recipient_room - prefix.len() - suffix.width());
        let label = format!("{prefix}{name}{suffix}");
        let start = phase_width + gap;
        let end = start + label.width();
        line.push_span(FIELD_GAP);
        line.push_span(Span::styled(
            label,
            emphasis(if app.is_subagent() { MODEL } else { ACCENT }),
        ));
        if render_metrics || show_thinking {
            line.push_span(" ".repeat(start + recipient_room - end));
        }
        start..end
    });

    if render_metrics {
        let tokens = match frame.status {
            ObservedActivity::Agent(_) => {
                app.pane().composer_output_tokens(frame.agent_activity(app))
            }
            _ => None,
        };
        let metric = match tokens {
            Some(OutputTokens::Reported(count)) => format!("{} tok", compact_token_count(count)),
            Some(OutputTokens::Estimated(count)) => format!("~{} tok", compact_token_count(count)),
            None => frame
                .elapsed
                .filter(|_| frame.status.clock_runs())
                .map_or_else(String::new, |seconds| {
                    format!("Turn {}", format_elapsed(Duration::from_secs(seconds)))
                }),
        };
        let metric = shorten(&metric, METER_SLOT);
        if !metric.is_empty() || show_thinking {
            let padding = if show_thinking {
                METER_SLOT - metric.width()
            } else {
                0
            };
            line.push_span(FIELD_GAP);
            line.push_span(Span::styled(metric, muted));
            line.push_span(" ".repeat(padding));
        }
    }

    if show_thinking {
        line.push_span(FIELD_GAP);
        line.push_span(Span::styled(key, Style::new().fg(SURFACE_TEXT)));
        line.push_span(Span::styled(action, muted));
    }

    if frame.status == ObservedActivity::Agent(Activity::RunningTools) {
        let tool = app.pane().running_tool_name();
        if let Some(tool) = tool.filter(|_| left_width.saturating_sub(line.width()) > 8) {
            let tool = shorten(tool, left_width - line.width() - 3);
            line.push_span(Span::styled(format!(" · {tool}"), muted));
        } else {
            let count = app.pane().parallel_tools();
            let summary = format!(" · {count}");
            if count > 1 && left_width.saturating_sub(line.width()) >= summary.width() {
                line.push_span(Span::styled(summary, muted));
            }
        }
    }

    if let Some(jobs) = app
        .pane()
        .snapshot
        .as_ref()
        .map(|snapshot| snapshot.agent.running_jobs)
        .filter(|jobs| *jobs > 0)
    {
        let label = if jobs == 1 { "job" } else { "jobs" };
        let summary = format!(" · {jobs} {label}");
        if left_width.saturating_sub(line.width()) >= summary.width() {
            line.push_span(Span::styled(summary, muted));
            if left_width.saturating_sub(line.width()) >= " · /jobs".width() {
                line.push_span(Span::styled(" · /jobs", muted));
            }
        }
    }

    if matches!(
        frame.status,
        ObservedActivity::Agent(Activity::Streaming | Activity::Thinking | Activity::ToolArguments)
    ) {
        let count = app.pane().parallel_tools();
        if count != 0 {
            let tools = format!("↳{}", count.min(999));
            if left_width.saturating_sub(line.width()) >= gap + tools.width() {
                line.push_span(FIELD_GAP);
                line.push_span(Span::styled(tools, Style::new().fg(crate::theme::USAGE)));
            }
        }
    }
    if let Some(seconds) = frame.phase_elapsed {
        let duration = format_elapsed(Duration::from_secs(seconds));
        if left_width.saturating_sub(line.width()) >= gap + duration.width() {
            line.push_span(FIELD_GAP);
            line.push_span(Span::styled(duration, muted));
        }
    }
    if let Some(feedback) = feedback {
        line.push_span(" ".repeat(width - line.width() - feedback.width()));
        line.push_span(feedback);
    }
    (line, recipient_hit)
}
