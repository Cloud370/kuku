use kuku_client::{Acknowledgement, Action, Command, Error, InputStatus, Target};

use crate::jobs::Job;

use super::{Address, App, SubmissionStatus};

const MAX_CHECKING: usize = 4;

#[derive(Debug)]
pub(crate) struct PendingItem {
    pub(crate) command: Command,
    pub(crate) stage: PendingStage,
    pub(super) origin: Option<u64>,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use kuku_client::{Action, AgentConfig, Error, Timing};

    use crate::app::App;
    use crate::jobs::{Job, Update};

    fn connected() -> App {
        let workspace = PathBuf::from("/workspace");
        let mut app = App::new(workspace.clone(), None);
        app.update(Update::Connected {
            endpoint: "http://127.0.0.1:6789".into(),
            workspace,
        });
        app.effects.clear();
        app
    }

    #[test]
    fn reopening_checks_receipt_before_retrying_exact_skill_command() {
        let mut original = connected();
        let text = "/skill:review Ask about tests";
        original.pane_mut().draft.insert(text);
        original.send(Timing::NextTurn);
        let Some(Job::Save(command)) = original.effects.pop() else {
            panic!("command must be saved before issuing")
        };
        assert!(
            matches!(&command.action, Action::Send { text: draft, bootstrap_skill, .. }
            if draft == text && bootstrap_skill.as_deref() == Some("review"))
        );
        let mut reopened = connected();
        reopened.update(Update::Pending(Ok(vec![command.clone()])));
        assert!(matches!(reopened.effects.as_slice(), [Job::Check(exact)] if exact == &command));
        reopened.effects.clear();
        reopened.pending_retry(&command.command_id);
        assert!(reopened.effects.is_empty());
        reopened.update(Update::Checked(command.command_id.clone(), Ok(None)));
        reopened.pending_retry(&command.command_id);
        assert!(matches!(reopened.effects.as_slice(), [Job::Send(exact)] if exact == &command));
    }

    #[test]
    fn rejected_initial_model_leaves_same_draft_available_after_route_correction() {
        let mut app = connected();
        app.configure(AgentConfig {
            model: Some("invalid-model".into()),
            ..AgentConfig::default()
        });
        app.pane_mut().draft.insert("Keep the first draft");
        app.send(Timing::Steer);
        let Some(Job::Save(first)) = app.effects.pop() else {
            panic!("initial send must be persisted")
        };
        app.update(Update::Saved(first.clone(), Ok(())));
        app.update(Update::Sent(
            first.command_id.clone(),
            Err(Error::Service {
                status: 400,
                code: "model_configuration".into(),
                error_message: "Unknown model".into(),
            }),
        ));
        assert_eq!("Keep the first draft", app.pane().draft.text());
        assert!(!app.pane().exists);

        app.configure(AgentConfig {
            model: Some("available-model".into()),
            ..AgentConfig::default()
        });
        app.send(Timing::Steer);
        let Some(Job::Save(corrected)) = app.effects.pop() else {
            panic!("corrected draft must be persisted")
        };
        assert_ne!(first.target.session_id, corrected.target.session_id);
        assert_ne!(first.command_id, corrected.command_id);
        assert!(
            matches!(&corrected.action, Action::Send { text, create: Some(config), .. }
            if text == "Keep the first draft" && config.model.as_deref() == Some("available-model"))
        );
    }
}

#[derive(Debug)]
pub(crate) enum PendingStage {
    Saving,
    Sending,
    Checking,
    Unknown(String),
    Missing,
    Rejected(String),
    Discarding,
}

impl PendingStage {
    pub(crate) fn label(&self) -> &str {
        match self {
            Self::Saving => "Saving send",
            Self::Sending => "Sending",
            Self::Checking => "Checking receipt",
            Self::Unknown(_) => "Receipt unconfirmed",
            Self::Missing => "No server receipt",
            Self::Rejected(_) => "Rejected",
            Self::Discarding => "Discarding",
        }
    }
}

impl App {
    pub(crate) fn pending_check(&mut self, id: &str) {
        if self.require_service() {
            if let Some(item) = self
                .pending
                .get_mut(id)
                .filter(|item| matches!(item.stage, PendingStage::Unknown(_)))
            {
                item.stage = PendingStage::Checking;
                self.checking.push_back(id.to_owned());
                self.check_pending();
            }
        }
    }

