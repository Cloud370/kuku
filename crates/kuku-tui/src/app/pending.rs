use kuku_client::{new_id, Acknowledgement, Action, Command, Error, InputStatus, Target, Timing};

use crate::jobs::Job;

use super::{Address, App, RewindDraft, SubmissionStatus};

const MAX_CHECKING: usize = 4;

impl App {
    pub(crate) fn send(&mut self, timing: Timing) {
        let empty =
            self.pane().draft.text().trim().is_empty() && self.pane().value_attachments.is_empty();
        if empty || !self.require_service() {
            return;
        }
        if self.pane().value_saving {
            self.problem("Wait for the current save");
            return;
        }
        if self.pane().sending {
            self.problem("Wait for the current submission's receipt before retrying it");
            return;
        }
        let target = self.target(&self.active);
        if self.pending.values().any(|item| {
            item.command.target == target
                && matches!(item.command.action, Action::Send { .. })
                && !matches!(item.stage, PendingStage::Rejected(_))
        }) {
            self.problem("Check the previous send with delivery recovery · draft kept");
            return;
        }
        if self.pane().creation_rejected && !self.pane().exists {
            self.renew_draft_identity();
        }
        let active = self.active.clone();
        let pane = self.pane_mut();
        let text = pane.draft.take_text();
        let bootstrap_skill = text
            .strip_prefix("/skill:")
            .and_then(|skill| skill.split(char::is_whitespace).next())
            .filter(|skill| !skill.is_empty())
            .map(str::to_owned);
        let create = (!pane.exists).then(|| Box::new(pane.initial_config.clone()));
        let values = pane.take_values();
        pane.sending = true;
        self.issue(
            active,
            Action::Send {
                text,
                timing,
                values,
                create,
                create_agent_if_missing: false,
                config_patch: None,
                prompts_dir: None,
                bootstrap_skill,
            },
        );
    }

    pub(crate) fn issue(&mut self, address: Address, action: Action) {
        if !self.require_service() {
            return;
        }
        let command = Command {
            target: self.target(&address),
            command_id: new_id(),
            action,
        };
        self.pending.insert(
            command.command_id.clone(),
            PendingItem {
                command: command.clone(),
                stage: PendingStage::Saving,
                origin: Some(self.generation),
            },
        );
        self.update_pending_delivery(&command.command_id);
        self.effects.push(Job::Save(command));
    }
}

#[derive(Debug)]
pub(crate) struct PendingItem {
    pub(crate) command: Command,
    pub(crate) stage: PendingStage,
    pub(super) origin: Option<u64>,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use kuku_client::{
        Acknowledgement, Action, AgentConfig, Command, Error, HistoryPage, InputStatus, Receipt,
        Snapshot, Timing,
    };
    use serde_json::json;

    use crate::app::{Activity, App};
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

    fn execution_started(app: &mut App, execution_id: u64) {
        app.update(Update::Observation(
            app.selection(),
            serde_json::from_value(json!({
                "type":"committed","id":execution_id,"agent":"main",
                "kind":"runtime.operation","operation":"execution_started",
            }))
            .unwrap(),
        ));
    }

    fn saved_interrupt(app: &mut App) -> Command {
        app.interrupt();
        let Some(Job::Save(command)) = app.effects.pop() else {
            panic!("interrupt must save the selected execution identity");
        };
        command
    }

    fn acknowledge_interrupt(app: &mut App, command: Command, event_id: u64) {
        app.update(Update::Sent(
            command.command_id.clone(),
            Ok(Acknowledgement {
                receipt: Receipt {
                    command_id: command.command_id,
                    event_id,
                    agent: "main".into(),
                },
                replayed: false,
                status: None,
                warnings: vec![],
            }),
        ));
    }

