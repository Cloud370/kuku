use crossterm::event::KeyEvent;
use kuku_client::Action;

use crate::app::App;
use crate::contextual::ContextControl;
use crate::keys::Shortcut;
use crate::menu::PickerKind;

impl App {
    pub(super) fn contextual_key(&mut self, key: KeyEvent) -> bool {
        if Shortcut::REPLY.matches(key) {
            return self.cycle_context_control();
        }
        if !self.pane().contextual_focus.active {
            return false;
        }
        if Shortcut::ESCAPE.matches(key) {
            self.deactivate_context_focus();
            self.focus_composer();
            return true;
        }
        if Shortcut::ENTER.matches(key) {
            self.activate_context_control();
            return true;
        }
        if matches!(key.code, crossterm::event::KeyCode::Char(_)) && super::is_text(key)
            || Shortcut::NEW_LINE.matches(key)
            || Shortcut::CONTROL_NEW_LINE.matches(key)
        {
            self.deactivate_context_focus();
            self.focus_composer();
            return false;
        }
        self.deactivate_context_focus();
        false
    }

    pub(crate) fn focus_context_control(&mut self, control: ContextControl) -> bool {
        if !self.contextual_controls().contains(&control) {
            self.problem("This contextual control is no longer available");
            return false;
        }
        if let ContextControl::Request { target, request_id } = &control {
            if !self.focus_request_id(target.clone(), request_id.clone()) {
                return false;
            }
            self.pane_mut().contextual_focus.cursor = Some(control);
            self.pane_mut().contextual_focus.active = false;
            return true;
        }
        self.focus_composer();
        let pane = self.pane_mut();
        pane.contextual_focus.cursor = Some(control);
        pane.contextual_focus.active = true;
        true
    }

    pub(crate) fn deactivate_context_focus(&mut self) {
        self.pane_mut().contextual_focus.active = false;
    }

    fn cycle_context_control(&mut self) -> bool {
        let controls = self.contextual_controls();
        if controls.is_empty() {
            let focus = &mut self.pane_mut().contextual_focus;
            focus.cursor = None;
            focus.active = false;
            return false;
        }
        let current = self.pane().contextual_focus.cursor.as_ref();
        let index = current
            .and_then(|current| controls.iter().position(|control| control == current))
            .map_or(0, |index| (index + 1) % controls.len());
        self.focus_context_control(controls[index].clone())
    }

    fn activate_context_control(&mut self) {
        let Some(control) = self.pane().contextual_focus.cursor.clone() else {
            self.deactivate_context_focus();
            return;
        };
        if !self.contextual_controls().contains(&control) {
            self.deactivate_context_focus();
            self.problem("This contextual control is no longer available");
            return;
        }
        self.deactivate_context_focus();
        match control {
            ContextControl::Request { target, request_id } => {
                self.focus_request_id(target, request_id);
            }
            ContextControl::Continue { .. } => {
                self.issue(self.active.clone(), Action::Advance);
            }
            ContextControl::Interrupt { .. } => self.interrupt(),
            ContextControl::PendingRecovery { .. } => {
                self.picker(PickerKind::Pending, self.pending_choices());
            }
            ContextControl::PreviousAgent { .. } => self.back_to_agent(),
            ContextControl::Reconnect { .. } => self.reconnect(),
            ContextControl::Restart { .. } => self.restart(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    #[test]
    fn context_focus_keeps_the_cursor_when_escape_returns_to_composer() {
        let mut app = crate::interaction::tests::connected_app();
        let control = app
            .contextual_controls()
            .into_iter()
            .find(|control| matches!(control, ContextControl::Restart { .. }))
            .unwrap();
        assert!(app.focus_context_control(control.clone()));
        assert!(app.pane().contextual_focus.active);
        assert!(app.contextual_key(KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            KeyModifiers::NONE
        )));
        assert!(!app.pane().contextual_focus.active);
        assert_eq!(Some(&control), app.pane().contextual_focus.cursor.as_ref());
    }

    #[test]
    fn enter_activates_focused_service_control_without_sending_the_draft() {
        let mut app = crate::interaction::tests::connected_app();
        app.pane_mut().draft.insert("unsent".into());
        app.key(KeyEvent::new(
            crossterm::event::KeyCode::F(2),
            KeyModifiers::NONE,
        ));
        assert!(app.pane().contextual_focus.active);
        assert_eq!("unsent", app.pane().draft.text());
        app.key(KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            KeyModifiers::NONE,
        ));
        assert!(app.restarting);
        assert_eq!("unsent", app.pane().draft.text());
    }

    #[test]
    fn stale_service_focus_cannot_restart_twice() {
        let mut app = crate::interaction::tests::connected_app();
        let control = app
            .contextual_controls()
            .into_iter()
            .find(|control| matches!(control, ContextControl::Restart { .. }))
            .unwrap();
        assert!(app.focus_context_control(control));
        app.restarting = true;
        app.key(KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            KeyModifiers::NONE,
        ));
        assert!(app.effects.is_empty());
    }
}
