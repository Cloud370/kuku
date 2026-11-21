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

const PHASE_SLOT: usize = 14;
const RECIPIENT_SLOT: usize = 30;
const TOOLS_SLOT: usize = 4;
const METER_SLOT: usize = 11;
const FEEDBACK_LIMIT: usize = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObservedActivity {
    Service(ServiceStatus),
    Agent(Activity),
    Sending,
}

impl ObservedActivity {
    fn of(app: &App) -> Self {
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
            Self::Agent(Activity::WaitingForModel) => "Waiting",
            Self::Agent(Activity::Streaming) => "Reply",
            Self::Agent(Activity::ToolArguments) => "Tool call",
            Self::Agent(Activity::RunningTools) => "Tools",
            Self::Agent(Activity::RetryingModel) => "Retrying",
            Self::Agent(activity) => activity.label(),
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
        }
    }

    pub(super) fn thinking_line(&self, width: usize) -> Line<'static> {
        let mut line = motion::thinking_label(&shorten("Thinking", width), self.text_step);
        let room = width.saturating_sub(line.width() + 2);
        if room >= 4 && !self.thinking.is_empty() {
            line.push_span(Span::styled(
                format!("  {}", motion::flow_excerpt(&self.thinking, room)),
                Style::new().fg(MUTED),
            ));
        }
        line
    }

    pub(super) fn still(self) -> Self {
        Self::new(self.status, None, self.elapsed)
    }

    pub(super) fn agent_activity(&self, app: &App) -> Activity {
        match self.status {
            ObservedActivity::Agent(activity) => activity,
            ObservedActivity::Service(_) | ObservedActivity::Sending => app.pane().activity(),
        }
    }
}

pub(super) fn render_heading(
    app: &App,
    width: usize,
    frame: ActivityFrame,
) -> (Line<'static>, Option<Range<usize>>) {
    let Some(name) = app.visible_recipient().filter(|_| width > 0) else {
        return (render_activity(app, width, frame), None);
    };

    let room = if width >= 24 {
        (width - PHASE_SLOT).min(RECIPIENT_SLOT)
    } else {
        width
    };
    let (prefix, suffix) = match room {
        16.. => ("Send to ", " ▾"),
        8..=15 => ("To ", " ▾"),
        _ => ("", ""),
    };
    let name = shorten_middle(&single_line(name), room - (prefix.width() + suffix.width()));
    let label = format!("{prefix}{name}{suffix}");
    let mut line = Line::styled(
        label,
        emphasis(if app.is_subagent() { MODEL } else { ACCENT }),
    );
    let span = 0..line.width();
    if width > room + 2 {
        line.push_span(" ".repeat(room + 2 - span.end));
        line.spans
            .extend(render_activity(app, width - room - 2, frame).spans);
    }
    (line, Some(span))
}

fn render_activity(app: &App, width: usize, frame: ActivityFrame) -> Line<'static> {
    let muted = Style::default().fg(MUTED);
    let feedback_limit = width
        .saturating_sub(PHASE_SLOT + 1)
        .min(width / 3)
        .min(FEEDBACK_LIMIT);
    let feedback = app
        .displayed_statusline()
        .composer_feedback(app, feedback_limit);
    let left_width = width - feedback.as_ref().map_or(0, |feedback| feedback.width() + 1);
    let render_metrics = left_width >= PHASE_SLOT + TOOLS_SLOT + METER_SLOT;
    let phase_width = if render_metrics {
        PHASE_SLOT
    } else {
        left_width
    };
    let mut line = frame.ring.line(phase_width);
    if phase_width > line.width() {
        line.push_span(" ");
    }
    let status = shorten(
        frame.status.label(),
        phase_width.saturating_sub(line.width()),
    );
    line.push_span(Span::styled(status, Style::new().fg(frame.status.color())));

    if render_metrics {
        line.push_span(" ".repeat(PHASE_SLOT - line.width()));
        let activity = frame.agent_activity(app);
        let parallel = matches!(
            frame.status,
            ObservedActivity::Agent(
                Activity::Streaming | Activity::Thinking | Activity::ToolArguments
            )
        );
        let count = app.pane().parallel_tools();
        let tools = if parallel && count != 0 {
            format!("↳{}", count.min(999))
        } else {
            String::new()
        };
        line.push_span(Span::styled(
            format!("{tools:<TOOLS_SLOT$}"),
            Style::new().fg(crate::theme::USAGE),
        ));
        let tokens = match frame.status {
            ObservedActivity::Agent(_) => app.pane().composer_output_tokens(activity),
            _ => None,
        };
        let metric = match tokens {
            Some(OutputTokens::Reported(count)) => format!("{} tok", compact_token_count(count)),
            Some(OutputTokens::Estimated(count)) => format!("~{} tok", compact_token_count(count)),
            None => frame
                .elapsed
                .filter(|_| frame.status.clock_runs())
                .map_or_else(String::new, |seconds| {
                    format_elapsed(Duration::from_secs(seconds))
                }),
        };
        let metric = shorten(&metric, METER_SLOT);
        line.push_span(Span::styled(format!("{metric:<METER_SLOT$}"), muted));
    }

    let key = Shortcut::CYCLE_THINKING.label;
    let action = " think";
    let key_width = 2 + key.width() + action.width();
    let thinking = app.modal.is_none()
        && app.available()
        && app.pane().effective_reasoning().is_some_and(|(_, caps)| {
            caps.selectable_reasoning_efforts()
                .any(|level| level != ThinkLevel::ProviderDefault.as_str())
        });
    if thinking && left_width.saturating_sub(line.width()) >= key_width {
        line.push_span("  ");
        line.push_span(Span::styled(key, Style::new().fg(SURFACE_TEXT)));
        line.push_span(Span::styled(action, muted));
    }
    if let Some(feedback) = feedback {
        line.push_span(" ".repeat(width - line.width() - feedback.width()));
        line.push_span(feedback);
    }
    line
}