    fn state_snapshot(
        app: &App,
        cursor: u64,
        status: &str,
        execution: Option<u64>,
    ) -> Box<Snapshot> {
        Box::new(
            serde_json::from_value(json!({
                "session_id":app.active.session,
                "workspace":app.workspace,
                "cursor":cursor,
                "agents":[],
                "pending":[],
                "progress":null,
                "agent":{
                    "address":app.active.agent,
                    "status":status,
                    "turn":5,
                    "pending_inputs":0,
                    "execution_id":execution,
                    "last_error":null,
                    "config":{},
                    "usage":{"model_requests":0,"unreported_requests":0},
                }
            }))
            .unwrap(),
        )
    }

    #[test]
    fn a_late_interrupt_receipt_cannot_block_or_clear_a_newer_interrupt() {
        let mut app = connected();
        let interrupt = |app: &mut App, execution_id| {
            app.update(Update::Observation(
                app.selection(),
                serde_json::from_value(json!({
                    "type":"committed","id":execution_id,"agent":"main",
                    "kind":"runtime.operation","operation":"execution_started",
                }))
                .unwrap(),
            ));
            assert_eq!(Activity::Running, app.pane().activity());
            app.interrupt();
            let Some(Job::Save(command)) = app.effects.pop() else {
                panic!("interrupt must save the selected execution identity");
            };
            assert!(
                matches!(command.action, Action::Cancel { execution_id: id } if id == execution_id)
            );
            assert_eq!(Activity::Stopping, app.pane().activity());
            command
        };
        let first = interrupt(&mut app, 5);
        let second = interrupt(&mut app, 12);
        app.update(Update::Sent(
            first.command_id.clone(),
            Ok(Acknowledgement {
                receipt: Receipt {
                    command_id: first.command_id,
                    event_id: 7,
                    agent: "main".into(),
                },
                replayed: false,
                status: None,
                warnings: vec![],
            }),
        ));
        assert_eq!(Activity::Stopping, app.pane().activity());
        app.update(Update::Sent(
            second.command_id,
            Err(Error::Io(std::io::Error::other("client connection closed"))),
        ));
        assert_eq!(Activity::Running, app.pane().activity());
    }

    #[test]
    fn stopped_notice_clears_on_execution_start_not_submission_and_preserves_other_notices() {
        let mut app = connected();
        execution_started(&mut app, 5);
        let cancel = saved_interrupt(&mut app);
        assert!(matches!(&cancel.action, Action::Cancel { execution_id: 5 }));
        acknowledge_interrupt(&mut app, cancel, 7);
        assert_eq!(
            Some("Execution stopped · draft kept"),
            app.notice.as_ref().map(|notice| notice.text.as_str())
        );

        app.pane_mut().draft.insert("Next turn");
        app.send(Timing::NextTurn);
        assert_eq!(
            Some("Execution stopped · draft kept"),
            app.notice.as_ref().map(|notice| notice.text.as_str())
        );

        execution_started(&mut app, 12);
        assert!(app.notice.is_none());

        app.problem("Unrelated persistent problem");
        execution_started(&mut app, 20);
        assert_eq!(
            Some("Unrelated persistent problem"),
            app.notice.as_ref().map(|notice| notice.text.as_str())
        );
        assert!(app.notice.as_ref().is_some_and(|notice| notice.error));
    }

    #[test]
    fn refreshed_active_execution_clears_the_stopped_notice() {
        let mut app = connected();
        execution_started(&mut app, 5);
        let cancel = saved_interrupt(&mut app);
        acknowledge_interrupt(&mut app, cancel, 7);
        assert!(app.notice.is_some());

        app.update(Update::Refreshed(
            app.selection(),
            Ok(state_snapshot(&app, 12, "running", Some(12))),
        ));
        assert!(app.notice.is_none());

        app.update(Update::Refreshed(
            app.selection(),
            Ok(state_snapshot(&app, 14, "idle", None)),
        ));
        assert!(app.notice.is_none());
    }

