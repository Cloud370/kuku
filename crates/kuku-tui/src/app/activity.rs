use std::collections::{HashMap, HashSet};

use kuku_client::local::LocalError;
use kuku_client::{
    Action, AgentProgress, AgentStatus, Error, HistoryEntry, HistoryKind, InputStatus, ModelPhase,
    Observation, ReplyPreview, RetryMetadata, RetryStopReason,
};
use serde_json::Value;

use super::{Address, App, Pane, PendingStage, SnapshotRefresh};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServiceStatus {
    Connecting,
    SetupRequired,
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
            Self::SetupRequired => "Model setup required",
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

    pub(crate) fn shows_sending(self) -> bool {
        matches!(
            self,
            Self::Ready | Self::Finished | Self::Stopped | Self::Interrupted | Self::Failed
        )
    }

    pub(super) fn executing(self) -> bool {
        matches!(
            self,
            Self::Running
                | Self::WaitingForModel
                | Self::RetryingModel
                | Self::Thinking
                | Self::Streaming
                | Self::ToolArguments
                | Self::RunningTools
        )
    }
}

#[derive(Debug, Default)]
pub(super) struct ExecutionProgress {
    request: ModelRequest,
    output: Option<ModelPhase>,
    tools: HashSet<String>,
    terminal_error: bool,
    model_failed: bool,
    preparing_file_contents: bool,
    permission_wait: Option<DirectWait>,
    question_wait: Option<DirectWait>,
    script_waits: HashMap<String, (u64, String)>,
    settled_script_waits: HashSet<String>,
    output_meter: OutputMeter,
}

#[derive(Debug)]
struct DirectWait {
    id: String,
    cursor: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WaitKind {
    Permission,
    Question,
}

#[derive(Debug, Clone, Copy, Default)]
struct OutputMeter {
    bytes: u64,
    reported: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutputTokens {
    Estimated(u64),
    Reported(u64),
}

impl OutputMeter {
    fn preview(bytes: Option<u64>, tokens: Option<u64>, text: &str, truncated: bool) -> Self {
        Self {
            bytes: bytes.unwrap_or(if truncated { 0 } else { text.len() as u64 }),
            reported: tokens,
        }
    }

