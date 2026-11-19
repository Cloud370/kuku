use crossterm::event::{KeyCode, KeyEvent};
use kuku_client::{Action, Target};
use ratatui::style::Style;

use crate::app::{App, KeyboardFocus, PendingStage};
use crate::keys::Shortcut;
use crate::markup::{render_literal, wrap_bounded};
use crate::text::single_line;

impl App {
    pub(super) fn inline_key(&mut self, key: KeyEvent) -> bool {
        match self.pane().keyboard_focus {
            KeyboardFocus::Queue => self.queue_key(key),
            KeyboardFocus::Request => self.request_key(key),
            KeyboardFocus::Composer | KeyboardFocus::Transcript => false,
        }
    }

    pub(crate) fn focus_queued(&mut self) {
        if let Some(id) = self
            .queued_for_edit()
            .last()
            .map(|entry| entry.receipt.command_id.clone())
        {
            let pane = self.pane_mut();
            pane.selected_queue_input = Some(id);
            pane.keyboard_focus = KeyboardFocus::Queue;
        }
    }

    pub(crate) fn focus_queue_input(&mut self, id: String) {
        if self
            .queued_for_edit()
            .any(|entry| entry.receipt.command_id == id)
        {
            let pane = self.pane_mut();
            pane.selected_queue_input = Some(id);
            pane.keyboard_focus = KeyboardFocus::Queue;
        } else {
            self.problem("Selected input is no longer queued");
        }
    }

    pub(crate) fn focus_request(&mut self) {
        let Some(id) = self.pending_request_id() else {
            self.problem("No request is waiting for this Agent");
            return;
        };
        self.activate_request_id(self.selection().target, id);
    }

    pub(crate) fn activate_request_id(&mut self, target: Target, request_id: String) -> bool {
        if !self.focus_request_id(target, request_id.clone()) {
            return false;
        }
        if self.pane().snapshot.as_ref().is_some_and(|snapshot| {
            snapshot
                .agent
                .question
                .as_ref()
                .is_some_and(|request| request.id == request_id)
        }) {
            self.answer_question();
        }
        true
    }

    pub(crate) fn focus_request_id(&mut self, target: Target, request_id: String) -> bool {
        let current = self.pending_request_id();
        if self.selection().target != target || current.as_deref() != Some(request_id.as_str()) {
            self.problem("This request is no longer waiting for this Agent");
            return false;
        }
        let pane = self.pane_mut();
        pane.focused_request = Some(request_id);
        pane.permission_choice = None;
        pane.request_scroll = 0;
        pane.selected_queue_input = None;
        pane.keyboard_focus = KeyboardFocus::Request;
        true
    }

    fn pending_request_id(&self) -> Option<String> {
        let snapshot = self.pane().snapshot.as_ref()?;
        snapshot
            .agent
            .question
            .as_ref()
            .map(|request| request.id.clone())
            .or_else(|| {
                snapshot
                    .agent
                    .permission
                    .as_ref()
                    .map(|request| request.id.clone())
            })
    }

    fn queue_key(&mut self, key: KeyEvent) -> bool {
        if Shortcut::ESCAPE.matches(key) {
            let pane = self.pane_mut();
            pane.keyboard_focus = KeyboardFocus::Composer;
            pane.selected_queue_input = None;
            return true;
        }
        if Shortcut::UP.matches(key) || Shortcut::DOWN.matches(key) {
            let direction = if Shortcut::UP.matches(key) { -1 } else { 1 };
            let ids = self
                .queued_for_edit()
                .map(|entry| entry.receipt.command_id.clone())
                .collect::<Vec<_>>();
            let current = self.pane().selected_queue_input.as_ref();
            let next = current
                .and_then(|id| ids.iter().position(|candidate| candidate == id))
                .and_then(|index| index.checked_add_signed(direction))
                .and_then(|index| ids.get(index).cloned());
            if current.is_some_and(|id| !ids.contains(id)) {
                self.problem("Selected input is no longer queued");
                return true;
            }
            if let Some(id) = next {
                self.pane_mut().selected_queue_input = Some(id);
            }
            return true;
        }
        if Shortcut::ENTER.matches(key) || key.code == KeyCode::Delete {
            let Some(id) = self.pane().selected_queue_input.clone() else {
                self.pane_mut().keyboard_focus = KeyboardFocus::Composer;
                return true;
            };
            if !self
                .queued_for_edit()
                .any(|entry| entry.receipt.command_id == id)
            {
                let pane = self.pane_mut();
                pane.keyboard_focus = KeyboardFocus::Composer;
                pane.selected_queue_input = None;
                self.problem("Selected input is no longer queued");
                return true;
            }
            if self.queue_mutation_pending(&id) {
                self.problem("A change to this queued input is already pending");
                return true;
            }
            if Shortcut::ENTER.matches(key) {
                self.edit_queued(id);
            } else {
                self.withdraw_queued(id);
            }
            return true;
        }
        false
    }