    #[test]
    fn late_cancel_receipt_after_newer_snapshots_cannot_restore_stopped_notice() {
        let mut app = connected();
        execution_started(&mut app, 5);
        let cancel = saved_interrupt(&mut app);

        app.update(Update::Refreshed(
            app.selection(),
            Ok(state_snapshot(&app, 12, "running", Some(12))),
        ));
        app.update(Update::Refreshed(
            app.selection(),
            Ok(state_snapshot(&app, 14, "idle", None)),
        ));
        acknowledge_interrupt(&mut app, cancel, 7);

        assert!(app.notice.is_none());
        assert_eq!(None, app.pane().execution());
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
    fn reopening_a_delivered_input_does_not_duplicate_committed_history() {
        let text = "same text as another submission";
        let mut first = connected();
        first.pane_mut().draft.insert(text);
        first.send(Timing::Steer);
        let Some(Job::Save(command)) = first.effects.pop() else {
            panic!("saved command")
        };

        let mut reopened = connected();
        reopened.update(Update::Pending(Ok(vec![command.clone()])));
        reopened.pending_open(command.target.clone(), true);
        assert!(reopened.pane().deliveries.rows().is_empty());
        let state = serde_json::from_value(json!({
            "session_id":command.target.session_id,"workspace":command.target.workspace,
            "cursor":14,"agents":[],"pending":[],
            "agent":{"address":"main","status":"running","config":{},"pending_inputs":0,"turn":13},
        }))
        .unwrap();
        let histories = serde_json::from_value(json!([
            {"id":4,"agent":"main","type":"user","turn":1,"text":text},
            {"id":14,"agent":"main","type":"user","turn":13,"text":text},
        ]))
        .unwrap();
        reopened.update(Update::Loaded(
            reopened.selection(),
            Box::new(state),
            HistoryPage {
                entries: histories,
                rewind_groups: Vec::new(),
                next_before: None,
            },
        ));
        assert!(reopened.pane().deliveries.rows().is_empty());
        reopened.update(Update::Checked(
            command.command_id.clone(),
            Ok(Some(Acknowledgement {
                receipt: Receipt {
                    command_id: command.command_id.clone(),
                    event_id: 9,
                    agent: "main".into(),
                },
                replayed: true,
                status: Some(InputStatus::Accepted),
                warnings: vec![],
            })),
        ));
        assert!(reopened.pane().deliveries.rows().is_empty());
        let history = reopened.pane_mut().transcript.project(70, 17);
        assert_eq!(
            2,
            history
                .lines
                .iter()
                .filter(|line| line.to_string().contains(text))
                .count()
        );
    }

    #[test]
    fn rejected_replacement_keeps_a_recoverable_edit_and_the_current_draft() {
        let mut app = connected();
        app.pane_mut().exists = true;
        app.pane_mut().draft.insert("independent draft");
        app.issue(
            app.active.clone(),
            Action::ReplaceQueued {
                input_command_id: "admitted".into(),
                text: "edited text\nwith a newline".into(),
            },
        );
        let Some(Job::Save(command)) = app.effects.pop() else {
            panic!("saved command")
        };
        app.update(Update::Saved(command.clone(), Ok(())));
        app.update(Update::Sent(
            command.command_id.clone(),
            Err(Error::Service {
                status: 409,
                code: "input_not_queued".into(),
                error_message: "Input has already been accepted".into(),
            }),
        ));
        assert_eq!("independent draft", app.pane().draft.text());
        app.restore_rejected(&command.command_id);
        assert_eq!(
            "independent draft\n\nedited text\nwith a newline",
            app.pane().draft.text()
        );
        assert!(app.pending.contains_key(&command.command_id));
    }

    #[test]
    fn a_late_queue_receipt_updates_only_its_original_recipient() {
        let mut app = connected();
        app.pane_mut().exists = true;
        let main = app.active.clone();
        app.pane_mut().draft.insert("same text");
        app.send(Timing::NextTurn);
        let Some(Job::Save(first)) = app.effects.pop() else {
            panic!("saved command")
        };
        let reviewer = super::Address::new(main.session.clone(), "reviewer".into());
        app.select(reviewer, true, String::new());
        app.pane_mut().draft.insert("same text");
        app.send(Timing::Steer);
        let Some(Job::Save(second)) = app.effects.pop() else {
            panic!("saved command")
        };

        app.update(Update::Sent(
            first.command_id.clone(),
            Ok(Acknowledgement {
                receipt: Receipt {
                    command_id: first.command_id.clone(),
                    event_id: 13,
                    agent: "main".into(),
                },
                replayed: false,
                status: Some(InputStatus::Queued),
                warnings: vec![],
            }),
        ));
        let visible = app.pane().deliveries.rows();
        assert_eq!(1, visible.len());
        assert_eq!(&second.command_id, &visible[0].id);
        assert_eq!(super::SubmissionStatus::Saving, visible[0].status);

        app.select(main, true, String::new());
        let original = app.pane().deliveries.rows();
        assert_eq!(1, original.len());
        assert_eq!(&first.command_id, &original[0].id);
        assert_eq!(super::SubmissionStatus::Queued, original[0].status);
        assert_eq!(Timing::NextTurn, original[0].timing);
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

pub(super) const MAX_REWIND_DRAFTS: usize = 64;

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
                self.update_pending_delivery(id);
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
                self.update_pending_delivery(id);
            }
        }
    }

    pub(crate) fn pending_discard(&mut self, id: &str) {
        if self.pending.get(id).is_some_and(|item| {
            matches!(item.stage, PendingStage::Rejected(_))
                && !matches!(item.command.action, Action::ReplaceQueued { .. })
        }) {
            if let Some(item) = self.pending.remove(id) {
                if let Action::Withdraw { input_command_id } = &item.command.action {
                    self.withdrawal_drafts
                        .remove(&(item.command.target.clone(), input_command_id.clone()));
                }
                if let Some(pane) = self
                    .panes
                    .get_mut(&Address::from_target(&item.command.target))
                {
                    pane.deliveries.remove(id);
                    if let Action::Withdraw { input_command_id } = &item.command.action {
                        pane.deliveries.withdrawal(input_command_id, None);
                    }
                }
            }
            self.message("Dismissed rejected command");
        } else if self.require_service() {
            if let Some(item) = self.pending.get_mut(id).filter(|item| {
                matches!(item.stage, PendingStage::Missing)
                    || matches!(item.stage, PendingStage::Rejected(_))
                        && matches!(item.command.action, Action::ReplaceQueued { .. })
            }) {
                item.stage = PendingStage::Discarding;
                self.effects.push(Job::Discard(item.command.clone()));
                self.update_pending_delivery(id);
            }
        }
    }

    pub(crate) fn pending_open(&mut self, target: Target, exists: bool) {
        let address = Address::from_target(&target);
        self.select(address, exists, String::new());
    }

    pub(crate) fn restore_rejected(&mut self, id: &str) {
        if let Some(item) = self.pending.get(id) {
            if let Action::ReplaceQueued { text, .. } = &item.command.action {
                let address = Address::from_target(&item.command.target);
                let text = text.clone();
                self.select(address, true, String::new());
                self.append_to_draft(&text);
                self.message(
                    "Copied replacement to draft · delivery recovery keeps the saved edit",
                );
                return;
            }
        }
        let Some(item) = self.pending.remove(id) else {
            return;
        };
        let target = item.command.target;
        if let Action::Send {
            text,
            create,
            values,
            ..
        } = item.command.action
        {
            let address = Address::from_target(&target);
            self.select(address, create.is_none(), String::new());
            let pane = self.pane_mut();
            if create.is_some() {
                pane.creation_rejected = true;
            }
            pane.restore_values(values);
            self.append_to_draft(&text);
            self.message("Restored rejected text to draft");
        }
    }

    fn append_to_draft(&mut self, text: &str) {
        let editor = &mut self.pane_mut().draft;
        let mut draft = editor.take_text();
        if !draft.is_empty() {
            draft.push_str("\n\n");
        }
        draft.push_str(text);
        editor.set_text(draft);
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
                self.update_pending_delivery(&id);
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
                    if let Action::Withdraw { input_command_id } = &item.command.action {
                        self.withdrawal_drafts
                            .remove(&(item.command.target.clone(), input_command_id.clone()));
                    }
                    if let Some(pane) = self
                        .panes
                        .get_mut(&Address::from_target(&item.command.target))
                    {
                        pane.deliveries.remove(id);
                        if let Action::Withdraw { input_command_id } = &item.command.action {
                            pane.deliveries.withdrawal(input_command_id, None);
                        }
                        if matches!(item.command.action, Action::Send { .. }) {
                            pane.sending = false;
                        }
                        if let Action::Cancel { execution_id } = item.command.action {
                            pane.finish_interrupt(execution_id);
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
                self.checking.push_back(id.clone());
                self.update_pending_delivery(&id);
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
                    self.message("No receipt found · use delivery recovery to retry or discard");
                }
            }
            Err(error) => {
                if let Some(item) = self.pending.get_mut(id) {
                    item.stage = PendingStage::Unknown(error.to_string());
                }
                self.problem(format!("Check receipt · {error} · delivery recovery"));
            }
        }
        self.update_pending_delivery(id);
        self.check_pending();
    }

    pub(super) fn command_failed(&mut self, id: &str, error: Error, save_failure: bool) {
        let Some(item) = self.pending.get_mut(id) else {
            return;
        };
        let rewind = matches!(&item.command.action, Action::RewindMessage { .. });
        let address = Address::from_target(&item.command.target);
        let rejected = error.definitively_rejected();
        let clear = rejected || save_failure;
        let problem = error.to_string();
        if !save_failure
            && address == self.active
            && matches!(error, Error::Transport(_))
            && self.panes.get(&address).is_some_and(|pane| !pane.exists)
        {
            self.connection_problem = Some(format!(
                "Service connection · {problem} · Ctrl+R to reconnect"
            ));
        }
        let mut restored = false;
        if let Some(pane) = self.panes.get_mut(&address) {
            if let Action::Cancel { execution_id } = item.command.action {
                pane.finish_interrupt(execution_id);
            }
            if let Action::Send {
                text,
                create,
                values,
                ..
            } = &item.command.action
            {
                pane.sending = false;
                pane.creation_rejected |= rejected && create.is_some();
                if clear && pane.draft.text().is_empty() {
                    pane.draft.set_text(text.clone());
                    pane.restore_values(values.clone());
                    restored = true;
                }
            }
        }
        item.stage = if clear {
            PendingStage::Rejected(problem.clone())
        } else {
            PendingStage::Unknown(problem.clone())
        };
        self.update_pending_delivery(id);
        if restored {
            self.pending.remove(id);
        }
        let recovery = if restored {
            "Draft restored"
        } else if rewind {
            "Selected text retained in pending action"
        } else {
            "Use delivery recovery"
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
        let target = item.command.target.clone();
        let address = Address::from_target(&target);
        let active = address == self.active;
        match item.command.action {
            Action::Send { text, timing, .. } => {
                let pane = self.panes.entry(address).or_default();
                let initial = !pane.exists;
                pane.exists = true;
                pane.sending = false;
                pane.record_submission(
                    ack.receipt.event_id,
                    SubmissionStatus::from_receipt(ack.status),
                );
                pane.deliveries.receipt(&ack, &text, timing, false);
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
                let route_changed = self
                    .panes
                    .get(&address)
                    .is_some_and(|pane| pane.model_route_changed(&config));
                if let Some(pane) = self.panes.get_mut(&address) {
                    if pane
                        .snapshot
                        .as_ref()
                        .is_none_or(|snapshot| snapshot.cursor < ack.receipt.event_id)
                    {
                        pane.config_receipt = Some((ack.receipt.event_id, config));
                    }
                }
                if active && route_changed {
                    self.message("Settings saved for the next safe model boundary");
                }
            }
            Action::ReplyPermission { .. } | Action::ReplyQuestion { .. } => {
                self.confirmation("Reply received")
            }
            Action::Withdraw { input_command_id } => {
                let restore_text = self
                    .withdrawal_drafts
                    .remove(&(target, input_command_id.clone()));
                if ack.status == Some(InputStatus::Withdrawn) {
                    if let Some(text) = restore_text {
                        if let Some(pane) = self.panes.get_mut(&address) {
                            if pane.draft.text().is_empty() {
                                pane.draft.set_text(text);
                            }
                        }
                    }
                }
                if ack.status.is_some() {
                    if let Some(pane) = self.panes.get_mut(&address) {
                        pane.record_submission(
                            ack.receipt.event_id,
                            SubmissionStatus::from_receipt(ack.status),
                        );
                    }
                }
                if let Some(pane) = self.panes.get_mut(&address) {
                    pane.deliveries
                        .acknowledge_withdrawal(&input_command_id, ack.status);
                }
                let message = match ack.status {
                    Some(InputStatus::Accepted) => "Input has already been accepted",
                    Some(InputStatus::Queued) => "Input is still queued",
                    Some(InputStatus::Withdrawn) => "Input withdrawn",
                    None => "Withdrawal receipt confirmed",
                };
                self.message(message);
            }
            Action::ReplaceQueued {
                input_command_id,
                text,
            } => {
                let pane = self.panes.entry(address).or_default();
                let timing = pane
                    .deliveries
                    .rows()
                    .iter()
                    .find(|row| row.id == ack.receipt.command_id || row.id == input_command_id)
                    .map_or(kuku_client::Timing::Steer, |row| row.timing);
                pane.deliveries.replace(
                    &input_command_id,
                    &ack.receipt.command_id,
                    ack.receipt.event_id,
                );
                pane.deliveries.receipt(&ack, &text, timing, true);
                self.confirmation(match ack.status {
                    Some(InputStatus::Accepted) => "Replacement accepted",
                    Some(InputStatus::Withdrawn) => "Replacement withdrawn",
                    Some(InputStatus::Queued) | None => "Queued input replaced",
                });
            }
            Action::RewindMessage { text, .. } => {
                let draft = self.rewind_drafts.remove(id).unwrap_or(RewindDraft {
                    address: address.clone(),
                    text,
                    revision: 0,
                });
                let mut restored = false;
                if let Some(pane) = self.panes.get_mut(&draft.address) {
                    if pane.draft_revision == draft.revision && pane.draft.text().is_empty() {
                        pane.draft.set_text(draft.text.clone());
                        pane.draft_revision = pane.draft_revision.wrapping_add(1);
                        restored = true;
                    }
                }
                if ack.warnings.is_empty() {
                    self.confirmation(if restored {
                        "Message rewound · original text restored as an unsent draft"
                    } else {
                        "Message rewound · original text retained in Ctrl+Up history"
                    });
                } else {
                    self.problem(format!(
                        "Message rewound with warnings · original text retained in Ctrl+Up history · {}",
                        ack.warnings.join(" · "),
                    ));
                }
                if active {
                    self.refresh_rewind_history();
                }
            }
            Action::Compact | Action::RestoreContext => {}
            Action::Advance => {
                if active && !self.notice.as_ref().is_some_and(|notice| notice.error) {
                    self.confirmation("Continue request received");
                }
            }
            Action::Cancel { execution_id } => {
                if let Some(pane) = self.panes.get_mut(&address) {
                    pane.finish_interrupt(execution_id);
                    if active
                        && pane.latest_execution_started == Some(execution_id)
                        && pane
                            .execution()
                            .is_none_or(|current| current == execution_id)
                    {
                        self.stopped_execution_notice(address.clone(), execution_id);
                    }
                }
            }
        }
        if active {
            self.refresh();
        }
    }
}
