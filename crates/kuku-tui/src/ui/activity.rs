use std::ops::Range;
use std::time::Duration;

use kuku::config::ThinkLevel;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::app::{Activity, App, OutputTokens, ServiceStatus, SubmissionStatus, WaitKind};
use crate::keys::Shortcut;
use crate::text::{compact_token_count, format_elapsed, shorten, shorten_middle, single_line};
use crate::theme::{ACCENT, ERROR, EXECUTION, MODEL, MUTED, SURFACE_TEXT, WARNING, emphasis};

mod clock;
mod motion;

pub(crate) use clock::ActivityClock;
use motion::Ring;

const PHASE_SLOT: usize = 25;
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
    PreparingFile,
    WaitingFor(WaitKind),
    ModelFailed,
}

impl ObservedActivity {
    fn of(app: &App) -> Self {
        if app.setup_required {
            return Self::Agent(Activity::Configuration);
        }
        let service = app.service_status();
        if service == ServiceStatus::Connected {
            if app.permission_mode.is_none() && !app.pane().exists {
                return Self::Agent(Activity::Configuration);
            }
            let activity = app.pane().composer_activity();
            if activity.shows_sending()
                && app
                    .input_status()
                    .submission
                    .is_some_and(SubmissionStatus::is_sending)
            {
                Self::Sending
            } else if !matches!(
                activity,
                Activity::Stopping | Activity::Stopped | Activity::Interrupted | Activity::Failed
            ) && let Some(wait) = app.pane().waiting_kind(app.approvals())
            {
                Self::WaitingFor(wait)
            } else if activity == Activity::ToolArguments && app.pane().preparing_file_contents() {
                Self::PreparingFile
            } else if matches!(
                activity,
                Activity::Interrupted | Activity::Failed | Activity::Running
            ) && app.pane().model_failed()
            {
                Self::ModelFailed
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
            Self::Sending => "Sending",
            Self::PreparingFile => "Preparing file contents",
            Self::WaitingFor(WaitKind::Permission) => "Waiting for approval",
            Self::WaitingFor(WaitKind::Question) => "Waiting for answer",
            Self::ModelFailed => "Model request failed",
            Self::Compacting => "Compacting context",
            Self::Agent(Activity::WaitingForModel) => "Waiting for model",
            Self::Agent(Activity::Streaming) => "Replying",
            Self::Agent(Activity::ToolArguments) => "Preparing tool call",
            Self::Agent(Activity::RunningTools) => "Using tools",
            Self::Agent(Activity::Stopped) => "Turn stopped",
            Self::Agent(Activity::RetryingModel) => "Retrying",
            Self::Agent(activity) => activity.label(),
        }
    }

    fn short_label(self) -> &'static str {
        match self {
            Self::Agent(Activity::WaitingForModel) => "Model wait",
            Self::Agent(Activity::ToolArguments) => "Preparing call",
            Self::PreparingFile => "Preparing file",
            Self::WaitingFor(WaitKind::Permission) => "Need approval",
            Self::WaitingFor(WaitKind::Question) => "Need answer",
            Self::ModelFailed => "Model failed",
            Self::Agent(Activity::Stopped) => "Stopped",
            Self::Compacting => "Compacting",
            status => status.label(),
        }
    }

