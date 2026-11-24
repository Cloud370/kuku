use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent};
use kuku_client::Observation;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use serde::Deserialize;

use crate::app::{App, Modal};
use crate::theme::{emphasis, ACCENT, MUTED};

const BELL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy)]
enum Sound {
    Question,
    Permission,
    Interrupted,
    Completed,
}

impl Sound {
    const ALL: [Self; 4] = [
        Self::Question,
        Self::Permission,
        Self::Interrupted,
        Self::Completed,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Question => "Agent question",
            Self::Permission => "Tool approval",
            Self::Interrupted => "Turn interrupted",
            Self::Completed => "Turn completed",
        }
    }

    fn for_observation(event: &Observation, last_event: u64) -> Option<Self> {
        let Observation::Committed {
            id,
            live_commit: true,
            kind,
            operation,
            ..
        } = event
        else {
            return None;
        };
        if *id <= last_event {
            return None;
        }
        match (kind.as_str(), operation.as_deref()) {
            ("runtime.operation", Some("question_requested")) => Some(Self::Question),
            ("runtime.operation", Some("permission_waiting")) => Some(Self::Permission),
            ("turn.interrupted", _) => Some(Self::Interrupted),
            ("turn.completed", _) => Some(Self::Completed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Notifications {
    pub(crate) question: bool,
    pub(crate) permission: bool,
    pub(crate) interrupted: bool,
    pub(crate) completed: bool,
}

impl Default for Notifications {
    fn default() -> Self {
        Self {
            question: true,
            permission: true,
            interrupted: true,
            completed: true,
        }
    }
}

impl Notifications {
    fn enabled(&self, sound: Sound) -> bool {
        match sound {
            Sound::Question => self.question,
            Sound::Permission => self.permission,
            Sound::Interrupted => self.interrupted,
            Sound::Completed => self.completed,
        }
    }

    fn toggle(&mut self, sound: Sound) {
        let enabled = match sound {
            Sound::Question => &mut self.question,
            Sound::Permission => &mut self.permission,
            Sound::Interrupted => &mut self.interrupted,
            Sound::Completed => &mut self.completed,
        };
        *enabled = !*enabled;
    }
}

#[derive(Debug, Default)]
pub(crate) struct TerminalBell {
    pending: bool,
    played_at: Option<Instant>,
}

impl TerminalBell {
    pub(crate) fn observe(
        &mut self,
        event: &Observation,
        last_event: u64,
        settings: Notifications,
    ) {
        self.pending |=
            Sound::for_observation(event, last_event).is_some_and(|sound| settings.enabled(sound));
    }

    pub(crate) fn take(&mut self, now: Instant) -> bool {
        let pending = std::mem::take(&mut self.pending);
        if pending
            && self
                .played_at
                .is_none_or(|at| now.duration_since(at) >= BELL_INTERVAL)
        {
            self.played_at = Some(now);
            true
        } else {
            false
        }
    }
}

#[derive(Debug)]
pub(crate) struct NotificationEditor {
    draft: Notifications,
    selected: usize,
    problem: Option<String>,
}

impl NotificationEditor {
    pub(crate) fn move_selection(&mut self, amount: isize) {
        self.selected = self
            .selected
            .saturating_add_signed(amount)
            .min(Sound::ALL.len() - 1);
    }

    pub(crate) fn render(&self, frame: &mut Frame<'_>, area: Rect) {
        let mut rows: Vec<_> = Sound::ALL
            .into_iter()
            .enumerate()
            .map(|(index, sound)| {
                let enabled = if self.draft.enabled(sound) {
                    "on"
                } else {
                    "off"
                };
                let selected = index == self.selected;
                let cursor = if selected { "›" } else { " " };
                let label = format!("{cursor} {:<18} {enabled}", sound.label());
                if selected {
                    Line::styled(label, emphasis(ACCENT))
                } else {
                    Line::raw(label)
                }
            })
            .collect();
        rows.push(Line::styled(
            "↑↓ Select · Space Toggle · Enter Save · Esc Close",
            emphasis(MUTED),
        ));
        if let Some(problem) = &self.problem {
            rows.push(Line::raw(problem.clone()));
        }
        frame.render_widget(Paragraph::new(rows), area);
    }
}

impl App {
    pub(crate) fn open_notifications(&mut self) {
        self.modal = Some(Modal::Notifications(NotificationEditor {
            draft: self.notification_settings.active(),
            selected: 0,
            problem: None,
        }));
    }

    pub(crate) fn notification_key(&mut self, key: KeyEvent) {
        let Some(Modal::Notifications(editor)) = self.modal.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Up => editor.move_selection(-1),
            KeyCode::Down => editor.move_selection(1),
            KeyCode::Char(' ') => editor.draft.toggle(Sound::ALL[editor.selected]),
            KeyCode::Enter => match self.notification_settings.save(editor.draft) {
                Ok(()) => {
                    self.modal = None;
                    self.confirmation("Terminal notifications saved");
                }
                Err(error) => editor.problem = Some(error),
            },
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn commit(id: u64, live: bool, kind: &str, operation: Option<&str>) -> Observation {
        serde_json::from_value(json!({
            "type": "committed", "id": id, "live_commit": live,
            "kind": kind, "operation": operation
        }))
        .unwrap()
    }

    #[test]
    fn each_new_event_rings_once_and_its_setting_can_disable_it() {
        let mut bell = TerminalBell::default();
        let start = Instant::now();
        for (index, (sound, kind, operation)) in [
            (
                Sound::Question,
                "runtime.operation",
                Some("question_requested"),
            ),
            (
                Sound::Permission,
                "runtime.operation",
                Some("permission_waiting"),
            ),
            (Sound::Interrupted, "turn.interrupted", None),
            (Sound::Completed, "turn.completed", None),
        ]
        .into_iter()
        .enumerate()
        {
            let id = index as u64 + 1;
            let time = start + Duration::from_secs(id);
            let mut settings = Notifications::default();
            let observed = commit(id, true, kind, operation);
            settings.toggle(sound);
            bell.observe(&observed, id - 1, settings);
            assert!(!bell.take(time));
            settings.toggle(sound);
            bell.observe(&observed, id - 1, settings);
            assert!(bell.take(time));
            bell.observe(&observed, id, settings);
            assert!(!bell.take(time + Duration::from_millis(500)));
        }
    }

    #[test]
    fn replay_user_cancellation_and_a_burst_are_quiet_after_the_first_bell() {
        let mut bell = TerminalBell::default();
        let now = Instant::now();
        let settings = Notifications::default();
        bell.observe(&commit(1, false, "turn.completed", None), 0, settings);
        bell.observe(&commit(2, true, "turn.cancelled", None), 1, settings);
        assert!(!bell.take(now));
        bell.observe(&commit(3, true, "turn.interrupted", None), 2, settings);
        assert!(bell.take(now));
        bell.observe(&commit(4, true, "turn.completed", None), 3, settings);
        assert!(!bell.take(now + Duration::from_millis(100)));
    }
}
