use std::collections::{BTreeMap, HashMap, VecDeque};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::event::{EventPayload, StoredEvent};
use crate::{Error, Result};

use super::types::{
    AgentConfig, AgentSnapshot, AgentStatus, Input, QuestionRequest, Receipt, UsageTotals,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub(super) enum Record {
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
    },
    InputsAccepted {
        address: String,
        input_ids: Vec<String>,
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
    },
    AgentResumed {
        address: String,
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
    pub(super) inbox: VecDeque<Input>,
    pub(super) accepted: Vec<Input>,
    pub(super) permission_reply: Option<(String, bool)>,
    pub(super) active_epoch: Option<u64>,
}

#[derive(Debug, Default)]
pub(super) struct State {
    pub(super) agents: BTreeMap<String, AgentState>,
    pub(super) commands: HashMap<String, (String, Receipt)>,
    pub(super) closing: bool,
    pub(super) closed: bool,
    pub(super) next_epoch: u64,
    pub(super) cleanup_failures: BTreeMap<String, String>,
}

impl State {
    pub(super) fn reduce(&mut self, event_id: u64, record: Record) -> Result<()> {
        match record {
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
                            status: AgentStatus::Idle,
                            pending_inputs: 0,
                            turn: None,
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
            Record::InputDelivered { fingerprint, input } => {
                self.remember(
                    input.command_id.clone(),
                    fingerprint,
                    event_id,
                    input.target.clone(),
                );
                let agent = self.agent_mut(&input.target)?;
                agent.inbox.push_back(input);
                agent.snapshot.pending_inputs = agent.inbox.len();
            }
            Record::InputsAccepted { address, input_ids } => {
                let agent = self.agent_mut(&address)?;
                for id in input_ids {
                    let index = agent
                        .inbox
                        .iter()
                        .position(|input| input.command_id == id)
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
                agent.snapshot.turn = Some(event_id);
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
            Record::AgentStopped { address } => {
                let agent = self.agent_mut(&address)?;
                agent.snapshot.status = AgentStatus::Stopped;
                agent.snapshot.question = None;
                agent.snapshot.permission = None;
                agent.permission_reply = None;
            }
            Record::AgentResumed { address } => {
                let agent = self.agent_mut(&address)?;
                agent.snapshot.status = AgentStatus::Idle;
                while let Some(input) = agent.accepted.pop() {
                    agent.inbox.push_front(input);
                }
                agent.snapshot.pending_inputs = agent.inbox.len();
            }
            Record::PermissionWaiting { address, request } => {
                let agent = self.agent_mut(&address)?;
                agent.snapshot.status = AgentStatus::Waiting;
                agent.snapshot.permission = Some(request);
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
                self.remember(
                    input.command_id.clone(),
                    fingerprint,
                    event_id,
                    input.target.clone(),
                );
                let agent = self.agent_mut(&input.target)?;
                agent.snapshot.question = None;
                if agent.snapshot.permission.is_some() {
                    agent.snapshot.status = AgentStatus::Waiting;
                } else if agent.snapshot.status != AgentStatus::Running {
                    agent.snapshot.status = AgentStatus::Idle;
                }
                agent.inbox.push_back(input);
                agent.snapshot.pending_inputs = agent.inbox.len();
            }
        }
        Ok(())
    }

    pub(super) fn agent_mut(&mut self, address: &str) -> Result<&mut AgentState> {
        self.agents.get_mut(address).ok_or_else(|| {
            Error::InvalidArgument(format!("agent '{address}' does not exist in this session"))
        })
    }

    pub(super) fn retry(&self, command_id: &str, fingerprint: &str) -> Result<Option<Receipt>> {
        match self.commands.get(command_id) {
            Some((original, receipt)) if original == fingerprint => Ok(Some(receipt.clone())),
            Some(_) => Err(Error::InvalidArgument(format!(
                "command '{command_id}' was already committed with different content"
            ))),
            None => Ok(None),
        }
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
