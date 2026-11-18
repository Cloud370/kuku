use std::time::Duration;

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::app::{Activity, App, ServiceStatus};
use crate::text::{format_elapsed, shorten, shorten_middle, single_line};
use crate::theme::{emphasis, ACCENT, ERROR, EXECUTION, MUTED, SURFACE_TEXT, WARNING};

mod clock;
mod motion;

pub(crate) use clock::ActivityClock;
use motion::Ring;

const COMPACT_WIDTH: usize = 37;
const WIDE_WIDTH: usize = 66;
const PHASE_SLOT: usize = 22;
const TIME_SLOT: usize = 12;
const FEEDBACK_LIMIT: usize = 48;
const RECIPIENT_LIMIT: usize = 28;
const SEPARATOR: &str = " · ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObservedActivity {
    Service(ServiceStatus),
    Agent(Activity),
}

impl ObservedActivity {
    fn of(app: &App) -> Self {
        let service = app.service_status();
        if service == ServiceStatus::Connected {
            Self::Agent(app.pane().activity())
        } else {
            Self::Service(service)
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Service(service) => service.label(),
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
        let ring = status.motion().zip(step).map_or_else(
            || Ring::resting(tone),
            |(phase, step)| {
                if phase == Activity::Thinking {
                    Ring::resting(MUTED)
                } else {
                    Ring::moving(phase, step)
                }
            },
        );
        Self {
            status,
            ring,
            text_step: step,
            thinking: String::new(),
            elapsed,
        }
    }

    pub(super) fn still(self) -> Self {
        Self::new(self.status, None, self.elapsed)
    }

    pub(super) fn agent_activity(&self, app: &App) -> Activity {
        match self.status {
            ObservedActivity::Agent(activity) => activity,
            ObservedActivity::Service(_) => app.pane().activity(),
        }
    }
}

pub(super) fn render_heading(app: &App, width: usize, frame: ActivityFrame) -> Line<'static> {
    let compact = width < COMPACT_WIDTH;
    let wide = width >= WIDE_WIDTH;
    let mut line = frame.ring.line(width);
    let muted = Style::default().fg(MUTED);
    let name = single_line(&app.active.agent);
    let separator = if compact { " " } else { SEPARATOR };
    let fixed_width = line.width() + " To ".width() + separator.width();
    let reserved = if wide { PHASE_SLOT + TIME_SLOT + 2 } else { 0 };
    let name_width = (width / 3).clamp(4, RECIPIENT_LIMIT);
    let name_width = name_width.min(width.saturating_sub(fixed_width + reserved));
    if name_width > 0 {
        line.push_span(Span::styled(" To ", muted));
        line.push_span(Span::styled(
            shorten_middle(&name, name_width),
            emphasis(SURFACE_TEXT),
        ));
    }
    if line.width() + separator.width() < width {
        line.push_span(Span::styled(separator, muted));
    }
    let label = frame.status.label();
    let elapsed = frame
        .elapsed
        .map(|seconds| format_elapsed(Duration::from_secs(seconds)));
    let elapsed = elapsed.filter(|text| {
        wide || line.width() + label.width() + text.width() + 2 <= width.saturating_sub(6)
    });
    let time_space = if wide {
        TIME_SLOT + 2
    } else {
        elapsed.as_ref().map_or(0, |text| text.width() + 2)
    };
    let phase_space = if wide { PHASE_SLOT } else { label.width() };
    let feedback_width = width
        .saturating_sub(line.width() + phase_space + time_space + 1)
        .min(width / 3)
        .min(FEEDBACK_LIMIT);
    let left_width = width - feedback_width - usize::from(feedback_width > 0);
    let status = shorten(label, left_width.saturating_sub(line.width()));
    let thinking = frame.status == ObservedActivity::Agent(Activity::Thinking);
    if thinking {
        line.extend(motion::thinking_label(&status, frame.text_step).spans);
    } else {
        line.push_span(Span::styled(status, Style::new().fg(frame.status.color())));
    }
    if let Some(elapsed) = elapsed {
        line.push_span(" ");
        line.push_span(Span::styled(shorten(&elapsed, TIME_SLOT), muted));
    }
    let room = left_width.saturating_sub(line.width() + 3);
    if thinking && !frame.thinking.is_empty() && room >= 12 {
        line.push_span(Span::styled("   ", muted));
        line.push_span(Span::styled(
            motion::flow_excerpt(&frame.thinking, room).to_owned(),
            muted,
        ));
    }
    if let Some(feedback) = app
        .displayed_statusline()
        .composer_feedback(app, feedback_width)
    {
        line.push_span(" ".repeat(width - line.width() - feedback.width()));
        line.push_span(feedback);
    }
    line
}