    pub(crate) fn pending_retry(&mut self, id: &str) {
        if self.require_service() {
            if let Some(item) = self
                .pending
                .get_mut(id)
                .filter(|item| matches!(item.stage, PendingStage::Missing))
            {
                item.stage = PendingStage::Sending;
                self.effects.push(Job::Send(item.command.clone()));
            }
        }
    }

    pub(crate) fn pending_discard(&mut self, id: &str) {
        if self
            .pending
            .get(id)
            .is_some_and(|item| matches!(item.stage, PendingStage::Rejected(_)))
        {
            self.pending.remove(id);
            self.message("Dismissed rejected command");
        } else if self.require_service() {
            if let Some(item) = self
                .pending
                .get_mut(id)
                .filter(|item| matches!(item.stage, PendingStage::Missing))
            {
                item.stage = PendingStage::Discarding;
                self.effects.push(Job::Discard(item.command.clone()));
            }
        }
    }

    pub(crate) fn pending_open(&mut self, target: Target, exists: bool) {
        let address = Address::from_target(&target);
        self.select(address, exists, String::new());
    }

    pub(crate) fn restore_rejected(&mut self, id: &str) {
        let Some(item) = self.pending.remove(id) else {
            return;
        };
        let target = item.command.target;
        if let Action::Send { text, create, .. } = item.command.action {
            let address = Address::from_target(&target);
            self.select(address, create.is_none(), String::new());
            let pane = self.pane_mut();
            if create.is_some() {
                pane.creation_rejected = true;
            }
            let mut draft = pane.draft.take_text();
            if !draft.is_empty() {
                draft.push_str("\n\n");
            }
            draft.push_str(&text);
            pane.draft.set_text(draft);
            self.message("Restored rejected text to draft");
        }
    }

    pub(super) fn saved(&mut self, command: Command, result: Result<(), Error>) {
        let id = command.command_id.clone();
        match result {
            Ok(()) => {
                if let Some(item) = self.pending.get_mut(&id) {
                    item.stage = if self.connecting || self.connection_problem.is_some() {
                        PendingStage::Unknown("Saved while reconnecting".into())
                    } else {
                        PendingStage::Sending
                    };
                }
                if self.available() {
                    self.effects.push(Job::Send(command));
                }
                if self.quit_after_save {
                    self.exit();
                }
            }
            Err(error) => {
                self.quit_after_save = false;
                self.command_failed(&id, error, true);
            }
        }
    }

    pub(super) fn discarded(&mut self, id: &str, result: Result<(), Error>) {
        match result {
            Ok(()) => {
                if let Some(item) = self.pending.remove(id) {
                    if let Some(pane) = self
                        .panes
                        .get_mut(&Address::from_target(&item.command.target))
                    {
                        if matches!(item.command.action, Action::Send { .. }) {
                            pane.sending = false;
                        }
                        if matches!(item.command.action, Action::Cancel { .. }) {
                            pane.stopping = false;
                        }
                    }
                }
                self.message("Discarded saved command");
            }
            Err(error) => self.command_failed(id, error, false),
        }
    }