    fn color(self) -> Color {
        match self {
            Self::Service(ServiceStatus::Offline | ServiceStatus::UpdatesUnavailable)
            | Self::ModelFailed
            | Self::Agent(Activity::Interrupted | Activity::Failed) => ERROR,
            Self::Service(ServiceStatus::Reconnecting)
            | Self::WaitingFor(_)
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
            Self::PreparingFile => Some(Activity::ToolArguments),
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

    fn can_coalesce_into(self, observed: Self) -> bool {
        self.motion().is_some() && observed == Self::Agent(Activity::Running)
    }

    fn clock_runs(self) -> bool {
        self.motion().is_some()
            || matches!(
                self,
                Self::WaitingFor(_) | Self::Agent(Activity::Waiting | Activity::Stopping)
            )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolActivity {
    label: &'static str,
    target: String,
    waiting_job: bool,
}

impl ToolActivity {
    fn current(app: &App) -> Option<(&str, Self)> {
        let (id, name, target) = app.pane().running_tool()?;
        let (label, waiting_job) = match name {
            "wait_job" => ("Waiting for", true),
            "write_file" => ("Writing file", false),
            "edit_file" => ("Editing file", false),
            "read_file" => ("Reading file", false),
            "run_command" => ("Running command", false),
            "run_js" => ("Running script", false),
            _ => return None,
        };
        Some((
            id,
            Self {
                label,
                target: shorten_middle(&single_line(target), 72),
                waiting_job,
            },
        ))
    }

    fn label(&self, room: usize) -> String {
        if self.waiting_job {
            let target = if self.target.is_empty() {
                "job"
            } else {
                &self.target
            };
            let full = format!("{} {target}", self.label);
            let short = format!("Wait {target}");
            if full.width() <= room {
                full
            } else {
                shorten(&short, room)
            }
        } else if !self.target.is_empty() && room > self.label.width() + 5 {
            let target = shorten_middle(&self.target, room - self.label.width() - 3);
            format!("{} · {target}", self.label)
        } else {
            shorten(self.label, room)
        }
    }

    fn desired_width(&self) -> usize {
        if self.waiting_job {
            self.label.width() + 1 + self.target.width().max(3)
        } else {
            self.label.width() + 3 + self.target.width()
        }
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
    tool: Option<ToolActivity>,
}

impl ActivityFrame {
    pub(crate) fn phase_changed(&self, previous: &Self) -> bool {
        self.status != previous.status || self.tool != previous.tool
    }

    pub(crate) fn changes_with_time(&self) -> bool {
        (self.status.motion().is_some() || self.status == ObservedActivity::Sending)
            || (self.elapsed.is_some() && self.status.clock_runs())
    }

    #[cfg(test)]
    pub(super) fn test_frame(app: &App, step: Option<u128>) -> Self {
        let elapsed = app
            .pane()
            .turn_timing()
            .and_then(|timing| timing.elapsed_ms);
        let mut frame = Self::new(ObservedActivity::of(app), step, elapsed.map(|ms| ms / 1000));
        if frame.status == ObservedActivity::Agent(Activity::RunningTools) {
            frame.tool = ToolActivity::current(app).map(|(_, tool)| tool);
        }
        frame
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
            ObservedActivity::PreparingFile
            | ObservedActivity::Agent(Activity::ToolArguments | Activity::RunningTools) => {
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
            tool: None,
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
            tool: self.tool,
            ..Self::new(self.status, None, self.elapsed)
        }
    }

    pub(super) fn agent_activity(&self, app: &App) -> Activity {
        match self.status {
            ObservedActivity::Agent(activity) => activity,
            ObservedActivity::Service(_)
            | ObservedActivity::Sending
            | ObservedActivity::Compacting
            | ObservedActivity::PreparingFile
            | ObservedActivity::WaitingFor(_)
            | ObservedActivity::ModelFailed => app.pane().activity(),
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
    assert!(
        frame
            .thinking_line(64)
            .spans
            .iter()
            .all(|span| span.content.as_ref() != "18s")
    );
}

pub(super) fn render_heading(
    app: &App,
    width: usize,
    frame: ActivityFrame,
    run_control_visible: bool,
    delivery_visible: bool,
) -> (Line<'static>, Option<Range<usize>>) {
    let muted = Style::default().fg(MUTED);
    let gap = FIELD_GAP.len();
    let terminal_status = matches!(
        frame.status,
        ObservedActivity::ModelFailed
            | ObservedActivity::Agent(Activity::Stopped | Activity::Interrupted | Activity::Failed)
    );
    let next_action =
        if terminal_status && !delivery_visible && !run_control_visible && app.available() {
            match app.run_control() {
                Some(crate::contextual::ContextControl::Continue { .. }) => {
                    Some("F2 Continue".to_owned())
                }
                Some(crate::contextual::ContextControl::ReviewBlocked { .. }) => {
                    Some("Review unconfirmed tool".to_owned())
                }
                _ if matches!(
                    app.pane().activity(),
                    Activity::Stopped | Activity::Interrupted | Activity::Failed
                ) =>
                {
                    Some("Send a new message".to_owned())
                }
                _ => None,
            }
        } else {
            None
        };
    let phase_desired = frame
        .tool
        .as_ref()
        .map_or(PHASE_SLOT, |tool| {
            (tool.desired_width() + 3).clamp(PHASE_SLOT, PHASE_SLOT + 22)
        })
        .max(frame.status.label().width() + 3);
    let phase_desired = next_action.as_ref().map_or(phase_desired, |action| {
        phase_desired.max(frame.status.label().width() + 3 + 3 + action.width())
    });
    // A healthy send has no delivery row until committed or uncertain. Keep
    // its only feedback visible even when a busy recipient and a long tool
    // label would otherwise fill the heading.
    let send_feedback_width = app.displayed_statusline().composer_send_feedback_width(app);
    let feedback_reserve = if send_feedback_width > 0 {
        (send_feedback_width + 1).min(width.saturating_sub(4))
    } else {
        0
    };
    let core_room = width - feedback_reserve;
    // While submission feedback consumes its own slot, let the execution
    // phase shorten a little further to retain a usable recipient badge. A
    // 36-column terminal has only 31 cells inside the composer border: the
    // compact "Model wait" phase, recipient, and Sending all fit there.
    let minimum_phase = if send_feedback_width > 0 {
        MIN_PHASE_SLOT.min(core_room.saturating_sub(MIN_RECIPIENT_SLOT + gap).max(10))
    } else {
        MIN_PHASE_SLOT
    };
    let recipient_room =
        (width / 3)
            .clamp(MIN_RECIPIENT_SLOT, RECIPIENT_SLOT)
            .min(core_room.saturating_sub(
                if next_action.is_some() || frame.tool.as_ref().is_some_and(|tool| tool.waiting_job)
                {
                    phase_desired
                } else {
                    minimum_phase
                } + gap,
            ));
    let recipient = app
        .visible_recipient()
        .filter(|_| recipient_room >= MIN_RECIPIENT_SLOT);
    let recipient_width = recipient.map_or(0, |_| recipient_room + gap);
    let phase_width = (core_room - recipient_width).min(phase_desired);
    let core_width = phase_width + recipient_width;
    let feedback_limit = width
        .saturating_sub(core_width + 1)
        .min((width / 3).max(send_feedback_width))
        .min(FEEDBACK_LIMIT);
    let feedback =
        app.displayed_statusline()
            .composer_feedback(app, feedback_limit, delivery_visible);
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
    let status = if let Some(tool) = &frame.tool {
        tool.label(label_room)
    } else if let Some(action) = &next_action {
        let normal = format!("{label} · {action}");
        let short_action = if action == "Send a new message" {
            "Send message"
        } else if action.ends_with("F2 Continue") {
            "F2 Continue"
        } else {
            action
        };
        let short = format!("{} · {short_action}", frame.status.short_label());
        if normal.width() <= label_room {
            normal
        } else if short.width() <= label_room {
            short
        } else {
            // Narrow terminals prioritize the available operation, and keep the
            // complete reason on the transcript's interruption card.
            let status_room = label_room.saturating_sub(short_action.width() + 3);
            if status_room >= 5 {
                format!(
                    "{} · {short_action}",
                    shorten(frame.status.short_label(), status_room)
                )
            } else {
                shorten(&normal, label_room)
            }
        }
    } else {
        shorten(
            if label.width() <= label_room {
                label
            } else {
                frame.status.short_label()
            },
            label_room,
        )
    };
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
            ObservedActivity::Agent(_) | ObservedActivity::PreparingFile => {
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
            | ObservedActivity::PreparingFile
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
