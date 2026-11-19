use std::collections::HashSet;

use kuku_client::{
    Action, AgentProgress, AgentStatus, HistoryEntry, HistoryKind, InputStatus, ModelPhase,
    Observation, ReplyPreview,
};
use serde_json::Value;

use super::{App, Pane, PendingStage, SnapshotRefresh};

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
    RetryingModel,
    Thinking,
    Streaming,
    ToolArguments,
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
            Self::RetryingModel => "Retrying model",
            Self::Thinking => "Thinking",
            Self::Streaming => "Streaming reply",
            Self::ToolArguments => "Writing tool arguments",
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
    request: ModelRequest,
    output: Option<ModelPhase>,
    tools: HashSet<String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
enum ModelRequest {
    #[default]
    None,
    Waiting,
    Retrying,
}

impl ExecutionProgress {
    pub(super) fn from_snapshot(progress: Option<&AgentProgress>) -> Self {
        match progress {
            Some(AgentProgress::WaitingForModel) => Self {
                request: ModelRequest::Waiting,
                ..Self::default()
            },
            Some(AgentProgress::Thinking) => Self {
                output: Some(ModelPhase::Thinking),
                ..Self::default()
            },
            Some(AgentProgress::Streaming { .. }) => Self {
                output: Some(ModelPhase::Streaming),
                ..Self::default()
            },
            Some(AgentProgress::ToolArguments) => Self {
                output: Some(ModelPhase::ToolArguments),
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
        self.output = None;
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
                    request: ModelRequest::Waiting,
                    ..Self::default()
                };
            }
            "runtime.model_retry" | "model.recovery" => {
                *self = Self {
                    request: ModelRequest::Retrying,
                    ..Self::default()
                };
            }
            "model.response" | "model.error" => {
                self.request = ModelRequest::None;
                self.clear_stream();
            }
            "runtime.tool_dispatched" => {
                if let Some(id) = tool_call_id {
                    self.tools.insert(id);
                }
            }
            "tool.result" => {
                if let Some(id) = entry
                    .and_then(|entry| entry.details["tool_call_id"].as_str())
                    .or(tool_call_id.as_deref())
                {
                    self.tools.remove(id);
                }
            }
            _ => {}
        }
    }

    pub(super) fn live(&mut self, event: &Value) {
        match event["type"].as_str() {
            Some("model_request") => {
                if self.request != ModelRequest::Retrying {
                    self.request = ModelRequest::Waiting;
                }
                self.clear_stream();
            }
            Some("model_recovery") => {
                self.request = ModelRequest::Retrying;
                self.clear_stream();
            }
            Some("thinking" | "reasoning_preview")
                if event
                    .get("content")
                    .is_none_or(|value| value.as_str().is_some_and(|text| !text.is_empty())) =>
            {
                self.output = Some(ModelPhase::Thinking);
            }
            Some("tool_arguments") => self.output = Some(ModelPhase::ToolArguments),
            Some("reply_preview") => {
                if let Ok(phase) = serde_json::from_value(event["phase"].clone()) {
                    self.output = Some(phase);
                }
            }
            Some("text")
                if event["content"]
                    .as_str()
                    .is_some_and(|text| !text.is_empty()) =>
            {
                self.output = Some(ModelPhase::Streaming);
            }
            _ => {}
        }
    }

    pub(super) fn activity(&self) -> Activity {
        if !self.tools.is_empty() {
            return Activity::RunningTools;
        }
        match (self.output, &self.request) {
            (Some(ModelPhase::Streaming), _) => Activity::Streaming,
            (Some(ModelPhase::Thinking), _) => Activity::Thinking,
            (Some(ModelPhase::ToolArguments), _) => Activity::ToolArguments,
            (None, ModelRequest::Waiting) => Activity::WaitingForModel,
            (None, ModelRequest::Retrying) => Activity::RetryingModel,
            (None, ModelRequest::None) => Activity::Running,
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

    pub(super) fn from_pending(stage: &PendingStage) -> Self {
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
    pub(super) fn observe(&mut self, event: Observation) {
        let active_agent = self.active.agent.clone();
        if matches!(
            &event,
            Observation::Committed {
                agent: Some(agent),
                operation: Some(operation),
                ..
            } if agent == &active_agent && operation == "message_rewind"
        ) {
            self.refresh_rewind_history();
        }
        self.related_agents.observe(&event, &active_agent);
        let pane = self.pane_mut();
        match event {
            Observation::Committed {
                id,
                agent,
                entry,
                state_changed,
                turn,
                turn_started_at_ms,
                kind,
                operation,
                delivery,
                tool_call_id,
                ..
            } => {
                if id <= pane.last_event {
                    return;
                }
                pane.last_event = id;
                let same_agent = agent.as_deref() == Some(&active_agent);
                let request_changed = same_agent
                    && (matches!(kind.as_str(), "runtime.step_prepared" | "model.response")
                        || matches!(
                            operation.as_deref(),
                            Some("agent_configured" | "step_configured")
                        ));
                if request_changed {
                    pane.request_usage_floor = id;
                }
                if same_agent {
                    let elapsed = entry.as_ref().and_then(|entry| entry.turn_summary.as_ref());
                    pane.observe_timing(
                        id,
                        turn.or_else(|| entry.as_ref().and_then(|entry| entry.turn)),
                        turn_started_at_ms,
                        elapsed.and_then(|summary| summary.elapsed_ms),
                    );
                    if let Some(delivery) = delivery {
                        pane.deliveries.committed(id, *delivery);
                    }
                    pane.observe_execution(
                        id,
                        &kind,
                        operation.as_deref(),
                        entry.as_deref(),
                        tool_call_id,
                    );
                    if operation.as_deref() == Some("inputs_accepted") {
                        pane.record_submission(id, SubmissionStatus::Accepted);
                    }
                }
                if let Some(entry) = entry.filter(|entry| entry.agent == active_agent) {
                    if entry.kind == HistoryKind::User {
                        if let Some(text) = &entry.text {
                            pane.draft.remember(text);
                        }
                    }
                    pane.transcript.apply(*entry, pane.execution);
                }
                if state_changed || request_changed {
                    self.request_refresh(SnapshotRefresh::AfterCommit(id));
                }
            }
            Observation::Live {
                agent,
                execution_id,
                turn,
                turn_number,
                cursor,
                event,
            } => {
                let terminal =
                    matches!(event["type"].as_str(), Some("done" | "cancelled" | "error"));
                if agent == active_agent
                    && cursor >= pane.live_floor
                    && execution_id > pane.retired_execution_floor
                    && pane
                        .execution
                        .is_none_or(|execution| execution_id >= execution)
                {
                    if !terminal && pane.execution != Some(execution_id) {
                        pane.execution = Some(execution_id);
                        pane.progress = ExecutionProgress::default();
                    }
                    pane.note_live_reply(cursor, &event);
                    pane.transcript
                        .live(execution_id, turn, turn_number, &event);
                }
            }
            Observation::Lagged { .. } => {
                pane.progress.clear_stream();
                pane.transcript.clear_preview();
                let cursor = pane.last_event;
                self.related_agents.invalidate(cursor);
                self.request_refresh(SnapshotRefresh::Progress);
            }
            Observation::Heartbeat => {}
        }
    }

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
    pub(super) fn observe_execution(
        &mut self,
        id: u64,
        kind: &str,
        operation: Option<&str>,
        entry: Option<&HistoryEntry>,
        tool_call_id: Option<String>,
    ) {
        if id <= self.retired_execution_floor || self.execution.is_some_and(|active| id < active) {
            return;
        }
        match operation {
            Some("execution_started") => {
                if self.execution != Some(id) {
                    self.progress = ExecutionProgress::default();
                }
                self.execution = Some(id);
                self.live_floor = id;
            }
            Some("turn_settled") => {
                self.execution = None;
                self.progress = ExecutionProgress::default();
                self.transcript.clear_preview();
                self.live_floor = id;
                self.retired_execution_floor = id;
            }
            Some("agent_stopped" | "permission_waiting") => {
                self.live_floor = id;
                self.progress.clear_stream();
                self.transcript.clear_reply_preview();
            }
            _ => {}
        }
        if matches!(
            kind,
            "runtime.step_prepared"
                | "runtime.model_retry"
                | "model.recovery"
                | "model.response"
                | "model.error"
        ) {
            self.live_floor = id;
            if id <= self.latest_reply_cursor {
                return;
            }
            if kind != "model.response" {
                self.transcript.clear_reply_preview();
            }
        }
        self.progress.committed(kind, entry, tool_call_id);
    }

    pub(super) fn note_live_reply(&mut self, cursor: u64, event: &Value) {
        if matches!(
            event["type"].as_str(),
            Some(
                "text"
                    | "thinking"
                    | "tool_arguments"
                    | "reply_preview"
                    | "model_request"
                    | "model_recovery"
            )
        ) {
            self.latest_reply_cursor = self.latest_reply_cursor.max(cursor);
        }
        self.progress.live(event);
    }

    pub(super) fn apply_reply_preview(&mut self, reply: ReplyPreview) {
        self.latest_reply_cursor = self.live_floor;
        self.progress.output = Some(reply.phase);
        self.transcript.replace_reply_preview(
            reply.execution_id,
            Some(reply.turn),
            reply.turn_number,
            reply.text,
            reply.truncated,
        );
        if reply.phase == ModelPhase::Thinking {
            self.transcript
                .begin_thinking(reply.execution_id, Some(reply.turn), reply.turn_number);
        }
        self.transcript.replace_summary(
            reply.execution_id,
            Some(reply.turn),
            reply.turn_number,
            &reply.reasoning_preview,
            reply.reasoning_preview_truncated,
        );
    }

    pub(crate) fn activity(&self) -> Activity {
        if self.interruption_pending()
            || self.snapshot.as_ref().is_some_and(|snapshot| {
                snapshot.cleaning_up && snapshot.execution_id() == self.execution
            })
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
        let settled = match status {
            SubmissionStatus::Queued => queued == 0,
            SubmissionStatus::Accepted => matches!(
                self.activity(),
                Activity::Finished | Activity::Interrupted | Activity::Stopped | Activity::Failed
            ),
            _ => false,
        };
        if settled
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
mod tests;
