use std::cell::Cell;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use kuku_client::{Action, Error, PendingApproval, PermissionMode};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use crate::jobs::{Job, Selection};
use crate::markup::{render_literal, wrap_bounded};
use crate::theme::{ACCENT, MUTED, emphasis};

use super::{Address, App, Modal};

pub(crate) const MODES: [PermissionMode; 3] = [
    PermissionMode::Full,
    PermissionMode::Ask,
    PermissionMode::ReadOnly,
];

pub(crate) fn mode_name(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Full => "Full access",
        PermissionMode::Ask => "Ask",
        PermissionMode::ReadOnly => "Read-only",
    }
}

pub(crate) fn parse_mode(name: &str) -> Option<PermissionMode> {
    match name {
        "full" => Some(PermissionMode::Full),
        "ask" => Some(PermissionMode::Ask),
        "read-only" => Some(PermissionMode::ReadOnly),
        _ => None,
    }
}

#[derive(Debug)]
pub(crate) struct PermissionEditor {
    selected: usize,
}

impl PermissionEditor {
    pub(crate) fn move_selection(&mut self, amount: isize) {
        self.selected = self
            .selected
            .saturating_add_signed(amount)
            .min(MODES.len() - 1);
    }

    pub(crate) fn render(&self, frame: &mut Frame<'_>, area: Rect, saved: Option<PermissionMode>) {
        let mut lines = vec![Line::raw("Applies to every Session in this workspace.")];
        for (index, mode) in MODES.into_iter().enumerate() {
            let cursor = if index == self.selected { "›" } else { " " };
            let current = if saved == Some(mode) {
                " · current"
            } else {
                ""
            };
            let line = format!("{cursor} {}{current}", mode_name(mode));
            lines.push(if index == self.selected {
                Line::styled(line, emphasis(ACCENT))
            } else {
                Line::raw(line)
            });
        }
        lines.push(Line::styled(
            "↑↓ Select · Enter Apply · Esc Close",
            emphasis(MUTED),
        ));
        frame.render_widget(Paragraph::new(lines), area);
    }
}

impl App {
    pub(crate) fn open_permissions(&mut self) {
        if !self.require_service() {
            return;
        }
        let mode = self.permission_mode.unwrap_or_default();
        let selected = MODES.iter().position(|&item| item == mode).unwrap_or(0);
        self.modal = Some(Modal::Permissions(PermissionEditor { selected }));
    }

    pub(crate) fn permission_mode_key(&mut self, key: KeyEvent) {
        let Some(Modal::Permissions(editor)) = self.modal.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Up => editor.move_selection(-1),
            KeyCode::Down => editor.move_selection(1),
            KeyCode::Enter => {
                let mode = MODES[editor.selected];
                self.set_permission_mode(mode);
            }
            _ => {}
        }
    }

    pub(crate) fn set_permission_mode(&mut self, mode: PermissionMode) {
        if !self.require_service() || self.permission_mode_saving {
            return;
        }
        self.permission_mode_saving = true;
        self.permission_mode_ticket = self.permission_mode_ticket.wrapping_add(1);
        self.modal = None;
        self.effects.push(Job::WorkspacePermission {
            ticket: self.permission_mode_ticket,
            workspace: self.workspace.clone(),
            mode: Some(mode),
        });
    }

    pub(super) fn refresh_workspace_permission(&mut self) {
        self.permission_mode_ticket = self.permission_mode_ticket.wrapping_add(1);
        self.permission_mode_loading = true;
        self.effects.push(Job::WorkspacePermission {
            ticket: self.permission_mode_ticket,
            workspace: self.workspace.clone(),
            mode: None,
        });
    }

    pub(super) fn observe_workspace_permission(&mut self, mode: Option<PermissionMode>) {
        if self.permission_mode_loading || self.permission_mode_saving {
            self.permission_mode_refresh_pending = true;
        } else {
            self.permission_mode = mode;
        }
    }

    pub(super) fn workspace_permission_loaded(
        &mut self,
        saving: bool,
        result: Result<Option<PermissionMode>, Error>,
    ) {
        if !saving && self.permission_mode_saving {
            return;
        }
        if saving {
            self.permission_mode_saving = false;
        }
        self.permission_mode_loading = false;
        match result {
            Ok(mode) => {
                self.permission_mode = mode;
                if let Some(mode) = mode.filter(|_| saving) {
                    self.confirmation(format!("Permissions · {}", mode_name(mode)));
                    if let Some((address, request_id)) = self.approval_after_mode_save.take() {
                        self.issue(
                            address,
                            Action::ReplyPermission {
                                request_id,
                                allow: true,
                            },
                        );
                    }
                } else if mode.is_none() && self.modal.is_none() && !self.pane().exists {
                    self.open_permissions();
                }
            }
            Err(error) => {
                self.approval_after_mode_save = None;
                self.problem(format!("Workspace permissions · {error}"));
            }
        }
        if std::mem::take(&mut self.permission_mode_refresh_pending) {
            self.refresh_workspace_permission();
        }
    }

    pub(super) fn permission_mode_ready(&mut self) -> bool {
        if self.permission_mode_loading || self.permission_mode_saving {
            self.problem("Wait for workspace permissions · draft kept");
            return false;
        }
        if self.permission_mode.is_none() {
            self.open_permissions();
            return false;
        }
        true
    }
}