    pub(super) fn recover_pending(&mut self, commands: Vec<Command>) {
        for command in commands {
            let id = command.command_id.clone();
            if !self.pending.contains_key(&id) {
                self.pending.insert(
                    id.clone(),
                    PendingItem {
                        command,
                        stage: PendingStage::Checking,
                        origin: None,
                    },
                );
                self.checking.push_back(id);
            }
        }
        let ids: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, item)| matches!(item.stage, PendingStage::Unknown(_)))
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            if let Some(item) = self.pending.get_mut(&id) {
                item.stage = PendingStage::Checking;
                self.checking.push_back(id);
            }
        }
        self.check_pending();
    }

    fn check_pending(&mut self) {
        if !self.available() {
            return;
        }
        while self.outstanding_checks < MAX_CHECKING {
            let Some(id) = self.checking.pop_front() else {
                break;
            };
            if let Some(item) = self.pending.get(&id) {
                self.outstanding_checks += 1;
                self.effects.push(Job::Check(item.command.clone()));
            }
        }
    }

    pub(super) fn checked(&mut self, id: &str, result: Result<Option<Acknowledgement>, Error>) {
        self.outstanding_checks = self.outstanding_checks.saturating_sub(1);
        match result {
            Ok(Some(ack)) => self.acknowledged(id, ack),
            Ok(None) => {
                if let Some(item) = self.pending.get_mut(id) {
                    item.stage = PendingStage::Missing;
                    self.message("No receipt found · /pending to retry or discard");
                }
            }
            Err(error) => {
                if let Some(item) = self.pending.get_mut(id) {
                    item.stage = PendingStage::Unknown(error.to_string());
                }
                self.problem(format!("Check receipt · {error} · /pending"));
            }
        }
        self.check_pending();
    }

    pub(super) fn command_failed(&mut self, id: &str, error: Error, save_failure: bool) {
        let Some(item) = self.pending.get_mut(id) else {
            return;
        };
        let address = Address::from_target(&item.command.target);
        let rejected = error.definitively_rejected();
        let clear = rejected || save_failure;
        let problem = error.to_string();
        if !save_failure
            && address == self.active
            && matches!(error, Error::Transport(_))
            && self.panes.get(&address).is_some_and(|pane| !pane.exists)
        {
            self.connection_problem = Some(format!("Service connection · {problem} · /reconnect"));
        }
        let mut restored = false;
        if let Some(pane) = self.panes.get_mut(&address) {
            if matches!(item.command.action, Action::Cancel { .. }) {
                pane.stopping = false;
            }
            if let Action::Send { text, create, .. } = &item.command.action {
                pane.sending = false;
                pane.creation_rejected |= rejected && create.is_some();
                if clear && pane.draft.text().is_empty() {
                    pane.draft.set_text(text.clone());
                    restored = true;
                }
            }
        }
        item.stage = if clear {
            PendingStage::Rejected(problem.clone())
        } else {
            PendingStage::Unknown(problem.clone())
        };
        if restored {
            self.pending.remove(id);
        }
        let recovery = if restored {
            "Draft restored"
        } else {
            "See /pending"
        };
        self.problem(format!(
            "{recovery} · {} · {problem}",
            if save_failure {
                "Client storage"
            } else {
                "Command"
            }
        ));
        if address == self.active {
            self.refresh();
        }
    }

    pub(super) fn acknowledged(&mut self, id: &str, ack: Acknowledgement) {
        let Some(item) = self.pending.remove(id) else {
            return;
        };
        let address = Address::from_target(&item.command.target);
        let active = address == self.active;
        match item.command.action {
            Action::Send { text, .. } => {
                let pane = self.panes.entry(address).or_default();
                let initial = !pane.exists;
                pane.exists = true;
                pane.sending = false;
                pane.record_submission(
                    ack.receipt.event_id,
                    SubmissionStatus::from_receipt(ack.status),
                );
                pane.draft.remember(&text);
                if pane.title.is_empty() {
                    pane.title = crate::text::shorten(&crate::text::single_line(&text), 68);
                }
                if active && initial {
                    self.begin_observing();
                }
            }
            Action::CreateAgent { .. } => {
                self.message(format!("Agent {} created", address.agent));
                if item.origin == Some(self.generation)
                    && address.session == self.active.session
                    && self.modal.is_none()
                {
                    let title = self.pane().title.clone();
                    self.select(address, true, title);
                }
            }
            Action::Configure { config } => {
                if let Some(pane) = self.panes.get_mut(&address) {
                    if pane
                        .snapshot
                        .as_ref()
                        .is_none_or(|snapshot| snapshot.cursor < ack.receipt.event_id)
                    {
                        pane.config_receipt = Some((ack.receipt.event_id, config));
                    }
                }
                self.message("Settings saved for the next safe model boundary");
            }
            Action::ReplyPermission { .. } | Action::ReplyQuestion { .. } => {
                self.confirmation("Reply received")
            }
            Action::Withdraw { .. } => {
                if ack.status.is_some() {
                    if let Some(pane) = self.panes.get_mut(&address) {
                        pane.record_submission(
                            ack.receipt.event_id,
                            SubmissionStatus::from_receipt(ack.status),
                        );
                    }
                }
                let message = match ack.status {
                    Some(InputStatus::Accepted) => "Input has already been accepted",
                    Some(InputStatus::Queued) => "Input is still queued",
                    Some(InputStatus::Withdrawn) => "Input withdrawn",
                    None => "Withdrawal receipt confirmed",
                };
                self.message(message);
            }
            Action::Advance => self.message("Continue request received"),
            Action::Cancel { .. } => {
                if let Some(pane) = self.panes.get_mut(&address) {
                    pane.stopping = false;
                }
                self.message("Execution stopped · draft kept");
            }
        }
        if active {
            self.refresh();
        }
    }
}
