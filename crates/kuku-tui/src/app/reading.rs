use crossterm::event::KeyEvent;

use crate::jobs::Job;
use crate::keys::Shortcut;

use super::App;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum KeyboardFocus {
    #[default]
    Composer,
    Transcript,
}

impl App {
    pub(crate) fn composer_focused(&self) -> bool {
        let pane = self.pane();
        pane.keyboard_focus == KeyboardFocus::Composer || !pane.transcript.is_reading()
    }

    pub(crate) fn toggle_transcript_detail(&mut self, key: &crate::transcript::DetailKey) -> bool {
        if let Some(notes) = self.pane().transcript.thinking_text(key) {
            self.show_output(notes);
            return true;
        }
        let (width, height) = self.transcript_area;
        let pane = self.pane_mut();
        let changed = pane.transcript.toggle_in_view(key, width, height);
        if changed && pane.transcript.is_reading() {
            pane.keyboard_focus = KeyboardFocus::Transcript;
        }
        changed
    }

    pub(crate) fn focus_composer(&mut self) {
        self.pane_mut().keyboard_focus = KeyboardFocus::Composer;
    }

    pub(crate) fn jump_latest(&mut self) {
        self.pane_mut().transcript.show_latest();
        self.focus_composer();
    }

    pub(crate) fn reading_key(&mut self, key: KeyEvent) -> bool {
        if Shortcut::LATEST.matches(key)
            || Shortcut::READING_BACK.matches(key) && self.pane().transcript.is_reading()
        {
            self.jump_latest();
            return true;
        }
        let page = self.transcript_area.1.saturating_sub(1).max(1) as isize;
        let amount = if Shortcut::PAGE_UP.matches(key) {
            -page
        } else if Shortcut::PAGE_DOWN.matches(key) {
            page
        } else if !self.composer_focused() && Shortcut::UP.matches(key) {
            -1
        } else if !self.composer_focused() && Shortcut::DOWN.matches(key) {
            1
        } else {
            return false;
        };
        self.scroll(amount);
        true
    }

    pub(crate) fn scroll(&mut self, amount: isize) {
        let (width, height) = self.transcript_area;
        let pane = self.pane_mut();
        let request = pane.transcript.scroll(amount, width, height);
        pane.keyboard_focus = if pane.transcript.is_reading() {
            KeyboardFocus::Transcript
        } else {
            KeyboardFocus::Composer
        };
        if let Some(request) = request {
            if self.pane().history_in_flight.is_none() && self.require_service() {
                self.pane_mut().history_in_flight = Some(request);
                self.effects.push(Job::History(self.selection(), request));
            }
        }
    }
}