#[derive(Clone, Copy)]
enum ApprovalChoice {
    Allow,
    Deny,
    Full,
}

const APPROVAL_CHOICES: [ApprovalChoice; 3] = [
    ApprovalChoice::Allow,
    ApprovalChoice::Deny,
    ApprovalChoice::Full,
];

#[derive(Debug)]
pub(crate) struct ApprovalEditor {
    request_id: String,
    selected: usize,
    detail_offset: Cell<usize>,
}

impl ApprovalEditor {
    fn can_trust(approval: &PendingApproval, mode: Option<PermissionMode>) -> bool {
        mode == Some(PermissionMode::Ask) && approval.request.source == "permission_mode"
    }

    pub(crate) fn render(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        approvals: &[PendingApproval],
        mode: Option<PermissionMode>,
    ) {
        let Some(index) = approvals
            .iter()
            .position(|item| item.request.id == self.request_id)
        else {
            frame.render_widget(Paragraph::new("This approval has closed · Esc Close"), area);
            return;
        };
        let approval = &approvals[index];
        let request = &approval.request;
        let source = if request.source == "permission_mode" {
            "Ask mode"
        } else {
            "Host policy"
        };
        let mut lines = vec![Line::raw(format!(
            "{} of {} · Agent {} · {source}",
            index + 1,
            approvals.len(),
            request.conversation.as_str(),
        ))];
        if let Some(job) = &approval.job_id {
            lines.extend(render_literal(&format!("Job {job}"), emphasis(MUTED)));
        }
        let mut details = format!("{} · {}", request.tool, request.summary);
        if let Some(workspace) = &request.workspace {
            details.push_str(&format!("\nDirectory {}", workspace.display()));
        }
        if !request.candidate.is_empty() {
            details.push('\n');
            details.push_str(&request.candidate);
        }
        if let Some(arguments) = &request.arguments {
            if let Ok(arguments) = serde_json::to_string_pretty(arguments) {
                details.push('\n');
                details.push_str(&arguments);
            }
        }
        let labels = [
            "y  Allow once",
            "n  Deny",
            "a  Trust this workspace and allow",
        ];
        let count = if Self::can_trust(approval, mode) {
            3
        } else {
            2
        };
        let rows = usize::from(area.height)
            .saturating_sub(lines.len() + count + 2)
            .max(1);
        let offset = self.detail_offset.get();
        let detail_lines = wrap_bounded(
            render_literal(&details, emphasis(MUTED)),
            usize::from(area.width),
            offset.saturating_add(rows + 1),
        );
        let offset = offset.min(detail_lines.len().saturating_sub(rows));
        self.detail_offset.set(offset);
        let scrollable = detail_lines.len() > rows;
        lines.extend(detail_lines.into_iter().skip(offset).take(rows));
        if scrollable {
            lines.push(Line::styled(
                "PgUp / PgDn · Scroll operation details",
                emphasis(MUTED),
            ));
        }
        for (index, label) in labels.into_iter().take(count).enumerate() {
            let active = self.selected.min(count - 1) == index;
            let cursor = if active { "›" } else { " " };
            lines.push(Line::styled(
                format!("{cursor} {label}"),
                if active {
                    emphasis(ACCENT)
                } else {
                    emphasis(MUTED)
                },
            ));
        }
        lines.push(Line::styled(
            "↑↓ Select · Enter Apply · Tab Next request · Esc Close",
            emphasis(MUTED),
        ));
        frame.render_widget(Paragraph::new(lines), area);
    }
}

impl App {
    pub(crate) fn approvals(&self) -> &[PendingApproval] {
        if self.approvals_session.as_deref() == Some(&self.active.session) {
            &self.approvals
        } else {
            &[]
        }
    }

    pub(super) fn refresh_approvals(&mut self) {
        if self.endpoint.is_none() || !self.pane().exists {
            return;
        }
        let selection = self.selection();
        if self.approval_refresh.as_ref() == Some(&selection) {
            self.approval_refresh_dirty = true;
            return;
        }
        self.approval_refresh_dirty = false;
        self.approval_refresh = Some(selection.clone());
        self.effects.push(Job::PendingApprovals(selection));
    }