    fn tokens(self) -> Option<OutputTokens> {
        self.reported
            .map(OutputTokens::Reported)
            .or_else(|| (self.bytes > 0).then_some(OutputTokens::Estimated(self.bytes.div_ceil(4))))
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
enum ModelRequest {
    #[default]
    None,
    Waiting,
    Retrying(RetryMetadata),
    RetryStopped(RetryMetadata),
    RetryUnknown(RetryMetadata),
}

impl ModelRequest {
    fn from_retry(retry: RetryMetadata) -> Self {
        match crate::retry_notice::decision(&retry) {
            Some(true) => Self::Retrying(retry),
            Some(false) => Self::RetryStopped(retry),
            None => Self::RetryUnknown(retry),
        }
    }
}

impl ExecutionProgress {
    pub(super) fn from_snapshot(progress: Option<&AgentProgress>) -> Self {
        match progress {
            Some(AgentProgress::WaitingForModel) => Self {
                request: ModelRequest::Waiting,
                ..Self::default()
            },
            Some(
                AgentProgress::RetryingModel { retry }
                | AgentProgress::RetryStopped { retry }
                | AgentProgress::RetryDecisionUnknown { retry },
            ) => Self {
                request: ModelRequest::from_retry(retry.clone()),
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
        self.preparing_file_contents = false;
    }

    pub(super) fn disconnected(&mut self) {
        // The subscription resumes committed events from its last cursor, so
        // already-observed dispatches may never be delivered a second time.
        // Only the transient preview is invalidated by a disconnection.
        self.clear_stream();
        self.output_meter = OutputMeter::default();
    }

    pub(super) fn reconcile_snapshot(
        &mut self,
        projection: &AgentProgress,
        same_live_cursor: bool,
    ) {
        let mut projected = Self::from_snapshot(Some(projection));
        if matches!(projection, AgentProgress::RunningTools { .. }) {
            // This is the durable set of unfinished calls. A reply preview at
            // the same cursor can be newer than the HTTP response, however.
            self.tools = std::mem::take(&mut projected.tools);
            self.script_waits
                .retain(|_, (_, call)| self.tools.contains(call));
            if self.tools.is_empty() {
                self.settled_script_waits.clear();
            }
        }
        if same_live_cursor && self.output.is_some() {
            return;
        }
        self.request = projected.request;
        if self.output != projected.output {
            self.output_meter = OutputMeter::default();
            self.preparing_file_contents = false;
        }
        self.output = projected.output;
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
                    tools: std::mem::take(&mut self.tools),
                    script_waits: std::mem::take(&mut self.script_waits),
                    settled_script_waits: std::mem::take(&mut self.settled_script_waits),
                    ..Self::default()
                };
            }
            "runtime.model_retry" | "model.recovery" => {
                let retry = crate::retry_notice::history_metadata(entry, kind);
                let request = ModelRequest::from_retry(retry);
                let stopped = matches!(request, ModelRequest::RetryStopped(_));
                if kind == "runtime.model_retry" && self.output == Some(ModelPhase::Streaming) {
                    if stopped {
                        self.request = request;
                        self.terminal_error = true;
                    } else {
                        self.request = ModelRequest::None;
                        self.terminal_error = false;
                    }
                } else {
                    *self = Self {
                        request,
                        tools: std::mem::take(&mut self.tools),
                        script_waits: std::mem::take(&mut self.script_waits),
                        settled_script_waits: std::mem::take(&mut self.settled_script_waits),
                        terminal_error: stopped,
                        ..Self::default()
                    };
                }
            }
            "model.response" => {
                self.model_failed = false;
                self.request = ModelRequest::None;
                self.terminal_error = false;
                self.clear_stream();
            }
            "model.error" => {
                self.model_failed = true;
                self.terminal_error = matches!(&self.request, ModelRequest::RetryStopped(_));
                if !self.terminal_error {
                    self.request = ModelRequest::None;
                    self.clear_stream();
                }
            }
            "turn.cancelled" => *self = Self::default(),
            "turn.interrupted" => self.retain_terminal_notice(),
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
                    self.script_waits.retain(|_, (_, call)| call != id);
                    if self.tools.is_empty() {
                        self.settled_script_waits.clear();
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn live(&mut self, event: &Value) {
        match event["type"].as_str() {
            Some("model_request") => {
                self.model_failed = false;
                self.permission_wait = None;
                self.question_wait = None;
                if !matches!(&self.request, ModelRequest::Retrying(_)) {
                    self.request = ModelRequest::Waiting;
                }
                self.clear_stream();
                self.output_meter = OutputMeter::default();
            }
            Some("model_recovery") => {
                self.request = ModelRequest::Retrying(crate::retry_notice::live_recovery(event));
                self.terminal_error = false;
                self.clear_stream();
                self.output_meter = OutputMeter::default();
            }
            Some("model_output_usage") => {
                if let Some(count) = event["output_tokens"].as_u64() {
                    self.output_meter.reported = Some(count);
                }
            }
            Some("thinking" | "reasoning_preview")
                if event
                    .get("content")
                    .is_none_or(|value| value.as_str().is_some_and(|text| !text.is_empty())) =>
            {
                if !self.is_stopped_retry() {
                    self.request = ModelRequest::None;
                }
                self.output = Some(ModelPhase::Thinking);
                self.preparing_file_contents = false;
            }
            Some("tool_arguments") => {
                if !self.is_stopped_retry() {
                    self.request = ModelRequest::None;
                }
                self.output = Some(ModelPhase::ToolArguments);
                self.preparing_file_contents = event["preparing_file_contents"]
                    .as_bool()
                    .unwrap_or_default();
            }
            Some("reply_preview") => {
                if let Ok(phase) = serde_json::from_value(event["phase"].clone()) {
                    if !self.is_stopped_retry() {
                        self.request = ModelRequest::None;
                    }
                    self.output = Some(phase);
                    self.preparing_file_contents = event["preparing_file_contents"]
                        .as_bool()
                        .unwrap_or_default();
                    self.output_meter = OutputMeter::preview(
                        event["output_bytes"].as_u64(),
                        event["output_tokens"].as_u64(),
                        event["content"].as_str().unwrap_or_default(),
                        event["truncated"].as_bool().unwrap_or_default(),
                    );
                }
            }
            Some("text")
                if event["content"]
                    .as_str()
                    .is_some_and(|text| !text.is_empty()) =>
            {
                if !self.is_stopped_retry() {
                    self.request = ModelRequest::None;
                }
                self.output = Some(ModelPhase::Streaming);
                self.preparing_file_contents = false;
                let bytes = event["content"]
                    .as_str()
                    .map_or(0, |text| text.len() as u64);
                self.output_meter.bytes = self.output_meter.bytes.saturating_add(bytes);
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
            (None, ModelRequest::Retrying(_)) => Activity::RetryingModel,
            (None, ModelRequest::RetryStopped(_) | ModelRequest::RetryUnknown(_)) => {
                Activity::Running
            }
            (None, ModelRequest::None) => Activity::Running,
        }
    }

    pub(super) fn retry(&self) -> Option<&RetryMetadata> {
        match &self.request {
            ModelRequest::RetryStopped(retry) => Some(retry),
            ModelRequest::Retrying(retry) | ModelRequest::RetryUnknown(retry)
                if self.output.is_none() =>
            {
                Some(retry)
            }
            ModelRequest::None | ModelRequest::Waiting => None,
            ModelRequest::Retrying(_) | ModelRequest::RetryUnknown(_) => None,
        }
    }

    fn preserve_reply_preview_on_settle(&self) -> bool {
        matches!(
            &self.request,
            ModelRequest::RetryStopped(retry)
                if retry.stop_reason == Some(RetryStopReason::StreamStarted)
        )
    }

    fn is_stopped_retry(&self) -> bool {
        matches!(&self.request, ModelRequest::RetryStopped(_))
    }

    fn retain_terminal_notice(&mut self) {
        if !self.terminal_error {
            *self = Self {
                output_meter: self.output_meter,
                model_failed: self.model_failed,
                permission_wait: self.permission_wait.take(),
                question_wait: self.question_wait.take(),
                ..Self::default()
            };
        } else {
            self.output = None;
            self.tools.clear();
            self.script_waits.clear();
            self.settled_script_waits.clear();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubmissionStatus {
    Saving,
    AwaitingReceipt,
    AwaitingAdmission,
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
            Self::AwaitingAdmission => "Sending",
            Self::CheckingReceipt => "Checking receipt",
            Self::ReceiptUnconfirmed => "Receipt unconfirmed",
            Self::ReceiptMissing => "Receipt missing",
            Self::Rejected => "Send rejected",
            Self::Discarding => "Discarding send",
            Self::Queued => "Input queued",
            Self::Accepted => "Input accepted",
            Self::Withdrawn => "Removed from queue",
            Self::Confirmed => "Receipt confirmed",
        }
    }

    pub(crate) fn is_sending(self) -> bool {
        matches!(
            self,
            Self::Saving | Self::AwaitingReceipt | Self::AwaitingAdmission
        )
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
        let active_address = self.active.clone();
        let active_agent = active_address.agent.clone();
        if let Observation::Committed {
            id,
            agent: Some(agent),
            operation: Some(operation),
            rewind_command_id,
            settlement,
            ..
        } = &event
        {
            // The selected Agent's observer also sees peer commits in this
            // Session. Invalidate requests on cached panes before navigation
            // can expose those older snapshots again.
            let address = Address::new(active_address.session.clone(), agent.clone());
            if let Some(pane) = self.panes.get_mut(&address) {
                pane.note_request_state_change(
                    *id,
                    Some(operation.as_str()),
                    settlement.as_ref().map(|settlement| settlement.status),
                );
            }
            if agent == &active_agent
                && matches!(operation.as_str(), "message_rewind" | "history_jump")
                && *id > self.pane().last_event
            {
                if let Some(command) = rewind_command_id {
                    if let Some(mut draft) = self.rewind_drafts.remove(command) {
                        self.restore_rewind_draft(&mut draft);
                        self.rewind_drafts.insert(command.clone(), draft);
                    }
                }
                self.refresh_rewind_history(*id);
            }
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
                let mut execution_started = false;
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
                    execution_started = pane.observe_execution(
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
                let mut jobs_changed = false;
                if let Some(entry) = entry.filter(|entry| entry.agent == active_agent) {
                    jobs_changed = entry.kind == HistoryKind::JobFinished
                        || (entry.kind == HistoryKind::ToolResult
                            && entry
                                .details
                                .get("structured")
                                .is_some_and(|facts| facts["kind"] == "job"));
                    if entry.kind == HistoryKind::User {
                        pane.remember_history_entry(&entry);
                    }
                    pane.transcript.apply(*entry, pane.execution);
                }
                if state_changed || request_changed {
                    self.request_refresh(SnapshotRefresh::AfterCommit(id));
                }
                if execution_started {
                    self.clear_execution_notice_on_start(&active_address, id);
                }
                if jobs_changed {
                    self.refresh_jobs();
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
                    let mut execution_started = false;
                    if !terminal && pane.execution != Some(execution_id) {
                        pane.execution = Some(execution_id);
                        pane.latest_execution_started = Some(
                            pane.latest_execution_started
                                .map_or(execution_id, |latest| latest.max(execution_id)),
                        );
                        pane.progress = ExecutionProgress::default();
                        execution_started = true;
                    }
                    pane.note_live_reply(cursor, &event);
                    pane.transcript
                        .live(execution_id, turn, turn_number, &event);
                    if execution_started {
                        self.clear_execution_notice_on_start(&active_address, execution_id);
                    }
                }
            }
            Observation::Lagged { .. } => {
                pane.progress.clear_stream();
                pane.progress.output_meter = OutputMeter::default();
                pane.transcript.clear_preview();
                let cursor = pane.last_event;
                self.related_agents.invalidate(cursor);
                self.request_refresh(SnapshotRefresh::Progress);
            }
            Observation::WorkspacePermission { .. } | Observation::Heartbeat => {}
        }
    }

    pub(super) fn connect_failed(&mut self, error: Error) {
        self.connecting = matches!(error, Error::Local(LocalError::BuildDeferred { .. }));
        let notice = match error {
            Error::Local(LocalError::BuildDeferred { .. }) => {
                "Waiting for the previous version to finish.\nThis window will connect automatically."
                    .into()
            }
            Error::Local(LocalError::BuildMismatch { running, .. }) => {
                format!("kuku {running} is already running.\nOpen that version to continue.")
            }
            _ if self.remote => format!(
                "kuku service connection · {error}\nCheck the service URL and token, then press Ctrl+R"
            ),
            _ => format!(
                "kuku service connection · {error}\nUse Ctrl+R to reconnect, /server for status, or /restart"
            ),
        };
        self.connection_problem = Some(notice);
    }

    pub(super) fn watch_failed(&mut self, error: Error) {
        if matches!(error, Error::Local(LocalError::BuildMismatch { .. })) {
            self.drop_watch(Watch::Detached);
            self.connect_failed(error);
        } else {
            self.drop_watch(Watch::Failed(format!(
                "kuku service live updates unavailable · Ctrl+R to reconnect · {error}"
            )));
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
        if self.setup_required {
            return ServiceStatus::SetupRequired;
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
        let queued = pane.snapshot.as_ref().map_or(0, |snapshot| {
            let job_notices = snapshot.pending.iter().filter(|entry|
                matches!(&entry.input.source, kuku_client::InputSource::External(source) if source == "session_job")
            ).count();
            snapshot.agent.pending_inputs.saturating_sub(job_notices + pane.deliveries.hidden_queue_count(snapshot))
        });
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
                .or_else(|| {
                    pane.deliveries
                        .awaiting_admission()
                        .then_some(SubmissionStatus::AwaitingAdmission)
                })
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
    ) -> bool {
        if id <= self.live_floor
            || id <= self.retired_execution_floor
            || self.execution.is_some_and(|active| id < active)
        {
            return false;
        }
        self.live_floor = id;
        let execution_started = matches!(
            operation,
            Some("execution_started" | "compaction_requested")
        );
        match operation {
            Some("execution_started" | "compaction_requested") => {
                if self.execution != Some(id) {
                    self.progress = ExecutionProgress::default();
                }
                self.execution = Some(id);
                self.latest_execution_started = Some(
                    self.latest_execution_started
                        .map_or(id, |latest| latest.max(id)),
                );
                self.live_floor = id;
            }
            Some("turn_settled" | "compaction_settled" | "execution_deferred") => {
                self.execution = None;
                if !self.progress.preserve_reply_preview_on_settle() {
                    self.transcript.clear_preview();
                }
                self.progress.retain_terminal_notice();
                self.live_floor = id;
                self.retired_execution_floor = id;
            }
            Some("agent_stopped" | "message_rewind" | "history_jump") => {
                self.live_floor = id;
                self.progress = ExecutionProgress::default();
                self.transcript.clear_reply_preview();
                if operation == Some("agent_stopped") {
                    self.retired_execution_floor = self.retired_execution_floor.max(id);
                }
            }
            Some("permission_waiting") => {
                if let Some(request_id) = entry.and_then(|entry| {
                    entry
                        .details
                        .get("request")
                        .and_then(|request| request["id"].as_str())
                }) {
                    self.progress.permission_wait = Some(DirectWait {
                        id: request_id.to_owned(),
                        cursor: id,
                    });
                }
                self.progress.clear_stream();
                self.transcript.clear_reply_preview();
            }
            Some("question_requested") => {
                if let Some(request_id) = entry.and_then(|entry| {
                    entry
                        .details
                        .get("request")
                        .and_then(|request| request["id"].as_str())
                }) {
                    self.progress.question_wait = Some(DirectWait {
                        id: request_id.to_owned(),
                        cursor: id,
                    });
                }
            }
            Some("script_permission_waiting") => {
                if let Some(entry) = entry
                    && let Some(turn) = entry.turn
                    && let Some(call) = entry
                        .details
                        .get("request")
                        .and_then(|request| request["tool_call_id"].as_str())
                    && let Some(request_id) = entry
                        .details
                        .get("request")
                        .and_then(|request| request["id"].as_str())
                    && self.foreground_script_call(turn, call)
                {
                    self.progress
                        .script_waits
                        .insert(request_id.to_owned(), (turn, call.to_owned()));
                }
            }
            Some("permission_replied" | "question_replied") => {
                let wait = if operation == Some("permission_replied") {
                    &mut self.progress.permission_wait
                } else {
                    &mut self.progress.question_wait
                };
                if let Some(request_id) =
                    entry.and_then(|entry| entry.details.get("request_id").and_then(Value::as_str))
                    && wait.as_ref().is_some_and(|wait| wait.id == request_id)
                {
                    *wait = None;
                }
            }
            Some("script_permission_replied" | "script_permission_ended") => {
                if let Some(request_id) =
                    entry.and_then(|entry| entry.details.get("request_id").and_then(Value::as_str))
                {
                    self.progress.script_waits.remove(request_id);
                    if !self.progress.tools.is_empty() {
                        self.progress
                            .settled_script_waits
                            .insert(request_id.to_owned());
                    }
                }
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
                return execution_started;
            }
            let preserve_preview = matches!(kind, "runtime.model_retry" | "model.recovery")
                || (kind == "model.error" && self.progress.is_stopped_retry());
            if kind != "model.response" && !preserve_preview {
                self.transcript.clear_reply_preview();
            }
        }
        self.progress.committed(kind, entry, tool_call_id);
        execution_started
    }

    pub(crate) fn retry_metadata(&self) -> Option<&RetryMetadata> {
        self.progress.retry()
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
        self.progress.preparing_file_contents = reply.preparing_file_contents;
        self.progress.output_meter = OutputMeter::preview(
            reply.output_bytes,
            reply.output_tokens,
            &reply.text,
            reply.truncated,
        );
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

    pub(crate) fn composer_activity(&self) -> Activity {
        let activity = self.activity();
        if activity == Activity::RunningTools {
            return match self.progress.output {
                Some(ModelPhase::Streaming) => Activity::Streaming,
                Some(ModelPhase::Thinking) => Activity::Thinking,
                Some(ModelPhase::ToolArguments) => Activity::ToolArguments,
                None => activity,
            };
        }
        activity
    }

    // Execution identity is projected from observation, not only from an
    // asynchronously refreshed snapshot. A new execution can already be
    // running while that snapshot still describes the previous idle turn.
    pub(crate) fn running_execution(&self) -> Option<u64> {
        let execution = self.execution?;
        (execution > self.retired_execution_floor
            && self.activity().executing()
            && !self.observed_direct_wait(self.progress.permission_wait.as_ref()))
        .then_some(execution)
    }

    pub(crate) fn busy_or_waiting(&self) -> bool {
        let activity = self.activity();
        activity.executing()
            || matches!(
                activity,
                Activity::Opening | Activity::Waiting | Activity::Stopping
            )
    }

    pub(crate) fn running_tool(&self) -> Option<(&str, &str, &str)> {
        if self.progress.tools.len() != 1 {
            return None;
        }
        let id = self.progress.tools.iter().next()?;
        let (name, target) = self.transcript.tool_activity(id)?;
        Some((id, name, target))
    }

    pub(crate) fn preparing_file_contents(&self) -> bool {
        self.progress.preparing_file_contents
    }

    fn foreground_script_call(&self, turn: u64, id: &str) -> bool {
        self.progress.tools.contains(id) && self.transcript.foreground_script_call(turn, id)
    }

    fn observed_direct_wait(&self, wait: Option<&DirectWait>) -> bool {
        wait.is_some_and(|wait| {
            self.snapshot
                .as_ref()
                .is_none_or(|snapshot| snapshot.cursor < wait.cursor)
        })
    }

    pub(crate) fn waiting_kind(
        &self,
        approvals: &[kuku_client::PendingApproval],
    ) -> Option<WaitKind> {
        if self.request_state_unconfirmed() {
            return None;
        }
        if self.observed_direct_wait(self.progress.permission_wait.as_ref())
            || self.permission().is_some()
        {
            return Some(WaitKind::Permission);
        }
        if self
            .progress
            .script_waits
            .values()
            .any(|(turn, call)| self.foreground_script_call(*turn, call))
            || approvals.iter().any(|approval| {
                approval.job_id.is_none()
                    && approval.request.id.starts_with("script:")
                    && self.snapshot.as_ref().is_some_and(|snapshot| {
                        approval.request.conversation.as_str() == snapshot.agent.address.as_str()
                    })
                    && !self
                        .progress
                        .settled_script_waits
                        .contains(&approval.request.id)
                    && self.foreground_script_call(
                        approval.request.turn,
                        &approval.request.tool_call_id,
                    )
            })
        {
            return Some(WaitKind::Permission);
        }
        // ask_user records a pending request inside the current tool batch;
        // actual suspension is at the batch boundary. While a sibling is
        // still executing, keep its tool activity visible. The request card
        // remains available for answering early.
        if self.progress.tools.is_empty()
            && (self.observed_direct_wait(self.progress.question_wait.as_ref())
                || self.question().is_some())
        {
            return Some(WaitKind::Question);
        }
        None
    }

    pub(crate) fn model_failed(&self) -> bool {
        self.progress.model_failed
            || self.snapshot.as_ref().is_some_and(|snapshot| {
                snapshot.agent.last_error.is_some()
                    && self.execution.is_none_or(|id| id <= snapshot.cursor)
                    && matches!(
                        snapshot.agent.status,
                        AgentStatus::Interrupted | AgentStatus::Idle
                    )
                    && (self.progress.is_stopped_retry()
                        || self.transcript.turn_model_failed(snapshot.agent.turn))
            })
    }

    pub(crate) fn parallel_tools(&self) -> usize {
        self.progress.tools.len()
    }

    pub(crate) fn composer_output_tokens(&self, activity: Activity) -> Option<OutputTokens> {
        match activity {
            Activity::Finished => self
                .current_request_usage()
                .and_then(|usage| usage.latest_request.as_ref())
                .and_then(|sample| sample.output_tokens)
                .map(OutputTokens::Reported)
                .or_else(|| self.progress.output_meter.tokens()),
            Activity::Streaming
            | Activity::Thinking
            | Activity::RunningTools
            | Activity::ToolArguments => self.progress.output_meter.tokens(),
            _ => None,
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
