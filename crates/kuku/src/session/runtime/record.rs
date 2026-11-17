use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::event::{EventPayload, StoredEvent};
use crate::{Error, Result};

use super::types::{
    AgentConfig, AgentSnapshot, AgentStatus, AgentView, Input, InputStatus, PendingInput,
    QuestionRequest, Receipt, UsageTotals,
};

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
    },
    InputWithdrawal {
        command: Command,
        address: String,
        input_command_id: String,
        status: InputStatus,
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
pub(super) struct AgentState {
    pub(super) snapshot: AgentSnapshot,
    pub(super) inbox: VecDeque<PendingInput>,
    pub(super) accepted: Vec<PendingInput>,
    pub(super) permission_reply: Option<(String, bool)>,
    pub(super) active_epoch: Option<u64>,
    pub(super) stopped_at: Option<u64>,
}

#[derive(Debug, Default)]
pub(super) struct State {
    pub(super) agents: BTreeMap<String, AgentState>,
    pub(super) commands: HashMap<String, (String, Receipt)>,
    pub(super) deliveries: HashMap<String, InputStatus>,
    pub(super) withdrawals: HashMap<String, InputStatus>,
    pub(super) creation: Option<CreationClaim>,
    pub(super) closing: bool,
    pub(super) closed: bool,
    pub(super) closed_cursor: u64,
    pub(super) cleanup_failures: BTreeMap<String, String>,
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
                            turn: None,
                            execution_id: None,
                            last_output: None,
                            last_error: None,
                            permission: None,
                            question: None,
                            usage: UsageTotals::default(),
                        },
                        inbox: VecDeque::new(),
                        accepted: Vec::new(),
                        permission_reply: None,
                        active_epoch: None,
                        stopped_at: None,
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
                self.agent_mut(&address)?.snapshot.config = config;
            }
            Record::InputDelivered {
                fingerprint,
                input,
                config,
            } => {
                if let Some(config) = config {
                    self.agent_mut(&input.target)?.snapshot.config = config;
                }
                self.deliver(event_id, fingerprint, input)?;
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
                    agent.snapshot.pending_inputs = agent.inbox.len();
                }
            }
            Record::ExecutionStarted { address } => {
                let agent = self.agent_mut(&address)?;
                let snapshot = &mut agent.snapshot;
                if agent.permission_reply.is_none() || snapshot.active_config.is_none() {
                    snapshot.active_config = Some(snapshot.config.clone());
                }
                snapshot.execution_id = Some(event_id);
                snapshot.status = AgentStatus::Running;
                snapshot.last_error = None;
            }
            Record::StepConfigured { address, config } => {
                self.agent_mut(&address)?.snapshot.active_config = Some(config);
            }
            Record::InputsAccepted {
                address,
                input_ids,
                turn,
            } => {
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
                    agent
                        .accepted
                        .push(agent.inbox.remove(index).expect("input position exists"));
                }
                agent.snapshot.pending_inputs = agent.inbox.len();
                agent.snapshot.status = AgentStatus::Running;
                agent.snapshot.turn = Some(turn.unwrap_or(event_id));
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
                let agent = self.agent_mut(&address)?;
                agent.snapshot.status = status;
                if status != AgentStatus::Waiting {
                    agent.snapshot.execution_id = None;
                    agent.snapshot.active_config = None;
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
                add_usage(&mut agent.snapshot.usage.input_tokens, usage.input_tokens)?;
                add_usage(&mut agent.snapshot.usage.output_tokens, usage.output_tokens)?;
                add_usage(
                    &mut agent.snapshot.usage.cache_read_input_tokens,
                    usage.cache_read_input_tokens,
                )?;
                add_usage(
                    &mut agent.snapshot.usage.cache_creation_input_tokens,
                    usage.cache_creation_input_tokens,
                )?;
                agent.snapshot.usage.model_requests += usage.model_requests;
                agent.snapshot.usage.unreported_requests += usage.unreported_requests;
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
            } => {
                if let Some(command) = command {
                    self.remember_command(command, event_id, address.clone());
                }
                let agent = self.agent_mut(&address)?;
                agent.snapshot.status = AgentStatus::Stopped;
                if !cleanup_pending {
                    agent.snapshot.execution_id = None;
                    agent.snapshot.active_config = None;
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
                        agent.snapshot.active_config = None;
                        agent.inbox.extend(agent.accepted.drain(..));
                        agent
                            .inbox
                            .make_contiguous()
                            .sort_by_key(|entry| entry.receipt.event_id);
                        agent.snapshot.pending_inputs = agent.inbox.len();
                    }
                }
            }
            Record::PermissionWaiting { address, request } => {
                let agent = self.agent_mut(&address)?;
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
                agent.snapshot.question = None;
                if agent.snapshot.permission.is_some() {
                    agent.snapshot.status = AgentStatus::Waiting;
                } else if agent.snapshot.status != AgentStatus::Running {
                    agent.snapshot.status = AgentStatus::Idle;
                }
                self.deliver(event_id, fingerprint, input)?;
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
            .cloned()
            .collect())
    }

    pub(super) fn view(&self, address: &str, cursor: u64) -> Result<AgentView> {
        Ok(AgentView {
            snapshot: self.agent(address)?.snapshot.clone(),
            agents: self
                .agents
                .values()
                .map(|agent| agent.snapshot.clone())
                .collect(),
            cursor,
            pending_inputs: self.pending_inputs(address)?,
        })
    }

    pub(super) fn mark_interrupted(&mut self) {
        for agent in self.agents.values_mut() {
            let snapshot = &mut agent.snapshot;
            if snapshot.status == AgentStatus::Running
                || snapshot.status == AgentStatus::Stopped && snapshot.execution_id.is_some()
            {
                snapshot.status = AgentStatus::Interrupted;
                snapshot.execution_id = None;
                if agent.permission_reply.is_none() {
                    snapshot.active_config = None;
                }
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

    fn deliver(&mut self, event_id: u64, fingerprint: String, input: Input) -> Result<()> {
        self.remember(
            input.command_id.clone(),
            fingerprint,
            event_id,
            input.target.clone(),
        );
        self.deliveries
            .insert(input.command_id.clone(), InputStatus::Queued);
        let receipt = self.commands[&input.command_id].1.clone();
        let agent = self.agent_mut(&input.target)?;
        agent.inbox.push_back(PendingInput { receipt, input });
        agent.snapshot.pending_inputs = agent.inbox.len();
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

fn restore_on_resume() -> bool {
    true
}

fn add_usage(total: &mut Option<u64>, amount: Option<u64>) -> Result<()> {
    if let Some(amount) = amount {
        *total = Some(
            total
                .unwrap_or(0)
                .checked_add(amount)
                .ok_or_else(|| Error::InvalidEventStream("usage counter overflow".into()))?,
        );
    }
    Ok(())
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