    pub(super) fn approvals_loaded(
        &mut self,
        selection: Selection,
        result: Result<Vec<PendingApproval>, Error>,
    ) {
        if self.approval_refresh.as_ref() != Some(&selection) {
            return;
        }
        self.approval_refresh = None;
        if selection != self.selection() {
            return;
        }
        if let Ok(approvals) = result {
            self.approvals_session = Some(selection.target.session_id);
            self.approvals = approvals;
        }
        if std::mem::take(&mut self.approval_refresh_dirty) {
            self.refresh_approvals();
        }
    }

    pub(crate) fn open_approvals(&mut self) -> bool {
        let Some(item) = self.approvals().first() else {
            return false;
        };
        self.modal = Some(Modal::Approvals(ApprovalEditor {
            request_id: item.request.id.clone(),
            selected: 0,
            detail_offset: Cell::default(),
        }));
        true
    }

    pub(crate) fn open_approval(&mut self, request_id: &str) -> bool {
        if !self
            .approvals()
            .iter()
            .any(|item| item.request.id == request_id)
        {
            return false;
        }
        self.modal = Some(Modal::Approvals(ApprovalEditor {
            request_id: request_id.into(),
            selected: 0,
            detail_offset: Cell::default(),
        }));
        true
    }

    pub(crate) fn approval_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        let Some(Modal::Approvals(mut editor)) = self.modal.take() else {
            return;
        };
        let approvals = self.approvals();
        let Some(index) = approvals
            .iter()
            .position(|item| item.request.id == editor.request_id)
        else {
            if key.code == KeyCode::Tab || key.code == KeyCode::Enter {
                self.open_approvals();
            } else {
                self.modal = Some(Modal::Approvals(editor));
            }
            return;
        };
        let approval = &approvals[index];
        let can_trust = ApprovalEditor::can_trust(approval, self.permission_mode);
        let last = if can_trust { 2 } else { 1 };
        let choice = match key.code {
            KeyCode::Up => {
                editor.selected = editor.selected.min(last).saturating_sub(1);
                None
            }
            KeyCode::Down => {
                editor.selected = (editor.selected + 1).min(last);
                None
            }
            KeyCode::Tab => {
                editor.request_id = approvals[(index + 1) % approvals.len()].request.id.clone();
                editor.selected = 0;
                editor.detail_offset.set(0);
                None
            }
            KeyCode::PageUp | KeyCode::PageDown => {
                let amount = if key.code == KeyCode::PageUp { -5 } else { 5 };
                editor
                    .detail_offset
                    .set(editor.detail_offset.get().saturating_add_signed(amount));
                None
            }
            KeyCode::Enter if key.kind == KeyEventKind::Press => {
                Some(APPROVAL_CHOICES[editor.selected.min(last)])
            }
            KeyCode::Char(character)
                if key.kind == KeyEventKind::Press && key.modifiers == KeyModifiers::NONE =>
            {
                match character {
                    'y' => Some(ApprovalChoice::Allow),
                    'n' => Some(ApprovalChoice::Deny),
                    'a' if can_trust => Some(ApprovalChoice::Full),
                    _ => None,
                }
            }
            _ => None,
        };
        let Some(choice) = choice else {
            self.modal = Some(Modal::Approvals(editor));
            return;
        };
        let address = Address::new(
            self.active.session.clone(),
            approval.request.conversation.as_str().into(),
        );
        let request_id = editor.request_id.clone();
        match choice {
            ApprovalChoice::Allow | ApprovalChoice::Deny => {
                self.issue(
                    address,
                    Action::ReplyPermission {
                        request_id,
                        allow: matches!(choice, ApprovalChoice::Allow),
                    },
                );
            }
            ApprovalChoice::Full => {
                if self.permission_mode_saving {
                    self.problem("Wait for workspace permissions");
                    self.modal = Some(Modal::Approvals(editor));
                    return;
                }
                self.set_permission_mode(PermissionMode::Full);
                if self.permission_mode_saving {
                    self.approval_after_mode_save = Some((address, request_id));
                }
            }
        }
    }
}

#[cfg(test)]
impl App {
    pub(crate) fn complete_test_permissions(&mut self) {
        self.effects
            .retain(|effect| !matches!(effect, Job::WorkspacePermission { mode: None, .. }));
        self.update(crate::jobs::Update::WorkspacePermission {
            ticket: self.permission_mode_ticket,
            saving: false,
            result: Ok(Some(PermissionMode::Full)),
        });
    }
}
