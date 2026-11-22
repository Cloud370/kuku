use std::collections::HashSet;

use crate::event::{EventPayload, ModelStopReason};
use crate::provider::types::ProviderToolCall;

use super::types::{PendingRun, PendingStep};

const MAX_TOOL_ARGUMENT_RETRIES: u8 = 5;

pub(super) fn validate_tool_calls(
    tool_calls: &mut [ProviderToolCall],
    argument_buffers: Vec<(u64, String)>,
    completions: Vec<u64>,
    stream_invalid: bool,
    admitted_tool_call_ids: &HashSet<String>,
) -> std::result::Result<(), ToolResponseError> {
    if stream_invalid {
        return Err(ToolResponseError::InvalidLifecycle);
    }
    if tool_calls.is_empty() {
        return Err(ToolResponseError::MissingCalls);
    }

    let mut ids = HashSet::new();
    let mut indices = HashSet::new();
    for tool_call in tool_calls.iter() {
        if tool_call.id.trim().is_empty() {
            return Err(ToolResponseError::EmptyId);
        }
        if tool_call.name.trim().is_empty() {
            return Err(ToolResponseError::EmptyName);
        }
        if admitted_tool_call_ids.contains(&tool_call.id) || !ids.insert(&tool_call.id) {
            return Err(ToolResponseError::DuplicateId);
        }
        if !indices.insert(tool_call.index) {
            return Err(ToolResponseError::DuplicateIndex);
        }
    }

    if argument_buffers.len() != tool_calls.len() {
        return Err(ToolResponseError::InvalidBufferMapping);
    }
    if completions.len() != tool_calls.len() {
        return Err(ToolResponseError::InvalidCompletionMapping);
    }

    let mut completed_indices = HashSet::new();
    for index in completions {
        if !indices.contains(&index) || !completed_indices.insert(index) {
            return Err(ToolResponseError::InvalidCompletionMapping);
        }
    }

    let mut buffered_indices = HashSet::new();
    for (index, buffer) in argument_buffers {
        if !buffered_indices.insert(index) {
            return Err(ToolResponseError::InvalidBufferMapping);
        }
        let Some(tool_call) = tool_calls
            .iter_mut()
            .find(|tool_call| tool_call.index == index)
        else {
            return Err(ToolResponseError::InvalidBufferMapping);
        };
        let args = if buffer.is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&buffer).map_err(|error| ToolResponseError::InvalidArguments {
                index,
                problem: ToolArgumentProblem::InvalidJson {
                    line: error.line(),
                    column: error.column(),
                },
            })?
        };
        if !args.is_object() {
            return Err(ToolResponseError::InvalidArguments {
                index,
                problem: ToolArgumentProblem::NonObject,
            });
        }
        tool_call.args = args;
    }
    if buffered_indices != indices {
        return Err(ToolResponseError::InvalidBufferMapping);
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToolResponseError {
    InvalidLifecycle,
    MissingCalls,
    EmptyId,
    EmptyName,
    DuplicateId,
    DuplicateIndex,
    InvalidBufferMapping,
    InvalidCompletionMapping,
    InvalidArguments {
        index: u64,
        problem: ToolArgumentProblem,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub(super) enum ToolArgumentProblem {
    InvalidJson { line: usize, column: usize },
    NonObject,
}

impl ToolResponseError {
    pub(super) fn message(self) -> &'static str {
        match self {
            Self::InvalidLifecycle => "The model sent tool events in an invalid order.",
            Self::MissingCalls => "The model ended the response without a complete tool call.",
            Self::EmptyId => "The model sent a tool call without an ID.",
            Self::EmptyName => "The model sent a tool call without a name.",
            Self::DuplicateId => "The model reused a tool call ID.",
            Self::DuplicateIndex => "The model reused a tool call index.",
            Self::InvalidBufferMapping => {
                "The model sent arguments that could not be matched to its tool calls."
            }
            Self::InvalidCompletionMapping => {
                "The model sent completion events that could not be matched to its tool calls."
            }
            Self::InvalidArguments { .. } => {
                "The model sent tool arguments that were not a valid JSON object."
            }
        }
    }
}

pub(super) fn stop_reason(
    reported: Option<ModelStopReason>,
    invalid_terminal: bool,
    validation: Option<&std::result::Result<(), ToolResponseError>>,
) -> ModelStopReason {
    use ModelStopReason::{EndTurn, InvalidResponse, ToolUse};

    if invalid_terminal {
        return InvalidResponse;
    }
    match (reported, validation) {
        (None | Some(ToolUse), Some(Ok(_))) => ToolUse,
        (None, None) => EndTurn,
        (None, Some(Err(_))) | (Some(ToolUse), _) | (Some(EndTurn), Some(_)) => InvalidResponse,
        (Some(reason), _) => reason,
    }
}

pub(super) fn can_regenerate(pending: &PendingRun) -> bool {
    pending.query.runtime_execution.is_some()
        && pending.query.runtime_limits.retry_transient_provider_errors
        && pending.tool_argument_retries < MAX_TOOL_ARGUMENT_RETRIES
        && !super::budget::exhausted(&pending.query)
}

pub(super) fn regenerate(mut pending: PendingRun, request_id: &str) -> crate::Result<PendingStep> {
    let execution = pending
        .query
        .runtime_execution
        .expect("Session owns regeneration");
    let failed_attempt = pending.tool_argument_retries + 1;
    let operation = EventPayload::Unknown(serde_json::json!({
        "kind":"runtime.model_retry", "ts": super::helpers::now_timestamp()?,
        "agent":pending.conversation.as_str(), "turn":pending.turn, "request_id": request_id,
        "phase":"waiting", "attempt":failed_attempt + 1, "failed_attempt":failed_attempt,
        "max_attempts":MAX_TOOL_ARGUMENT_RETRIES + 1,
        "delay_ms":0, "will_retry":true, "failed_kind":"invalid_response",
        "recovery":"regenerate_tool_arguments", "provider_stop_reason":"tool_use",
        "message":"Regenerating the model response after invalid tool arguments. The batch was discarded before execution.",
    }));
    super::provider::runtime_session(&pending, execution)?.commit_model_facts(
        pending.conversation.as_str(),
        execution,
        [operation],
    )?;
    pending.tool_argument_retries = failed_attempt;
    Ok(PendingStep::Pending {
        pending: Box::new(pending),
        slot: None,
        event: None,
    })
}

pub(super) fn resumed_tool_argument_retries(
    events: &[crate::event::StoredEvent],
    turn: u64,
    agent: &str,
) -> u8 {
    let mut retries = 0u8;
    for event in events.iter().rev() {
        match &event.payload {
            EventPayload::ToolCall {
                turn: recorded_turn,
                conversation,
                ..
            } if *recorded_turn == turn
                && conversation
                    .as_deref()
                    .map_or(agent == "main", |owner| owner == agent) =>
            {
                break
            }
            EventPayload::Unknown(value)
                if value["kind"] == "runtime.model_retry"
                    && value["recovery"] == "regenerate_tool_arguments"
                    && value["turn"] == turn
                    && value["agent"] == agent =>
            {
                retries = retries.saturating_add(1)
            }
            _ => {}
        }
    }
    retries
}

#[cfg(test)]
mod tests {
    use super::resumed_tool_argument_retries;
    use crate::event::{EventPayload, StoredEvent};
    use serde_json::json;

    #[test]
    fn restart_recovers_only_the_current_steps_argument_retries() {
        let retry = |id, agent: &str| StoredEvent {
            id,
            payload: EventPayload::Unknown(json!({
                "kind":"runtime.model_retry", "recovery":"regenerate_tool_arguments", "turn":7, "agent":agent,
            })),
        };
        let admitted = StoredEvent {
            id: 2,
            payload: EventPayload::ToolCall {
                turn: 7,
                ts: String::new(),
                conversation: Some("main".into()),
                tool_call_id: "read".into(),
                request_id: "req_2".into(),
                index: 0,
                tool: "read_file".into(),
                args: json!({"path":"evidence.txt"}),
            },
        };
        let events = [
            retry(1, "main"),
            admitted,
            retry(3, "main"),
            retry(4, "main"),
            retry(5, "reviewer"),
        ];
        assert_eq!(0, resumed_tool_argument_retries(&events[..2], 7, "main"));
        assert_eq!(2, resumed_tool_argument_retries(&events, 7, "main"));
        assert_eq!(0, resumed_tool_argument_retries(&events, 8, "main"));
    }
}
