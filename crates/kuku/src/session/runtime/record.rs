use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::event::{EventPayload, StoredEvent};
use crate::execution::{ModelImage, OutputReference};
use crate::{Error, Result};

use super::TurnIndex;
use super::jobs::{JOB_DELIVERY_PREFIX, JobCompletion, JobIndex};
use super::media::RetainedModelImage;
use super::observe::ReplyPreview;
use super::retention::{CaptureOrigin, OutputStore, output_id};
use super::types::{
    AgentConfig, AgentSnapshot, AgentStatus, AgentView, Input, InputSource, InputStatus,
    PendingInput, QuestionRequest, Receipt, UsageTotals,
};
use super::usage::{AgentUsage, SummaryRequest};
use super::values::{NamedBinding, ValueState};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Command {
    pub(super) command_id: String,
    pub(super) fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct CreationClaim {
    pub(super) command: Command,
    pub(super) created_at: String,
    pub(super) default_model: Option<crate::config::ModelRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) prompts_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct InterruptedExecution {
    pub(super) execution_id: u64,
    pub(super) turn: Option<u64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(super) text: String,
    #[serde(default)]
    pub(super) truncated: bool,
    pub(super) usage: UsageTotals,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub(super) enum Record {
    SessionClaimed {
        claim: CreationClaim,
    },
    AgentCreated {
        command_id: String,
        fingerprint: String,
        address: String,
        config: AgentConfig,
    },
    AgentConfigured {
        command_id: String,
        fingerprint: String,
        address: String,
        config: AgentConfig,
    },
    InputDelivered {
        fingerprint: String,
        input: Input,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        config: Option<AgentConfig>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        values: Vec<OutputReference>,
    },
    OutputReserved {
        agent: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<CaptureOrigin>,
    },
    NamedValueSaved {
        agent: String,
        binding: NamedBinding,
    },
    ModelImageSaved {
        agent: String,
        turn: Option<u64>,
        digest: String,
        image: ModelImage,
    },
    InputImageRestaged {
        id: String,
        at: std::time::SystemTime,
    },
    OutputsPinned {
        agent: String,
        turn: u64,
        ids: Vec<String>,
    },
    InputWithdrawal {
        command: Command,
        address: String,
        input_command_id: String,
        status: InputStatus,
    },
    InputReplacement {
        command: Command,
        address: String,
        input_command_id: String,
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<crate::execution::InputImage>,
    },
    MessageRewind {
        command: Command,
        address: String,
        target_event_id: u64,
        target_turn: u64,
        restore_files: bool,
        #[serde(default)]
        removed_input_ids: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        abandoned_turn: Option<u64>,
        #[serde(default)]
        warnings: Vec<String>,
    },
    HistoryJump {
        command: Command,
        address: String,
        target_event_id: u64,
        target_turn: u64,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        restore_files: bool,
    },
    ExecutionStarted {
        address: String,
    },
    ExecutionDeferred {
        address: String,
    },
    CompactionRequested {
        command: Command,
        address: String,
        turn: u64,
    },
    CompactionSettled {
        address: String,
        execution: u64,
        status: AgentStatus,
        cancelled: bool,
        error: Option<String>,
    },
    CompactionReverted {
        command: Command,
        address: String,
        segment: u64,
    },
    StepConfigured {
        address: String,
        config: AgentConfig,
    },
    InputsAccepted {
        address: String,
        input_ids: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        message_input_ids: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn: Option<u64>,
    },
    TurnSettled {
        address: String,
        status: AgentStatus,
        output: Option<String>,
        error: Option<String>,
        #[serde(default)]
        resume_blocked: bool,
        turn: Option<u64>,
        usage: UsageTotals,
    },
    AgentStopped {
        address: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<Command>,
        #[serde(default)]
        cleanup_pending: bool,
        #[serde(default)]
        continue_queued: bool,
        #[serde(default)]
        discard_cancelled_turn: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        interruption: Option<InterruptedExecution>,
    },
    AgentResumed {
        address: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<Command>,
        #[serde(default = "restore_on_resume")]
        restore: bool,
    },
    PermissionWaiting {
        address: String,
        request: crate::query::PermissionRequest,
    },
    PermissionReplied {
        command_id: String,
        fingerprint: String,
        address: String,
        request_id: String,
        allow: bool,
    },
    ScriptPermissionWaiting {
        address: String,
        request: crate::query::PermissionRequest,
    },
    ScriptPermissionReplied {
        address: String,
        command_id: String,
        fingerprint: String,
        request_id: String,
        allow: bool,
    },
    ScriptPermissionEnded {
        address: String,
        turn: u64,
        request_id: String,
        reason: String,
    },
    ScriptRecoveryCompleted {
        address: String,
        turn: u64,
        tool_call_id: String,
    },
    CleanupFailed {
        address: String,
        message: String,
    },
    JobCleanupReviewed {
        command: Command,
        address: String,
        job_id: String,
    },
    JobFinished {
        job_id: String,
        agent: String,
        turn: u64,
        result: super::JobResult,
        status: super::JobStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        elapsed_ms: Option<u64>,
        delivery: Box<Input>,
    },
    QuestionRequested {
        address: String,
        request: QuestionRequest,
    },
    QuestionReplied {
        fingerprint: String,
        request_id: String,
        input: Input,
    },
    SessionClosed,
}

impl Record {
    pub(super) fn payload(&self) -> Result<EventPayload> {
        Ok(EventPayload::Unknown(serde_json::json!({
            "kind": "runtime.operation",
            "ts": timestamp()?,
            "record": self,
        })))
    }

    pub(super) fn from_event(event: &StoredEvent) -> Result<Option<Self>> {
        let EventPayload::Unknown(value) = &event.payload else {
            return Ok(None);
        };
        if value["kind"] != "runtime.operation" {
            return Ok(None);
        }
        Ok(Some(serde_json::from_value(value["record"].clone())?))
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct QueuedDelivery {
    pub(super) queued_at: u64,
    pub(super) receipt: Receipt,
    pub(super) input: Input,
    pub(super) values: Vec<OutputReference>,
    pub(super) accepted_turn: Option<u64>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct ManualCompaction {
    pub(super) execution: u64,
    pub(super) previous_status: AgentStatus,
    pub(super) finished: bool,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct AgentState {
    pub(super) snapshot: AgentSnapshot,
    pub(super) inbox: VecDeque<QueuedDelivery>,
    pub(super) accepted: Vec<QueuedDelivery>,
    pub(super) last_question_turn: Option<u64>,
    question_replies: HashSet<String>,
    #[serde(skip)]
    pub(super) reply_preview: Option<ReplyPreview>,
    pub(super) permission_reply: Option<(String, bool)>,
    #[serde(skip)]
    pub(super) active_epoch: Option<u64>,
    pub(super) stopped_at: Option<u64>,
    #[serde(default)]
    pub(super) continue_queued: bool,
    // Required in a checkpoint: older projections may already have lost an
    // interrupt disposition when a later Stop revoked the queue wake.
    discard_cancelled_turn: bool,
    pub(super) model_usage: AgentUsage,
    pub(super) manual_compaction: Option<ManualCompaction>,
    pub(super) summary_request: Option<SummaryRequest>,
}

impl AgentState {
    pub(super) fn discard_on_resume(&self) -> bool {
        self.stopped_at.is_some() && self.discard_cancelled_turn
    }

    pub(super) fn can_admit(&self, input: &Input) -> bool {
        self.snapshot.job_needing_review().is_none()
            || input.command_id.starts_with(JOB_DELIVERY_PREFIX) && input.source.is_job_delivery()
    }

    pub(super) fn submitted_inputs(&self) -> usize {
        self.inbox
            .iter()
            .filter(|entry| !entry.input.command_id.starts_with(JOB_DELIVERY_PREFIX))
            .count()
    }

    pub(super) fn pending_usage(&self) -> UsageTotals {
        if self.accepted.is_empty() {
            UsageTotals::default()
        } else {
            self.model_usage.turn_totals(self.snapshot.turn)
        }
    }

    fn set_config(&mut self, config: AgentConfig) {
        if self.snapshot.active_config.is_none() && self.snapshot.config != config {
            self.snapshot.usage.latest_request = None;
        }
        self.snapshot.config = config;
    }

    pub(super) fn set_active_config(&mut self, config: Option<AgentConfig>) {
        let requested = &self.snapshot.config;
        if self.snapshot.active_config.as_ref().unwrap_or(requested)
            != config.as_ref().unwrap_or(requested)
        {
            self.snapshot.usage.latest_request = None;
        }
        self.snapshot.active_config = config;
    }

    fn update_inbox(&mut self) {
        self.snapshot.pending_inputs = self.inbox.len();
        self.snapshot.queued_task = self
            .inbox
            .iter()
            .rev()
            .filter(|entry| !self.question_replies.contains(&entry.input.command_id))
            .filter(|entry| !entry.input.source.is_job_delivery())
            .find_map(|entry| task_preview(&entry.input.text));
    }

    pub(super) fn queued_message_position(&self, command_id: &str) -> Option<usize> {
        if self.question_replies.contains(command_id) {
            return None;
        }
        self.inbox
            .iter()
            .position(|entry| entry.input.command_id == command_id)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct DelegatedDelivery {
    pub(super) sender: String,
    pub(super) command_id: String,
    pub(super) turn: Option<u64>,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub(super) struct State {
    pub(super) agents: BTreeMap<String, AgentState>,
    pub(super) turns: TurnIndex,
    pub(super) commands: HashMap<String, (String, Receipt)>,
    pub(super) message_rewind_warnings: HashMap<String, Vec<String>>,
    pub(super) deliveries: HashMap<String, InputStatus>,
    // Missing in older recovery checkpoints. Rebuild from the journal in that case.
    pub(super) delegated_deliveries: HashMap<u64, DelegatedDelivery>,
    pub(super) withdrawals: HashMap<String, InputStatus>,
    pub(super) creation: Option<CreationClaim>,
    pub(super) default_model: Option<crate::config::ModelRef>,
    pub(super) closing: bool,
    pub(super) closed: bool,
    pub(super) closed_cursor: u64,
    pub(super) cleanup_failures: BTreeMap<String, String>,
    pub(super) outputs: OutputStore,
    pub(super) values: ValueState,
    pub(super) model_images: HashMap<String, RetainedModelImage>,
    pub(super) model_image_ids: HashMap<String, String>,
    pub(super) jobs: JobIndex,
    #[serde(default)]
    pub(super) script_permissions: BTreeMap<String, crate::query::PermissionRequest>,
    #[serde(default)]
    pub(super) recoverable_scripts: BTreeSet<(u64, String, String)>,
}

impl State {
    pub(super) fn acknowledge_job_result(&mut self, agent: &str, job_id: &str) -> Result<()> {
        let command_id = format!("{JOB_DELIVERY_PREFIX}{job_id}");
        let agent = self.agent_mut(agent)?;
        if let Some(position) = agent
            .inbox
            .iter()
            .position(|entry| entry.input.command_id == command_id)
        {
            agent.inbox.remove(position);
            agent.update_inbox();
            self.deliveries.insert(command_id, InputStatus::Accepted);
        }
        Ok(())
    }

    pub(super) fn reduce(&mut self, event_id: u64, record: Record) -> Result<()> {
        match record {
            Record::SessionClaimed { claim } => self.creation = Some(claim),
            Record::AgentCreated {
                command_id,
                fingerprint,
                address,
                config,
            } => {
                self.remember(command_id, fingerprint, event_id, address.clone());
                self.agents.insert(
                    address.clone(),
                    AgentState {
                        snapshot: AgentSnapshot {
                            address,
                            creation_event_id: event_id,
                            config,
                            active_config: None,
                            status: AgentStatus::Idle,
                            pending_inputs: 0,
                            running_jobs: 0,
                            admitted_task: None,
                            queued_task: None,
                            turn: None,
                            turn_started_at_ms: None,
                            turn_elapsed_ms: None,
                            cache_warning: None,
                            last_model_route: None,
                            execution_id: None,
                            last_output: None,
                            last_error: None,
                            resume_blocked: false,
                            job_cleanup_reviews: Vec::new(),
                            permission: None,
                            question: None,
                            usage: UsageTotals::default(),
                        },
                        inbox: VecDeque::new(),
                        accepted: Vec::new(),
                        last_question_turn: None,
                        question_replies: HashSet::new(),
                        reply_preview: None,
                        permission_reply: None,
                        active_epoch: None,
                        stopped_at: None,
                        continue_queued: false,
                        discard_cancelled_turn: false,
                        model_usage: AgentUsage::default(),
                        manual_compaction: None,
                        summary_request: None,
                    },
                );
            }
            Record::AgentConfigured {
                command_id,
                fingerprint,
                address,
                config,
            } => {
                self.remember(command_id, fingerprint, event_id, address.clone());
                self.agent_mut(&address)?.set_config(config);
            }
            Record::InputDelivered {
                fingerprint,
                input,
                config,
                values,
            } => {
                if let Some(config) = config {
                    self.agent_mut(&input.target)?.set_config(config);
                }
                self.deliver(event_id, fingerprint, input, values)?;
            }
            Record::OutputReserved { agent, origin } => {
                let id = output_id(event_id);
                let turn = origin.as_ref().map(|origin| origin.turn);
                self.outputs.claim_capture(&agent, &id, origin);
                if let Some(turn) = turn {
                    self.pin_current_turn(agent, turn, vec![id])?;
                }
            }
            Record::NamedValueSaved { agent, binding } => {
                self.values.save(agent, binding);
            }
            Record::ModelImageSaved {
                agent,
                turn,
                digest,
                image,
            } => {
                let id = image.output.id.clone();
                self.model_image_ids.insert(digest.clone(), id.clone());
                self.model_images.insert(
                    id,
                    RetainedModelImage {
                        agent,
                        turn,
                        digest,
                        image,
                        referenced: false,
                        staged: turn.is_none(),
                    },
                );
            }
            Record::InputImageRestaged { id, at } => {
                if let Some(image) = self.model_images.get_mut(&id) {
                    image.staged = true;
                    self.outputs.refresh_retention(&id, at);
                }
            }
            Record::OutputsPinned { agent, turn, ids } => {
                self.pin_current_turn(agent, turn, ids)?;
            }
            Record::InputWithdrawal {
                command,
                address,
                input_command_id,
                status,
            } => {
                self.withdrawals.insert(command.command_id.clone(), status);
                self.remember_command(command, event_id, address.clone());
                if status == InputStatus::Withdrawn {
                    self.deliveries.insert(input_command_id.clone(), status);
                    let agent = self.agent_mut(&address)?;
                    agent
                        .inbox
                        .retain(|entry| entry.input.command_id != input_command_id);
                    agent.question_replies.remove(&input_command_id);
                    agent.update_inbox();
                }
            }
            Record::InputReplacement {
                command,
                address,
                input_command_id,
                text,
                images,
            } => {
                let position = self
                    .agent(&address)?
                    .queued_message_position(&input_command_id)
                    .ok_or_else(|| {
                        Error::InvalidEventStream(format!(
                            "agent {address}: replaced input {input_command_id} was not queued"
                        ))
                    })?;
                let command_id = command.command_id.clone();
                self.remember_command(command, event_id, address.clone());
                self.deliveries
                    .insert(input_command_id, InputStatus::Withdrawn);
                self.deliveries
                    .insert(command_id.clone(), InputStatus::Queued);
                let receipt = self.commands[&command_id].1.clone();
                let agent = self.agent_mut(&address)?;
                let entry = &mut agent.inbox[position];
                entry.receipt = receipt;
                entry.input.command_id = command_id;
                entry.input.text = text;
                entry.input.images = images;
                agent.update_inbox();
            }
            Record::MessageRewind {
                command,
                address,
                removed_input_ids,
                abandoned_turn,
                warnings,
                ..
            } => {
                let removed = {
                    let agent = self.agent_mut(&address)?;
                    agent.snapshot.last_output = None;
                    if abandoned_turn.is_some() {
                        agent.snapshot.status = AgentStatus::Idle;
                        agent.snapshot.execution_id = None;
                        agent.snapshot.last_error = None;
                        agent.snapshot.resume_blocked = false;
                        agent.reply_preview = None;
                        agent.set_active_config(None);
                        agent.model_usage.settle(abandoned_turn);
                    }
                    let ids: HashSet<_> = removed_input_ids.into_iter().collect();
                    if ids.is_empty() {
                        Vec::new()
                    } else {
                        let mut removed = Vec::new();
                        agent.accepted.retain(|delivery| {
                            if ids.contains(&delivery.input.command_id) {
                                removed.push(delivery.input.command_id.clone());
                                false
                            } else {
                                true
                            }
                        });
                        agent.inbox.retain(|delivery| {
                            if ids.contains(&delivery.input.command_id) {
                                removed.push(delivery.input.command_id.clone());
                                false
                            } else {
                                true
                            }
                        });
                        for id in &removed {
                            agent.question_replies.remove(id);
                        }
                        if !removed.is_empty() {
                            agent.snapshot.admitted_task = None;
                        }
                        agent.update_inbox();
                        removed
                    }
                };
                self.values.release_turn(&address, abandoned_turn);
                for id in removed {
                    self.deliveries.insert(id, InputStatus::Withdrawn);
                }
                self.message_rewind_warnings
                    .insert(command.command_id.clone(), warnings);
                self.remember_command(command, event_id, address);
            }
            Record::HistoryJump {
                command,
                address,
                target_turn,
                ..
            } => {
                let snapshot = &mut self.agent_mut(&address)?.snapshot;
                snapshot.turn = Some(target_turn);
                snapshot.last_output = None;
                snapshot.last_error = None;
                snapshot.resume_blocked = false;
                snapshot.admitted_task = None;
                self.remember_command(command, event_id, address);
            }
            Record::ExecutionStarted { address } => {
                let agent = self.agent_mut(&address)?;
                agent.reply_preview = None;
                if agent.permission_reply.is_none() || agent.snapshot.active_config.is_none() {
                    agent.set_active_config(Some(agent.snapshot.config.clone()));
                }
                let snapshot = &mut agent.snapshot;
                snapshot.execution_id = Some(event_id);
                snapshot.status = AgentStatus::Running;
                snapshot.last_error = None;
                snapshot.resume_blocked = false;
            }
            Record::ExecutionDeferred { address } => {
                let agent = self.agent_mut(&address)?;
                agent.snapshot.status = AgentStatus::Idle;
                agent.snapshot.execution_id = None;
                if agent.permission_reply.is_none() {
                    agent.set_active_config(None);
                }
            }
            Record::CompactionRequested {
                command, address, ..
            } => {
                self.compaction_requested(event_id, command, address)?;
            }
            Record::CompactionSettled {
                address,
                status,
                error,
                ..
            } => {
                self.compaction_settled(&address, status, error)?;
            }
            Record::CompactionReverted {
                command, address, ..
            } => {
                self.agent_mut(&address)?.snapshot.usage.latest_request = None;
                self.remember_command(command, event_id, address);
            }
            Record::StepConfigured { address, config } => {
                self.agent_mut(&address)?.set_active_config(Some(config));
            }
            Record::InputsAccepted {
                address,
                input_ids,
                message_input_ids,
                turn,
            } => {
                let admitted_turn = turn.unwrap_or(event_id);
                let previous_turn = self.agent(&address)?.snapshot.turn;
                if previous_turn != Some(admitted_turn) {
                    self.values.release_turn(&address, previous_turn);
                }
                for id in &input_ids {
                    self.deliveries.insert(id.clone(), InputStatus::Accepted);
                }
                let mut turn_preview = None;
                for id in input_ids {
                    let agent = self.agent_mut(&address)?;
                    let index = agent
                        .inbox
                        .iter()
                        .position(|entry| entry.input.command_id == id)
                        .ok_or_else(|| {
                            Error::InvalidEventStream(format!(
                                "agent {address}: accepted input {id} was not pending"
                            ))
                        })?;
                    let mut input = agent.inbox.remove(index).expect("input position exists");
                    if message_input_ids.contains(&id) {
                        input.accepted_turn = Some(admitted_turn);
                    } else {
                        input.accepted_turn.get_or_insert(admitted_turn);
                    }
                    let question_reply = agent.question_replies.remove(&id);
                    if !question_reply && !input.input.source.is_job_delivery() {
                        if let Some(preview) = task_preview(&input.input.text) {
                            if turn_preview.is_none() {
                                turn_preview = Some(preview.clone());
                            }
                            agent.snapshot.admitted_task = Some(preview);
                        }
                    }
                    let receipt_id = input.receipt.event_id;
                    let image_keys: Vec<_> = input
                        .input
                        .images
                        .iter()
                        .map(|image| image.key(&address))
                        .collect();
                    agent.accepted.push(input);
                    for key in image_keys {
                        if let Some(id) = self.model_image_ids.get(&key)
                            && let Some(retained) = self.model_images.get_mut(id)
                        {
                            retained.referenced = true;
                        }
                    }
                    if let Some(delivery) = self.delegated_deliveries.get_mut(&receipt_id) {
                        delivery.turn.get_or_insert(admitted_turn);
                    }
                }
                let agent = self.agent_mut(&address)?;
                agent.update_inbox();
                agent.snapshot.status = AgentStatus::Running;
                agent.snapshot.turn = Some(admitted_turn);
                agent.snapshot.execution_id.get_or_insert(event_id);
                if agent.snapshot.active_config.is_none() {
                    agent.snapshot.active_config = Some(agent.snapshot.config.clone());
                }
                agent.snapshot.last_error = None;
                if let Some(preview) = turn_preview {
                    self.turns.preview(&address, admitted_turn, &preview);
                }
            }
            Record::TurnSettled {
                address,
                status,
                output,
                error,
                resume_blocked,
                turn,
                usage: _,
            } => {
                if matches!(status, AgentStatus::Idle | AgentStatus::Waiting) {
                    self.recoverable_scripts.retain(|(script_turn, owner, _)| {
                        owner != &address || Some(*script_turn) != turn
                    });
                }
                if !matches!(status, AgentStatus::Waiting | AgentStatus::Interrupted) {
                    self.values
                        .release_turn(&address, turn.or(self.agent(&address)?.snapshot.turn));
                }
                let agent = self.agent_mut(&address)?;
                agent.reply_preview = None;
                agent.snapshot.status = status;
                if status != AgentStatus::Waiting {
                    agent.snapshot.execution_id = None;
                    agent.set_active_config(None);
                }
                if status == AgentStatus::Stopped {
                    agent.stopped_at.get_or_insert(event_id);
                }
                if output.is_some() {
                    agent.snapshot.last_output = output;
                }
                agent.snapshot.last_error = error;
                agent.snapshot.resume_blocked = resume_blocked;
                if turn.is_some() {
                    agent.snapshot.turn = turn;
                }
                agent.model_usage.settle(agent.snapshot.turn);
                if status != AgentStatus::Interrupted {
                    agent.accepted.clear();
                }
                agent.snapshot.permission = None;
                agent.permission_reply = None;
            }
            Record::AgentStopped {
                address,
                command,
                cleanup_pending,
                continue_queued,
                discard_cancelled_turn,
                ..
            } => {
                if let Some(command) = command {
                    self.remember_command(command, event_id, address.clone());
                }
                if !cleanup_pending {
                    self.values
                        .release_turn(&address, self.agent(&address)?.snapshot.turn);
                }
                let agent = self.agent_mut(&address)?;
                agent.reply_preview = None;
                agent.snapshot.status = AgentStatus::Stopped;
                if !cleanup_pending {
                    agent.snapshot.execution_id = None;
                    agent.set_active_config(None);
                    agent.accepted.clear();
                }
                agent.snapshot.question = None;
                agent.snapshot.permission = None;
                agent.permission_reply = None;
                // A later Stop can revoke the queue wake without undoing the
                // interruption's cancellation of the accepted Turn. The old
                // continue_queued field also identifies legacy interruptions.
                agent.discard_cancelled_turn =
                    agent.discard_on_resume() || discard_cancelled_turn || continue_queued;
                agent.stopped_at = Some(event_id);
                agent.continue_queued = continue_queued;
            }
            Record::AgentResumed {
                address,
                command,
                restore,
            } => {
                if let Some(command) = command {
                    self.remember_command(command, event_id, address.clone());
                }
                // If the service stopped before the interruption could settle, keep
                // its accepted turn out of the next execution on explicit recovery.
                let discarded_turn = restore && self.agent(&address)?.discard_on_resume();
                if discarded_turn {
                    self.values
                        .release_turn(&address, self.agent(&address)?.snapshot.turn);
                }
                let agent = self.agent_mut(&address)?;
                if restore {
                    agent.snapshot.status = AgentStatus::Idle;
                    agent.snapshot.execution_id = None;
                    agent.snapshot.resume_blocked = false;
                    agent.stopped_at = None;
                    agent.continue_queued = false;
                    agent.discard_cancelled_turn = false;
                    if agent.permission_reply.is_none() {
                        agent.set_active_config(None);
                        if discarded_turn {
                            agent.accepted.clear();
                        } else {
                            agent.inbox.extend(agent.accepted.drain(..));
                        }
                        agent
                            .inbox
                            .make_contiguous()
                            .sort_by_key(|delivery| delivery.queued_at);
                        agent.snapshot.pending_inputs = agent.inbox.len();
                    }
                }
            }
            Record::PermissionWaiting { address, request } => {
                let agent = self.agent_mut(&address)?;
                agent.reply_preview = None;
                agent.snapshot.status = AgentStatus::Waiting;
                agent.snapshot.permission = Some(request);
                agent.permission_reply = None;
            }
            Record::PermissionReplied {
                command_id,
                fingerprint,
                address,
                request_id,
                allow,
            } => {
                self.remember(command_id, fingerprint, event_id, address.clone());
                let agent = self.agent_mut(&address)?;
                agent.permission_reply = Some((request_id, allow));
                agent.snapshot.status = AgentStatus::Idle;
            }
            Record::ScriptPermissionWaiting {
                address, request, ..
            } => {
                self.recoverable_scripts.insert((
                    request.turn,
                    address,
                    request.tool_call_id.clone(),
                ));
                self.script_permissions.insert(request.id.clone(), request);
            }
            Record::ScriptPermissionReplied {
                address,
                command_id,
                fingerprint,
                request_id,
                ..
            } => {
                self.remember(command_id, fingerprint, event_id, address);
                self.script_permissions.remove(&request_id);
            }
            Record::ScriptPermissionEnded { request_id, .. } => {
                self.script_permissions.remove(&request_id);
            }
            Record::ScriptRecoveryCompleted {
                address,
                turn,
                tool_call_id,
            } => {
                self.recoverable_scripts
                    .remove(&(turn, address, tool_call_id));
            }
            Record::SessionClosed => {}
            Record::JobFinished {
                job_id,
                agent,
                turn: _,
                result,
                status,
                elapsed_ms,
                delivery,
            } => {
                self.jobs.finish(JobCompletion {
                    event_id,
                    agent: &agent,
                    job_id: &job_id,
                    result: &result,
                    status,
                    elapsed_ms,
                })?;
                if result.cleanup_incomplete {
                    let owner = self.agent_mut(&agent)?;
                    owner.snapshot.job_cleanup_reviews.push(job_id);
                }
                self.deliver(event_id, String::new(), *delivery, Vec::new())?;
            }
            Record::JobCleanupReviewed {
                command,
                address,
                job_id,
            } => {
                let owner = self.agent_mut(&address)?;
                if owner.snapshot.job_needing_review() != Some(job_id.as_str()) {
                    return Err(Error::InvalidEventStream(format!(
                        "invalid cleanup review for job '{job_id}'"
                    )));
                }
                owner.snapshot.job_cleanup_reviews.remove(0);
                self.remember_command(command, event_id, address);
            }
            Record::CleanupFailed { address, message } => {
                self.agent_mut(&address)?.snapshot.last_error = Some(message.clone());
                self.cleanup_failures.insert(address, message);
            }
            Record::QuestionRequested { address, request } => {
                let agent = self.agent_mut(&address)?;
                agent.last_question_turn = Some(request.turn);
                agent.snapshot.question = Some(request);
            }
            Record::QuestionReplied {
                fingerprint,
                request_id: _,
                input,
            } => {
                let agent = self.agent_mut(&input.target)?;
                agent.question_replies.insert(input.command_id.clone());
                agent.snapshot.question = None;
                if agent.snapshot.permission.is_some() {
                    agent.snapshot.status = AgentStatus::Waiting;
                } else if agent.snapshot.status != AgentStatus::Running {
                    agent.snapshot.status = AgentStatus::Idle;
                }
                self.deliver(event_id, fingerprint, input, Vec::new())?;
            }
        }
        Ok(())
    }

    pub(super) fn agent_mut(&mut self, address: &str) -> Result<&mut AgentState> {
        self.agents.get_mut(address).ok_or_else(|| {
            Error::InvalidArgument(format!("agent '{address}' does not exist in this session"))
        })
    }

    pub(super) fn agent(&self, address: &str) -> Result<&AgentState> {
        self.agents.get(address).ok_or_else(|| {
            Error::InvalidArgument(format!("agent '{address}' does not exist in this session"))
        })
    }

    pub(super) fn input_status(&self, address: &str, command_id: &str) -> Result<InputStatus> {
        self.commands
            .get(command_id)
            .filter(|(_, receipt)| receipt.agent == address)
            .and_then(|_| self.deliveries.get(command_id))
            .copied()
            .ok_or_else(|| {
                Error::InvalidArgument(format!("no input '{command_id}' for agent '{address}'"))
            })
    }

    pub(super) fn pending_inputs(&self, address: &str) -> Result<Vec<PendingInput>> {
        Ok(self
            .agent(address)?
            .inbox
            .iter()
            .filter(|entry| {
                self.deliveries.get(&entry.input.command_id) == Some(&InputStatus::Queued)
            })
            .map(|entry| PendingInput {
                receipt: entry.receipt.clone(),
                input: entry.input.clone(),
            })
            .collect())
    }

    pub(super) fn first_queued_text(&self) -> Option<&str> {
        self.agents.values().find_map(|agent| {
            agent.inbox.iter().find_map(|entry| {
                (self.deliveries.get(&entry.input.command_id) == Some(&InputStatus::Queued))
                    .then_some(entry.input.text.as_str())
            })
        })
    }

    pub(super) fn view(&self, address: &str, cursor: u64) -> Result<AgentView> {
        Ok(AgentView {
            session_default_model: self.default_model.clone(),
            snapshot: self.snapshot(address)?,
            agents: self.snapshots(),
            cursor,
            pending_inputs: self.pending_inputs(address)?,
        })
    }

    pub(super) fn snapshot(&self, address: &str) -> Result<AgentSnapshot> {
        Ok(self.timed_snapshot(&self.agent(address)?.snapshot))
    }

    pub(super) fn snapshots(&self) -> Vec<AgentSnapshot> {
        self.agents
            .values()
            .map(|agent| self.timed_snapshot(&agent.snapshot))
            .collect()
    }

    fn timed_snapshot(&self, current: &AgentSnapshot) -> AgentSnapshot {
        let mut snapshot = current.clone();
        self.turns.time_snapshot(&mut snapshot);
        snapshot.running_jobs = self.jobs.running_count(&snapshot.address);
        snapshot
    }

    pub(super) fn observe_script_dispatch(&mut self, payload: &crate::event::EventPayload) {
        if let crate::event::EventPayload::Unknown(data) = payload
            && data["kind"] == "runtime.tool_dispatched"
            && data["tool"] == "run_js"
            && let (Some(turn), Some(agent), Some(id)) = (
                data["turn"].as_u64(),
                data["agent"].as_str(),
                data["tool_call_id"].as_str(),
            )
        {
            self.recoverable_scripts
                .insert((turn, agent.into(), id.into()));
        }
    }

    pub(super) fn mark_interrupted(&mut self) {
        for (address, agent) in &mut self.agents {
            if agent.snapshot.status == AgentStatus::Running
                || agent.snapshot.status == AgentStatus::Stopped
                    && agent.snapshot.execution_id.is_some()
            {
                if agent.permission_reply.is_none() {
                    agent.set_active_config(None);
                }
                let snapshot = &mut agent.snapshot;
                snapshot.status = AgentStatus::Interrupted;
                snapshot.execution_id = None;
                let pending_script = self
                    .recoverable_scripts
                    .iter()
                    .any(|(_, owner, _)| owner == address);
                snapshot.last_error = Some(if pending_script {
                    super::script_permissions::RESTARTED_SCRIPT.into()
                } else {
                    "execution interrupted before a durable settlement; explicitly advance after reconciling side effects".into()
                });
            }
        }
    }

    pub(super) fn retry(&self, command_id: &str, fingerprint: &str) -> Result<Option<Receipt>> {
        match self.commands.get(command_id) {
            Some((original, receipt)) if original == fingerprint => Ok(Some(receipt.clone())),
            Some(_) => Err(Error::CommandConflict(command_id.into())),
            None => Ok(None),
        }
    }

    fn deliver(
        &mut self,
        event_id: u64,
        fingerprint: String,
        input: Input,
        values: Vec<OutputReference>,
    ) -> Result<()> {
        self.remember(
            input.command_id.clone(),
            fingerprint,
            event_id,
            input.target.clone(),
        );
        self.deliveries
            .insert(input.command_id.clone(), InputStatus::Queued);
        let receipt = self.commands[&input.command_id].1.clone();
        if let InputSource::Agent(sender) = &input.source {
            self.delegated_deliveries.insert(
                event_id,
                DelegatedDelivery {
                    sender: sender.clone(),
                    command_id: input.command_id.clone(),
                    turn: None,
                },
            );
        }
        for image in &input.images {
            if let Some(id) = self.model_image_ids.get(&image.key(&input.target))
                && let Some(retained) = self.model_images.get_mut(id)
            {
                retained.staged = false;
            }
        }
        self.values.grant(&input.target, &values);
        let agent = self.agent_mut(&input.target)?;
        agent.inbox.push_back(QueuedDelivery {
            queued_at: event_id,
            receipt,
            input,
            values,
            accepted_turn: None,
        });
        agent.update_inbox();
        Ok(())
    }

    pub(super) fn remember_command(&mut self, command: Command, event_id: u64, address: String) {
        self.remember(command.command_id, command.fingerprint, event_id, address);
    }

    fn remember(&mut self, command_id: String, fingerprint: String, event_id: u64, agent: String) {
        self.commands.insert(
            command_id.clone(),
            (
                fingerprint,
                Receipt {
                    command_id,
                    event_id,
                    agent,
                },
            ),
        );
    }
}

fn task_preview(text: &str) -> Option<String> {
    const TASK_PREVIEW_BYTES: usize = 240;
    let text = text.trim();
    (!text.is_empty())
        .then(|| text[..text.floor_char_boundary(text.len().min(TASK_PREVIEW_BYTES))].to_owned())
}

fn restore_on_resume() -> bool {
    true
}

pub(super) fn fingerprint(value: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}

pub(super) fn timestamp() -> Result<String> {
    Ok(time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339)?)
}
