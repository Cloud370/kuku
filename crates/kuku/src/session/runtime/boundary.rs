use std::borrow::Cow;

use crate::event::{EventPayload, EventStore};
use crate::execution::OutputReference;
use crate::{Error, Result};

use super::record::{timestamp, AgentState, QueuedDelivery, Record, State};
use super::{
    AgentConfig, AgentStatus, InputSource, InputStatus, InputTiming, Session, UsageTotals,
};

pub(super) struct TurnWork {
    pub(super) config: AgentConfig,
    pub(super) text: String,
    pub(super) skip_user_message: bool,
    pub(super) resume_input_turn: Option<u64>,
    pub(super) source: InputSource,
    pub(super) reply: Option<(String, bool)>,
    pub(super) execution: u64,
    pub(super) turn: u64,
    pub(super) skills: Vec<String>,
}

pub(crate) struct StepBoundary {
    pub(crate) source: Option<InputSource>,
    pub(crate) config: Option<AgentConfig>,
    pub(crate) skills: Vec<String>,
}

impl Session {
    pub(crate) fn prepare_step_boundary(
        &self,
        address: &str,
        execution: u64,
        turn: u64,
    ) -> Result<StepBoundary> {
        let mut state = self.inner.state.lock().unwrap();
        let agent = self.executing_agent(&state, address, execution)?;
        if agent.snapshot.turn != Some(turn) {
            return Err(Error::ExecutionNotActive(execution));
        }
        let inputs = self.select_inputs(agent, Some(InputTiming::NextStep));
        let config = update_model_selection(&agent.snapshot);
        let config = config
            .map(|config| -> Result<_> {
                self.resolve_model(&config)?;
                self.commit(
                    &mut state,
                    Record::StepConfigured {
                        address: address.into(),
                        config: config.clone(),
                    },
                )?;
                Ok(config)
            })
            .transpose()?;
        let source = self.accept_steering(&mut state, address, turn, &inputs)?;
        Ok(StepBoundary {
            source,
            config,
            skills: input_skills(&inputs),
        })
    }

    fn accept_steering(
        &self,
        state: &mut State,
        address: &str,
        turn: u64,
        inputs: &[QueuedDelivery],
    ) -> Result<Option<InputSource>> {
        let Some(first) = inputs.first() else {
            return Ok(None);
        };
        let source = first.input.source.clone();
        self.accept(state, address, inputs, Some(turn))?;
        EventStore::open(self.events_path())?.append(EventPayload::MessageUser {
            turn,
            ts: timestamp()?,
            conversation: address.into(),
            text: input_text(inputs),
            from: source.agent_address().map(str::to_owned),
            via_tool_call_id: None,
        })?;
        Ok(Some(source))
    }

    pub(crate) fn require_execution(&self, address: &str, execution: u64) -> Result<()> {
        let state = self.inner.state.lock().unwrap();
        self.executing_agent(&state, address, execution).map(|_| ())
    }

    pub(crate) fn mark_tool_dispatch(
        &self,
        address: &str,
        execution: u64,
        turn: u64,
        tool_call_id: &str,
    ) -> Result<()> {
        let state = self.inner.state.lock().unwrap();
        if self
            .executing_agent(&state, address, execution)?
            .snapshot
            .turn
            != Some(turn)
        {
            return Err(Error::ExecutionNotActive(execution));
        }
        EventStore::open(self.events_path())?.append(EventPayload::Unknown(serde_json::json!({
            "kind": "runtime.tool_dispatched", "ts": timestamp()?, "agent": address,
            "turn": turn, "tool_call_id": tool_call_id,
        })))?;
        Ok(())
    }

    pub(super) fn accept_turn(&self, address: &str, epoch: u64) -> Result<Option<TurnWork>> {
        let mut state = self.inner.state.lock().unwrap();
        let agent = state.agent(address)?;
        if agent.active_epoch != Some(epoch)
            || agent.snapshot.status != AgentStatus::Running
            || state.closing
        {
            return Ok(None);
        }
        let execution = agent
            .snapshot
            .execution_id
            .expect("running agent has a durable execution ID");
        let reply = agent.permission_reply.clone();
        let config = agent
            .snapshot
            .active_config
            .as_ref()
            .unwrap_or(&agent.snapshot.config)
            .clone();
        let turn = agent.snapshot.turn;
        let inputs = if reply.is_some() {
            agent.accepted.clone()
        } else {
            self.select_inputs(agent, None)
        };
        if inputs.is_empty() && reply.is_none() {
            self.commit(
                &mut state,
                Record::TurnSettled {
                    address: address.into(),
                    status: AgentStatus::Idle,
                    output: None,
                    error: None,
                    turn: None,
                    usage: UsageTotals::default(),
                },
            )?;
            state.agent_mut(address)?.active_epoch = None;
            return Ok(None);
        }
        let source = inputs
            .last()
            .map(|entry| entry.input.source.clone())
            .unwrap_or_else(|| InputSource::Host("local".into()));
        let (text, skip_user_message, resume_input_turn) = if reply.is_some() {
            let resume_input_turn = inputs
                .last()
                .and_then(|entry| entry.accepted_turn)
                .or(turn);
            let text = if let Some(source_turn) = resume_input_turn {
                last_user_message_text(self.events_path(), address, source_turn)?
                    .unwrap_or_else(|| input_text(&inputs))
            } else {
                input_text(&inputs)
            };
            (text, true, resume_input_turn)
        } else {
            let mut queued = Vec::new();
            for entry in &inputs {
                match state.input_status(address, &entry.input.command_id)? {
                    InputStatus::Queued => queued.push(entry.clone()),
                    InputStatus::Accepted => {}
                    InputStatus::Withdrawn => {
                        return Err(Error::InvalidEventStream(format!(
                            "agent '{address}': withdrawn input '{}' remains in its inbox",
                            entry.input.command_id
                        )))
                    }
                }
            }
            if queued.is_empty() && !inputs.is_empty() {
                let resume_input_turn = inputs.last().and_then(|entry| entry.accepted_turn);
                if let Some(turn) = resume_input_turn {
                    if let Some(text) = last_user_message_text(self.events_path(), address, turn)? {
                        (text, true, Some(turn))
                    } else {
                        (input_text(&inputs), false, None)
                    }
                } else {
                    (input_text(&inputs), false, None)
                }
            } else {
                (input_text(&queued), false, None)
            }
        };
        let turn = if reply.is_some() {
            turn.ok_or_else(|| {
                Error::InvalidEventStream(format!(
                    "agent '{address}' pending permission has no turn"
                ))
            })?
        } else {
            self.accept(&mut state, address, &inputs, None)?
        };
        Ok(Some(TurnWork {
            config,
            text,
            skip_user_message,
            resume_input_turn,
            source,
            skills: if reply.is_some() {
                Vec::new()
            } else {
                input_skills(&inputs)
            },
            reply,
            execution,
            turn,
        }))
    }

