use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::event::{EventPayload, StoredEvent};
use crate::execution::OutputReference;
use crate::{Error, Result};

use super::observe::ReplyPreview;
use super::retention::OutputStore;
use super::types::{
    AgentConfig, AgentSnapshot, AgentStatus, AgentView, Input, InputStatus, PendingInput,
    QuestionRequest, Receipt, UsageTotals,
};
use super::usage::AgentUsage;
use super::values::{NamedBinding, ValueState};
use super::TurnIndex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Command {
    pub(super) command_id: String,
    pub(super) fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct CreationClaim {
    pub(super) command: Command,
    pub(super) created_at: String,
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
    OutputOwned {
        agent: String,
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn: Option<u64>,
    },
    NamedValueSaved {
        agent: String,
        binding: NamedBinding,
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
    },
    ExecutionStarted {
        address: String,
    },
    StepConfigured {
        address: String,
        config: AgentConfig,
    },
    InputsAccepted {
        address: String,
        input_ids: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn: Option<u64>,
    },
    TurnSettled {
        address: String,
        status: AgentStatus,
        output: Option<String>,
        error: Option<String>,
        turn: Option<u64>,
        usage: UsageTotals,
    },
    AgentStopped {
        address: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<Command>,
        #[serde(default)]
        cleanup_pending: bool,
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
    CleanupFailed {
        address: String,
        message: String,
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

#[derive(Debug, Clone)]
pub(super) struct QueuedDelivery {
    pub(super) queued_at: u64,
    pub(super) receipt: Receipt,
    pub(super) input: Input,
    pub(super) values: Vec<OutputReference>,
}

#[derive(Debug)]
pub(super) struct AgentState {
    pub(super) snapshot: AgentSnapshot,
    pub(super) inbox: VecDeque<QueuedDelivery>,
    pub(super) accepted: Vec<QueuedDelivery>,
    question_replies: HashSet<String>,
    pub(super) reply_preview: Option<ReplyPreview>,
    pub(super) permission_reply: Option<(String, bool)>,
    pub(super) active_epoch: Option<u64>,
    pub(super) stopped_at: Option<u64>,
    pub(super) model_usage: AgentUsage,
}

impl AgentState {
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

    fn set_active_config(&mut self, config: Option<AgentConfig>) {
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

#[derive(Debug, Default)]
pub(super) struct State {
    pub(super) agents: BTreeMap<String, AgentState>,
    pub(super) turns: TurnIndex,
    pub(super) commands: HashMap<String, (String, Receipt)>,
    pub(super) deliveries: HashMap<String, InputStatus>,
    pub(super) withdrawals: HashMap<String, InputStatus>,
    pub(super) creation: Option<CreationClaim>,
    pub(super) closing: bool,
    pub(super) closed: bool,
    pub(super) closed_cursor: u64,
    pub(super) cleanup_failures: BTreeMap<String, String>,
    pub(super) outputs: OutputStore,
    pub(super) values: ValueState,
}

impl State {
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
                            config,
                            active_config: None,
                            status: AgentStatus::Idle,
                            pending_inputs: 0,
                            admitted_task: None,
                            queued_task: None,
                            turn: None,
                            turn_started_at_ms: None,
                            turn_elapsed_ms: None,
                            execution_id: None,
                            last_output: None,
                            last_error: None,
                            permission: None,
                            question: None,
                            usage: UsageTotals::default(),
                        },
                        inbox: VecDeque::new(),
                        accepted: Vec::new(),
                        question_replies: HashSet::new(),
                        reply_preview: None,
                        permission_reply: None,
                        active_epoch: None,
                        stopped_at: None,
                        model_usage: AgentUsage::default(),
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
            Record::OutputOwned { agent, id, turn } => {
                self.outputs.claim(&agent, &id);
                if let Some(turn) = turn {
                    self.values.pin(agent, turn, vec![id]);
                }
            }
            Record::NamedValueSaved { agent, binding } => {
                self.outputs.claim(&agent, &binding.value.reference.id);
                self.values.save(agent, binding);
            }
            Record::OutputsPinned { agent, turn, ids } => {
                self.values.pin(agent, turn, ids);
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
                agent.update_inbox();
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
            }
            Record::StepConfigured { address, config } => {
                self.agent_mut(&address)?.set_active_config(Some(config));
            }
            Record::InputsAccepted {
                address,
                input_ids,
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
                let agent = self.agent_mut(&address)?;
                for id in input_ids {
                    let index = agent
                        .inbox
                        .iter()
                        .position(|entry| entry.input.command_id == id)
                        .ok_or_else(|| {
                            Error::InvalidEventStream(format!(
                                "agent {address}: accepted input {id} was not pending"
                            ))
                        })?;
                    let input = agent.inbox.remove(index).expect("input position exists");
                    if !agent.question_replies.remove(&id) {
                        if let Some(preview) = task_preview(&input.input.text) {
                            agent.snapshot.admitted_task = Some(preview);
                        }
                    }
                    agent.accepted.push(input);
                }
                agent.update_inbox();
                agent.snapshot.status = AgentStatus::Running;
                agent.snapshot.turn = Some(admitted_turn);
                agent.snapshot.execution_id.get_or_insert(event_id);
                if agent.snapshot.active_config.is_none() {
                    agent.snapshot.active_config = Some(agent.snapshot.config.clone());
                }
                agent.snapshot.last_error = None;
            }
            Record::TurnSettled {
                address,
                status,
                output,
                error,
                turn,
                usage,
            } => {
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
                if turn.is_some() {
                    agent.snapshot.turn = turn;
                }
                agent
                    .model_usage
                    .settle(agent.snapshot.turn, &mut agent.snapshot.usage, usage)?;
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
                agent.stopped_at = Some(event_id);
            }
            Record::AgentResumed {
                address,
                command,
                restore,
            } => {
                if let Some(command) = command {
                    self.remember_command(command, event_id, address.clone());
                }
                let agent = self.agent_mut(&address)?;
                if restore {
                    agent.snapshot.status = AgentStatus::Idle;
                    agent.snapshot.execution_id = None;
                    agent.stopped_at = None;
                    if agent.permission_reply.is_none() {
                        agent.set_active_config(None);
                        agent.inbox.extend(agent.accepted.drain(..));
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
            Record::SessionClosed => {}
            Record::CleanupFailed { address, message } => {
                self.agent_mut(&address)?.snapshot.last_error = Some(message.clone());
                self.cleanup_failures.insert(address, message);
            }
            Record::QuestionRequested { address, request } => {
                self.agent_mut(&address)?.snapshot.question = Some(request);
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

    pub(super) fn view(&self, address: &str, cursor: u64) -> Result<AgentView> {
        Ok(AgentView {
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
        snapshot
    }

    pub(super) fn mark_interrupted(&mut self) {
        for agent in self.agents.values_mut() {
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
                snapshot.last_error = Some("execution interrupted before a durable settlement; explicitly advance after reconciling side effects".into());
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
        self.values.grant(&input.target, &values);
        let agent = self.agent_mut(&input.target)?;
        agent.inbox.push_back(QueuedDelivery {
            queued_at: event_id,
            receipt,
            input,
            values,
        });
        agent.update_inbox();
        Ok(())
    }

    fn remember_command(&mut self, command: Command, event_id: u64, address: String) {
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

pub(super) fn validate_key(key: &str) -> Result<()> {
    if key.is_empty() || key.len() > 128 {
        return Err(Error::InvalidArgument(
            "command identity must contain 1..=128 bytes".into(),
        ));
    }
    Ok(())
}

pub(super) fn validate_address(address: &str) -> Result<()> {
    if address.is_empty()
        || address.len() > 64
        || !address
            .bytes()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_'))
    {
        return Err(Error::InvalidArgument(format!("agent address '{address}' must be flat and contain 1..=64 lowercase ASCII letters, digits, '-' or '_'")));
    }
    Ok(())
}