    fn request_key(&mut self, key: KeyEvent) -> bool {
        if Shortcut::ESCAPE.matches(key) {
            let pane = self.pane_mut();
            pane.keyboard_focus = KeyboardFocus::Composer;
            pane.selected_queue_input = None;
            return true;
        }
        let focused = self.pane().focused_request.clone();
        let current = self.pending_request_id();
        if focused.is_none() || focused != current {
            self.pane_mut().keyboard_focus = KeyboardFocus::Composer;
            self.problem("This request is no longer waiting for this Agent");
            return true;
        }
        if self
            .pane()
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.agent.question.is_some())
        {
            if Shortcut::ENTER.matches(key) {
                self.answer_question();
            }
            return true;
        }
        if key.code == KeyCode::PageUp {
            let page = self.pane().request_detail_page_size.max(1);
            let end = self.request_scroll_end(page);
            let pane = self.pane_mut();
            pane.request_scroll = pane.request_scroll.saturating_sub(page).min(end);
            return true;
        }
        if key.code == KeyCode::PageDown {
            let page = self.pane().request_detail_page_size.max(1);
            let end = self.request_scroll_end(page);
            let pane = self.pane_mut();
            pane.request_scroll = pane.request_scroll.saturating_add(page).min(end);
            return true;
        }
        if key.code == KeyCode::Left || Shortcut::UP.matches(key) {
            self.pane_mut().permission_choice = Some(false);
        } else if key.code == KeyCode::Right || Shortcut::DOWN.matches(key) {
            self.pane_mut().permission_choice = Some(true);
        } else if Shortcut::ENTER.matches(key) {
            let Some(allow) = self.pane().permission_choice else {
                return true;
            };
            let request_id = focused.expect("validated request");
            self.issue(
                self.active.clone(),
                Action::ReplyPermission { request_id, allow },
            );
            self.pane_mut().keyboard_focus = KeyboardFocus::Composer;
        }
        true
    }

    fn request_scroll_end(&self, page: usize) -> usize {
        let Some(request) = self
            .pane()
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.agent.permission.as_ref())
        else {
            return 0;
        };
        let details = format!(
            "Summary: {}\nCandidate:\n{}",
            single_line(&request.summary),
            request.candidate
        );
        let total = wrap_bounded(
            render_literal(&details, Style::default()),
            usize::from(self.request_area.width).max(1),
            usize::MAX,
        )
        .len();
        total.saturating_sub(page)
    }

    pub(crate) fn queue_mutation_pending(&self, input_id: &str) -> bool {
        let target = self.selection().target;
        self.pending.values().any(|item| {
            item.command.target == target
                && !matches!(item.stage, PendingStage::Rejected(_))
                && matches!(
                    &item.command.action,
                    Action::ReplaceQueued { input_command_id, .. }
                        | Action::Withdraw { input_command_id }
                        if input_command_id == input_id
                )
        })
    }

    pub(crate) fn withdraw_queued(&mut self, input_id: String) {
        let text = self
            .queued_for_edit()
            .find(|entry| entry.receipt.command_id == input_id)
            .map(|entry| entry.input.text.clone());
        let Some(text) = text else {
            self.problem("Selected input is no longer queued");
            return;
        };
        if self.queue_mutation_pending(&input_id) {
            self.problem("A change to this queued input is already pending");
            return;
        }
        let restore_text = self.pane().draft.text().is_empty().then_some(text);
        let target = self.selection().target;
        let pending_before = self.pending.len();
        self.issue(
            self.active.clone(),
            Action::Withdraw {
                input_command_id: input_id.clone(),
            },
        );
        if self.pending.len() > pending_before {
            let key = (target, input_id);
            self.withdrawal_drafts.remove(&key);
            if let Some(text) = restore_text {
                self.withdrawal_drafts.insert(key, text);
            }
        }
        let pane = self.pane_mut();
        pane.keyboard_focus = KeyboardFocus::Composer;
        pane.selected_queue_input = None;
    }
}
