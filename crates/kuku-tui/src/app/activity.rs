use std::collections::HashSet;

use kuku_client::{Action, AgentProgress, AgentStatus, HistoryEntry, InputStatus};
use serde_json::Value;

use super::{App, Pane, PendingStage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServiceStatus {
    Connecting,
    Connected,
    Reconnecting,
    Restarting,
    Offline,
    UpdatesUnavailable,
}

impl ServiceStatus {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Connecting => "Connecting",
            Self::Connected => "Connected",
            Self::Reconnecting => "Reconnecting",
            Self::Restarting => "Restarting",
            Self::Offline => "Offline",
            Self::UpdatesUnavailable => "Updates unavailable",
        }
    }
}

#[derive(Debug, Default)]
pub(super) enum Watch {
    #[default]
    Detached,
    Connecting,
    Connected,
    Reconnecting(String),
    Failed(String),
}

impl Watch {
    pub(super) fn problem(&self) -> Option<&str> {
        match self {
            Self::Reconnecting(problem) | Self::Failed(problem) => Some(problem),
            Self::Detached | Self::Connecting | Self::Connected => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Activity {
    Ready,
    Opening,
    Configuration,
    Running,
    WaitingForModel,
    Streaming,
    RunningTools,
    Waiting,
    Stopping,
    Stopped,
    Interrupted,
    Finished,
    Failed,
}

impl Activity {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::Opening => "Opening",
            Self::Configuration => "Configuration",
            Self::Running => "Running",
            Self::WaitingForModel => "Waiting for model",
            Self::Streaming => "Streaming reply",
            Self::RunningTools => "Running tools",
            Self::Waiting => "Waiting",
            Self::Stopping => "Stopping",
            Self::Stopped => "Stopped",
            Self::Interrupted => "Interrupted",
            Self::Finished => "Finished",
            Self::Failed => "Failed",
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct ExecutionProgress {
    waiting_for_model: bool,
    streaming: bool,
    tools: HashSet<String>,
}

impl ExecutionProgress {
    pub(super) fn from_snapshot(progress: Option<&AgentProgress>) -> Self {
        match progress {
            Some(AgentProgress::WaitingForModel) => Self {
                waiting_for_model: true,
                ..Self::default()
            },
            Some(AgentProgress::RunningTools { tool_call_ids }) => Self {
                tools: tool_call_ids.iter().cloned().collect(),
                ..Self::default()
            },
            None => Self::default(),
        }
    }

    pub(super) fn clear_stream(&mut self) {
        self.streaming = false;
    }

    pub(super) fn committed(
        &mut self,
        kind: &str,
        entry: Option<&HistoryEntry>,
        tool_call_id: Option<String>,
    ) {
        match kind {
            "runtime.step_prepared" => {
                *self = Self {
                    waiting_for_model: true,
                    ..Self::default()
                };
            }
            "model.response" | "model.error" => {
                self.waiting_for_model = false;
                self.streaming = false;
            }
            "runtime.tool_dispatched" => {
                if let Some(id) = tool_call_id {
                    self.tools.insert(id);
                }
            }
            "tool.result" => {
                if let Some(id) = entry.and_then(|entry| entry.details["tool_call_id"].as_str()) {
                    self.tools.remove(id);
                }
            }
            _ => {}
        }
    }

    pub(super) fn live(&mut self, event: &Value) {
        match event["type"].as_str() {
            Some("model_request" | "model_recovery") => {
                self.waiting_for_model = true;
                self.streaming = false;
            }
            Some("text" | "thinking")
                if event["content"]
                    .as_str()
                    .is_some_and(|text| !text.is_empty()) =>
            {
                self.streaming = true;
            }
            _ => {}
        }
    }

    fn activity(&self) -> Activity {
        if !self.tools.is_empty() {
            Activity::RunningTools
        } else if self.streaming {
            Activity::Streaming
        } else if self.waiting_for_model {
            Activity::WaitingForModel
        } else {
            Activity::Running
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubmissionStatus {
    Saving,
    AwaitingReceipt,
    CheckingReceipt,
    ReceiptUnconfirmed,
    ReceiptMissing,
    Rejected,
    Discarding,
    Queued,
    Accepted,
    Withdrawn,
    Confirmed,
}

impl SubmissionStatus {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Saving => "Saving input",
            Self::AwaitingReceipt => "Awaiting receipt",
            Self::CheckingReceipt => "Checking receipt",
            Self::ReceiptUnconfirmed => "Receipt unconfirmed",
            Self::ReceiptMissing => "Receipt missing",
            Self::Rejected => "Send rejected",
            Self::Discarding => "Discarding send",
            Self::Queued => "Input queued",
            Self::Accepted => "Input accepted",
            Self::Withdrawn => "Input withdrawn",
            Self::Confirmed => "Receipt confirmed",
        }
    }

    pub(super) fn from_receipt(status: Option<InputStatus>) -> Self {
        match status {
            Some(InputStatus::Queued) => Self::Queued,
            Some(InputStatus::Accepted) => Self::Accepted,
            Some(InputStatus::Withdrawn) => Self::Withdrawn,
            None => Self::Confirmed,
        }
    }

    fn from_pending(stage: &PendingStage) -> Self {
        match stage {
            PendingStage::Saving => Self::Saving,
            PendingStage::Sending => Self::AwaitingReceipt,
            PendingStage::Checking => Self::CheckingReceipt,
            PendingStage::Unknown(_) => Self::ReceiptUnconfirmed,
            PendingStage::Missing => Self::ReceiptMissing,
            PendingStage::Rejected(_) => Self::Rejected,
            PendingStage::Discarding => Self::Discarding,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct InputActivity {
    pub(crate) draft: bool,
    pub(crate) submission: Option<SubmissionStatus>,
    pub(crate) queued: usize,
}

impl App {
    pub(crate) fn service_status(&self) -> ServiceStatus {
        if self.restarting {
            return ServiceStatus::Restarting;
        }
        if self.connecting {
            return if self.resolved_workspace {
                ServiceStatus::Reconnecting
            } else {
                ServiceStatus::Connecting
            };
        }
        if self.connection_problem.is_some() {
            return ServiceStatus::Offline;
        }
        match self.pane().watch {
            Watch::Detached | Watch::Connected => ServiceStatus::Connected,
            Watch::Connecting => ServiceStatus::Connecting,
            Watch::Reconnecting(_) => ServiceStatus::Reconnecting,
            Watch::Failed(_) => ServiceStatus::UpdatesUnavailable,
        }
    }

    pub(crate) fn service_problem(&self) -> Option<&str> {
        self.connection_problem
            .as_deref()
            .or_else(|| self.pane().observation_problem())
    }

    pub(crate) fn input_status(&self) -> InputActivity {
        let pane = self.pane();
        let queued = pane
            .snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.agent.pending_inputs);
        let mut rejected = false;
        let pending = self.pending.values().find_map(|item| {
            if item.command.target.workspace != self.workspace
                || item.command.target.session_id != self.active.session
                || item.command.target.agent != self.active.agent
                || !matches!(item.command.action, Action::Send { .. })
            {
                return None;
            }
            if matches!(item.stage, PendingStage::Rejected(_)) {
                rejected = true;
                return None;
            }
            Some(SubmissionStatus::from_pending(&item.stage))
        });
        InputActivity {
            draft: !pane.draft.text().is_empty(),
            submission: pending
                .or_else(|| rejected.then_some(SubmissionStatus::Rejected))
                .or_else(|| pane.submission(queued)),
            queued,
        }
    }
}

impl Pane {
    pub(crate) fn activity(&self) -> Activity {
        if self.stopping
            || self
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.cleaning_up)
        {
            return Activity::Stopping;
        }
        if self.opening {
            return Activity::Opening;
        }
        if self.execution.is_some_and(|execution| {
            self.snapshot
                .as_ref()
                .is_none_or(|snapshot| execution > snapshot.cursor)
        }) {
            return self.progress.activity();
        }
        if let Some(snapshot) = &self.snapshot {
            return match snapshot.agent.status {
                AgentStatus::Running => self.progress.activity(),
                AgentStatus::Waiting => Activity::Waiting,
                AgentStatus::Stopped => Activity::Stopped,
                AgentStatus::Interrupted => Activity::Interrupted,
                AgentStatus::Idle if snapshot.agent.last_error.is_some() => Activity::Failed,
                AgentStatus::Idle if snapshot.agent.turn.is_some() => Activity::Finished,
                AgentStatus::Idle => Activity::Ready,
            };
        }
        if self.model_problem.is_some() {
            Activity::Configuration
        } else {
            Activity::Ready
        }
    }

    pub(super) fn record_submission(&mut self, cursor: u64, status: SubmissionStatus) {
        if self.submission.is_none_or(|(current, _)| cursor >= current) {
            self.submission = Some((cursor, status));
        }
    }

    fn submission(&self, queued: usize) -> Option<SubmissionStatus> {
        let (cursor, status) = self.submission?;
        if status == SubmissionStatus::Queued
            && queued == 0
            && self
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.cursor >= cursor)
        {
            return None;
        }
        Some(status)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use kuku_client::{
        Acknowledgement, Error, HistoryPage, Observation, Receipt, Snapshot, Timing,
    };
    use serde_json::{json, Value};

    use super::{Activity, App, HistoryEntry, InputStatus, ServiceStatus, SubmissionStatus};
    use crate::app::Address;
    use crate::jobs::{Job, Update};

    fn connected() -> App {
        let workspace = PathBuf::from("/workspace");
        let mut app = App::new(workspace.clone(), None);
        assert_eq!(ServiceStatus::Connecting, app.service_status());
        app.update(Update::Connected {
            endpoint: "http://127.0.0.1:6789".into(),
            workspace,
        });
        assert_eq!(ServiceStatus::Connected, app.service_status());
        app.effects.clear();
        app
    }

    fn snapshot(
        app: &App,
        cursor: u64,
        status: &str,
        execution: Option<u64>,
        queued: usize,
        progress: Value,
        error: Option<&str>,
    ) -> Box<Snapshot> {
        Box::new(
            serde_json::from_value(json!({
                "session_id": app.active.session,
                "workspace": app.workspace,
                "cursor": cursor,
                "agents": [],
                "pending": [],
                "progress": progress,
                "agent": {
                    "address": app.active.agent,
                    "status": status,
                    "turn": 5,
                    "pending_inputs": queued,
                    "execution_id": execution,
                    "last_error": error,
                    "config": {},
                    "usage": {"model_requests": 0, "unreported_requests": 0},
                }
            }))
            .unwrap(),
        )
    }

    fn loaded(app: &mut App, snapshot: Box<Snapshot>, entries: Vec<HistoryEntry>) {
        let selection = app.selection();
        app.update(Update::Loaded(
            selection.clone(),
            snapshot,
            HistoryPage {
                entries,
                next_before: None,
            },
        ));
        app.update(Update::WatchConnected(selection));
    }

    fn observe(app: &mut App, frame: Value) {
        app.update(Update::Observation(
            app.selection(),
            serde_json::from_value(frame).unwrap(),
        ));
    }

    fn streaming(app: &mut App, cursor: u64) {
        observe(
            app,
            json!({
                "type":"live", "agent":"main", "execution_id":5, "cursor":cursor,
                "event":{"type":"text","content":"First tokens"},
            }),
        );
    }

    fn committed(app: &mut App, id: u64, kind: &str, details: Value) {
        let mut frame = json!({"type":"committed","id":id,"agent":"main","kind":kind});
        frame
            .as_object_mut()
            .unwrap()
            .extend(details.as_object().unwrap().clone());
        observe(app, frame);
    }

    fn tool_result(app: &mut App, id: u64, tool_call_id: &str) {
        committed(
            app,
            id,
            "tool.result",
            json!({"entry":{"id":id,"agent":"main","type":"tool_result","tool_call_id":tool_call_id}}),
        );
    }

    #[test]
    fn upstream_wait_stream_parallel_tools_and_settlement_keep_service_connected() {
        let mut app = connected();
        app.select(app.active.clone(), true, String::new());
        let attached = snapshot(
            &app,
            6,
            "running",
            Some(5),
            0,
            json!({"phase":"waiting_for_model"}),
            None,
        );
        loaded(&mut app, attached, Vec::new());
        assert_eq!(Activity::WaitingForModel, app.pane().activity());
        assert_eq!(ServiceStatus::Connected, app.service_status());
        streaming(&mut app, 7);
        assert_eq!(Activity::Streaming, app.pane().activity());
        committed(&mut app, 8, "model.response", json!({}));
        streaming(&mut app, 7);
        assert_eq!(Activity::Running, app.pane().activity());

        committed(
            &mut app,
            9,
            "runtime.tool_dispatched",
            json!({"tool_call_id": "tool-a"}),
        );
        committed(
            &mut app,
            10,
            "runtime.tool_dispatched",
            json!({"tool_call_id": "tool-b"}),
        );
        assert_eq!(Activity::RunningTools, app.pane().activity());
        tool_result(&mut app, 11, "tool-a");
        assert_eq!(Activity::RunningTools, app.pane().activity());
        tool_result(&mut app, 12, "tool-b");
        assert_eq!(Activity::Running, app.pane().activity());
        committed(&mut app, 13, "runtime.step_prepared", json!({}));
        assert_eq!(Activity::WaitingForModel, app.pane().activity());
        let finished = snapshot(&app, 16, "idle", None, 0, Value::Null, None);
        app.update(Update::Refreshed(app.selection(), Ok(finished)));
        assert_eq!(Activity::Finished, app.pane().activity());
        streaming(&mut app, 15);
        assert_eq!(Activity::Finished, app.pane().activity());
        let failed = snapshot(
            &app,
            20,
            "idle",
            None,
            0,
            Value::Null,
            Some("Provider response timed out"),
        );
        app.update(Update::Refreshed(app.selection(), Ok(failed)));
        assert_eq!(Activity::Failed, app.pane().activity());
        assert_eq!(ServiceStatus::Connected, app.service_status());
    }

    #[test]
    fn reconnect_discards_live_preview_without_changing_drafts_and_rejects_old_target() {
        let mut app = connected();
        app.select(app.active.clone(), true, String::new());
        let attached = snapshot(
            &app,
            6,
            "running",
            Some(5),
            0,
            json!({"phase":"waiting_for_model"}),
            None,
        );
        loaded(&mut app, attached, Vec::new());
        let active = app.selection();
        app.pane_mut().draft.insert("Keep this draft");
        streaming(&mut app, 7);
        app.update(Update::WatchReconnecting(
            active.clone(),
            "service observation ended".into(),
        ));
        assert_eq!(ServiceStatus::Reconnecting, app.service_status());
        assert_eq!(Activity::WaitingForModel, app.pane().activity());
        assert!(app.service_problem().unwrap().contains("observation ended"));
        assert_eq!("Keep this draft", app.pane().draft.text());
        app.update(Update::WatchConnected(active.clone()));
        app.update(Update::Observation(
            active.clone(),
            Observation::Lagged { skipped: 0 },
        ));
        assert_eq!(ServiceStatus::Connected, app.service_status());
        assert_eq!(None, app.service_problem());

        let reviewer = Address::new(app.active.session.clone(), "reviewer".into());
        app.select(reviewer, true, String::new());
        let unauthorized = || Error::Service {
            status: 401,
            code: "unauthorized".into(),
            error_message: "Sign in again".into(),
        };
        app.update(Update::WatchFailed(active, unauthorized()));
        assert_eq!(ServiceStatus::Connecting, app.service_status());
        app.update(Update::WatchFailed(app.selection(), unauthorized()));
        assert_eq!(ServiceStatus::UpdatesUnavailable, app.service_status());
        assert!(app.service_problem().unwrap().contains("Sign in again"));
    }

    #[test]
    fn draft_confirmation_durable_queue_and_admission_are_independent() {
        let mut app = connected();
        app.select(app.active.clone(), true, String::new());
        let attached = snapshot(&app, 6, "running", Some(5), 0, Value::Null, None);
        loaded(&mut app, attached, Vec::new());
        app.pane_mut().draft.insert("Next turn");
        assert!(app.input_status().draft);
        app.send(Timing::NextTurn);
        let Some(Job::Save(command)) = app.effects.pop() else {
            panic!("send must be saved before transmission")
        };
        assert_eq!(
            Some(SubmissionStatus::Saving),
            app.input_status().submission
        );
        app.update(Update::Saved(command.clone(), Ok(())));
        app.pane_mut().draft.insert("Another idea");
        assert!(app.input_status().draft);
        assert_eq!(
            Some(SubmissionStatus::AwaitingReceipt),
            app.input_status().submission
        );
        app.update(Update::Sent(
            command.command_id.clone(),
            Ok(Acknowledgement {
                receipt: Receipt {
                    command_id: command.command_id,
                    event_id: 7,
                    agent: "main".into(),
                },
                replayed: false,
                status: Some(InputStatus::Queued),
            }),
        ));
        assert_eq!(
            Some(SubmissionStatus::Queued),
            app.input_status().submission
        );
        let queued = snapshot(&app, 7, "running", Some(5), 1, Value::Null, None);
        app.update(Update::Refreshed(app.selection(), Ok(queued)));
        assert_eq!(1, app.input_status().queued);
        committed(
            &mut app,
            8,
            "runtime.operation",
            json!({"operation":"inputs_accepted","state_changed":true}),
        );
        let admitted = snapshot(&app, 8, "running", Some(5), 0, Value::Null, None);
        app.update(Update::Refreshed(app.selection(), Ok(admitted)));
        assert_eq!(0, app.input_status().queued);
        assert_eq!(
            Some(SubmissionStatus::Accepted),
            app.input_status().submission
        );
        assert!(app.input_status().draft);
    }

    #[test]
    fn first_attachment_observes_input_already_accepted_after_queued_receipt() {
        let mut app = connected();
        app.pane_mut().draft.insert("First send");
        app.send(Timing::Steer);
        let Some(Job::Save(command)) = app.effects.pop() else {
            panic!("send must be saved before transmission")
        };
        app.update(Update::Saved(command.clone(), Ok(())));
        app.update(Update::Sent(
            command.command_id.clone(),
            Ok(Acknowledgement {
                receipt: Receipt {
                    command_id: command.command_id,
                    event_id: 3,
                    agent: "main".into(),
                },
                replayed: false,
                status: Some(InputStatus::Queued),
            }),
        ));
        let attached = snapshot(&app, 6, "running", Some(4), 0, Value::Null, None);
        let input = serde_json::from_value(json!({
            "id":6,"agent":"main","type":"user","text":"First send"
        }))
        .unwrap();
        loaded(&mut app, attached, vec![input]);
        assert_eq!(
            Some(SubmissionStatus::Accepted),
            app.input_status().submission
        );
    }
}