    pub(crate) fn has_step_input(&self, address: &str) -> Result<bool> {
        let state = self.inner.state.lock().unwrap();
        Ok(state
            .agent(address)?
            .inbox
            .iter()
            .any(|entry| entry.input.timing == InputTiming::NextStep))
    }

    fn select_inputs(
        &self,
        agent: &AgentState,
        timing: Option<InputTiming>,
    ) -> Vec<QueuedDelivery> {
        let mut eligible = agent
            .inbox
            .iter()
            .filter(|entry| timing.is_none_or(|timing| entry.input.timing == timing))
            .peekable();
        let source = eligible.peek().map(|entry| &entry.input.source);
        let mut bytes = 0;
        eligible
            .take_while(|entry| {
                bytes += input_model_text(&entry.input.text, &entry.values).len() + 2;
                Some(&entry.input.source) == source
                    && bytes <= self.inner.options.limits.max_input_bytes + 2
            })
            .cloned()
            .collect()
    }

    fn accept(
        &self,
        state: &mut State,
        address: &str,
        entries: &[QueuedDelivery],
        turn: Option<u64>,
    ) -> Result<u64> {
        self.commit(
            state,
            Record::InputsAccepted {
                address: address.into(),
                input_ids: entries
                    .iter()
                    .map(|entry| entry.input.command_id.clone())
                    .collect(),
                turn,
            },
        )
    }

    pub(super) fn executing_agent<'a>(
        &self,
        state: &'a State,
        address: &str,
        execution: u64,
    ) -> Result<&'a AgentState> {
        let agent = state.agent(address)?;
        if state.closing
            || agent.snapshot.status != AgentStatus::Running
            || agent.snapshot.execution_id != Some(execution)
        {
            return Err(Error::ExecutionNotActive(execution));
        }
        Ok(agent)
    }
}

fn update_model_selection(snapshot: &super::AgentSnapshot) -> Option<AgentConfig> {
    let requested = &snapshot.config;
    let active = snapshot.active_config.as_ref()?;
    let config = AgentConfig {
        connection: requested.connection.clone(),
        tier: requested.tier.clone(),
        model: requested.model.clone(),
        reasoning_effort: requested.reasoning_effort.clone(),
        service_tier: requested.service_tier.clone(),
        max_output_tokens: requested.max_output_tokens,
        ..active.clone()
    };
    (config != *active).then_some(config)
}

fn input_text(entries: &[QueuedDelivery]) -> String {
    entries
        .iter()
        .map(|entry| input_model_text(&entry.input.text, &entry.values))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn last_user_message_text(
    events_path: &std::path::Path,
    address: &str,
    turn: u64,
) -> Result<Option<String>> {
    let mut after = 0;
    let mut text = None;
    loop {
        let events = EventStore::read_page(events_path, after, 256)?;
        if events.is_empty() {
            break;
        }
        for event in events {
            after = event.id;
            if let EventPayload::MessageUser {
                conversation,
                turn: event_turn,
                text: event_text,
                ..
            } = event.payload
            {
                if conversation == address && event_turn == turn {
                    text = Some(event_text);
                }
            }
        }
    }
    Ok(text)
}

pub(super) fn input_model_text<'a>(text: &'a str, values: &[OutputReference]) -> Cow<'a, str> {
    if values.is_empty() {
        return Cow::Borrowed(text);
    }
    let separator = if text.is_empty() { "" } else { "\n\n" };
    let references = serde_json::to_string(values).expect("output references are serializable");
    Cow::Owned(format!(
        "{text}{separator}<kuku_output_refs>{references}</kuku_output_refs>"
    ))
}

fn input_skills(entries: &[QueuedDelivery]) -> Vec<String> {
    entries
        .iter()
        .filter_map(|entry| entry.input.bootstrap_skill.clone())
        .collect()
}
