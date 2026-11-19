use super::{App, Job, Modal, Shortcut};
use crossterm::event::KeyEvent;

impl App {
    pub(super) fn key(&mut self, key: KeyEvent) {
        self.dismiss_confirmation();
        self.text_selection.cancel_drag();
        if !Shortcut::ESCAPE.matches(key) {
            self.escape_armed_at = None;
        }
        if Shortcut::ESCAPE.matches(key) && self.modal.take().is_some() {
            self.rewind_restore_files = false;
            self.escape_armed_at = None;
            self.effects.push(Job::ClosePicker);
            return;
        }
        let address = self.active.clone();
        let draft = self.pane().draft.text().to_owned();
        match self.modal {
            Some(Modal::Picker(_)) => self.picker_key(key),
            Some(Modal::Prompt(_)) => self.prompt_key(key),
            Some(Modal::Output(_)) => self.output_key(key),
            Some(Modal::StatusLineEditor(_)) => self.statusline_key(key),
            None => self.composer_key(key),
        }
        if let Some(pane) = self.panes.get_mut(&address) {
            if pane.draft.text() != draft {
                pane.draft_revision = pane.draft_revision.wrapping_add(1);
            }
        }
    }

    pub(super) fn paste(&mut self, text: &str) {
        self.dismiss_confirmation();
        self.text_selection.cancel_drag();
        self.escape_armed_at = None;
        let address = self.active.clone();
        let draft = self.pane().draft.text().to_owned();
        if let Some(Modal::Picker(picker)) = &mut self.modal {
            if picker.kind.searchable() {
                picker.query.insert(&text.replace(['\r', '\n'], " "));
                picker.selected = 0;
                self.search(false);
            }
        } else if let Some(Modal::Prompt(prompt)) = &mut self.modal {
            if prompt.kind.multiline() {
                prompt.editor.insert(text);
            } else {
                prompt.editor.insert(&text.replace(['\r', '\n'], " "));
            }
        } else if self.modal.is_none() {
            self.focus_composer();
            self.pane_mut().draft.insert(text);
        }
        if let Some(pane) = self.panes.get_mut(&address) {
            if pane.draft.text() != draft {
                pane.draft_revision = pane.draft_revision.wrapping_add(1);
            }
        }
    }
}
