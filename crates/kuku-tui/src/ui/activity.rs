use std::time::{Duration, Instant};

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::app::{Activity, App, ServiceStatus};
use crate::text::{shorten, single_line};
use crate::theme::{ACCENT, ERROR, EXECUTION, KUKU_MARK, MUTED, WARNING};

const FRAME_INTERVAL: Duration = Duration::from_millis(160);
const WINGS: [&str; 8] = ["▖", "▚", "▙", "▛", "▘", "▞", "▗", "▚"];
const COMPACT_WIDTH: usize = 37;
const FEEDBACK_LIMIT: usize = 48;
const LABEL_SEPARATOR: &str = " · ";

#[derive(Default)]
pub(crate) struct ActivityClock {
    started: Option<Instant>,
}

impl ActivityClock {
    pub(crate) fn frame_at(&mut self, now: Instant, active: bool) -> Option<usize> {
        if !active {
            self.started = None;
            return None;
        }
        let started = *self.started.get_or_insert(now);
        let step = now.duration_since(started).as_millis() / FRAME_INTERVAL.as_millis();
        Some((step % WINGS.len() as u128) as usize)
    }
}

struct ObservedActivity {
    label: &'static str,
    color: Color,
    moving: bool,
}

impl ObservedActivity {
    fn of(app: &App) -> Self {
        let service = app.service_status();
        let service_color = match service {
            ServiceStatus::Offline | ServiceStatus::UpdatesUnavailable => Some(ERROR),
            ServiceStatus::Connecting | ServiceStatus::Reconnecting | ServiceStatus::Restarting => {
                Some(MUTED)
            }
            ServiceStatus::Connected => None,
        };
        if let Some(color) = service_color {
            return Self {
                label: service.label(),
                color,
                moving: false,
            };
        }
        let activity = app.pane().activity();
        let (color, moving) = match activity {
            Activity::Running
            | Activity::WaitingForModel
            | Activity::RetryingModel
            | Activity::Thinking
            | Activity::Streaming
            | Activity::RunningTools
            | Activity::Stopping => (EXECUTION, true),
            Activity::Waiting | Activity::Configuration | Activity::Stopped => (WARNING, false),
            Activity::Interrupted | Activity::Failed => (ERROR, false),
            Activity::Ready | Activity::Opening | Activity::Finished => (MUTED, false),
        };
        Self {
            label: activity.label(),
            color,
            moving,
        }
    }
}

pub(crate) fn animation_active(app: &App) -> bool {
    ObservedActivity::of(app).moving
}

pub(super) fn render_heading(app: &App, width: usize, step: Option<usize>) -> Line<'static> {
    let activity = ObservedActivity::of(app);
    let compact = width < COMPACT_WIDTH;
    let mut line = mark(&activity, compact, step);
    let left_min = line.width() + 1 + activity.label.width();
    let feedback_width = width
        .saturating_sub(left_min + 1)
        .min(width / 3)
        .min(FEEDBACK_LIMIT);
    let left_width = width - feedback_width - usize::from(feedback_width > 0);
    if line.width() < left_width {
        line.push_span(" ");
    }
    let recipient = format!("To {}", single_line(&app.active.agent));
    let separator = if compact { " " } else { LABEL_SEPARATOR };
    let recipient_width =
        left_width.saturating_sub(line.width() + activity.label.width() + separator.width());
    let show_recipient = recipient_width >= "To x".width();
    let recipient = Span::styled(
        shorten(&recipient, recipient_width),
        Style::default().fg(MUTED),
    );
    let separator = Span::styled(separator, Style::default().fg(MUTED));
    let status = Span::styled(
        shorten(activity.label, left_width.saturating_sub(line.width())),
        Style::default().fg(activity.color),
    );
    match (compact, show_recipient) {
        (true, true) => line.extend([status, separator, recipient]),
        (false, true) => line.extend([recipient, separator, status]),
        (_, false) => line.push_span(status),
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

fn mark(activity: &ObservedActivity, compact: bool, step: Option<usize>) -> Line<'static> {
    let [rest, head] = KUKU_MARK;
    let wing = step
        .filter(|_| activity.moving)
        .map_or(rest, |step| WINGS[step]);
    let color = if matches!(activity.color, WARNING | ERROR) {
        activity.color
    } else {
        ACCENT
    };
    let style = Style::default().fg(color);
    let mut line = Line::from(Span::styled(wing, style));
    if !compact {
        line.push_span(Span::styled(head, style));
    }
    line
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{ActivityClock, FRAME_INTERVAL, WINGS};

    #[test]
    fn clock_skips_delayed_frames_and_resets_after_hidden_or_idle() {
        let mut clock = ActivityClock::default();
        let now = Instant::now();
        assert_eq!(None, clock.frame_at(now, false));
        assert_eq!(Some(0), clock.frame_at(now, true));
        assert_eq!(
            Some(0),
            clock.frame_at(now + Duration::from_millis(100), true)
        );
        assert_eq!(Some(3), clock.frame_at(now + FRAME_INTERVAL * 3, true));
        assert_eq!(
            Some(0),
            clock.frame_at(now + FRAME_INTERVAL * WINGS.len() as u32, true)
        );
        assert_eq!(None, clock.frame_at(now + Duration::from_secs(3), false));
        assert_eq!(Some(0), clock.frame_at(now + Duration::from_secs(10), true));
    }
}
