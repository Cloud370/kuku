//! Command envelopes and the client view of committed and live Session facts.

use std::path::PathBuf;

use kuku::event::{EventPayload, StoredEvent};
use kuku::session::{
    AgentConfig, AgentSnapshot, Input, InputSource, InputStatus, InputTiming, Receipt, ReplyPreview,
};
use kuku::UiEvent;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::run_manager::TurnLookup;

#[derive(Deserialize)]
pub(crate) struct CommandRequest {
    pub(crate) workspace: PathBuf,
    pub(crate) agent: String,
    pub(crate) command_id: String,
    #[serde(flatten)]
    pub(crate) command: Command,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum Command {
    Send {
        text: String,
        #[serde(default)]
        timing: SendTiming,
        bootstrap_skill: Option<String>,
        create: Option<Box<AgentConfig>>,
        config_patch: Option<Box<AgentConfig>>,
        #[serde(default)]
        create_agent_if_missing: bool,
        prompts_dir: Option<PathBuf>,
    },
    CreateAgent {
        #[serde(default)]
        config: Box<AgentConfig>,
    },
    Configure {
        config: Box<AgentConfig>,
    },
    ReplyQuestion {
        request_id: String,
        text: String,
    },
    ReplyPermission {
        request_id: String,
        allow: bool,
    },
    Withdraw {
        input_command_id: String,
    },
    ReplaceQueued {
        input_command_id: String,
        text: String,
    },
    RewindMessage {
        target_event_id: u64,
        text: String,
        #[serde(default)]
        restore_files: bool,
    },
    Advance,
    Cancel {
        execution_id: u64,
    },
}

impl Command {
    pub(crate) fn creation(&self) -> Option<&AgentConfig> {
        if let Self::Send { create, .. } = self {
            create.as_deref()
        } else {
            None
        }
    }

    pub(crate) fn input(&self, command_id: &str, address: &str) -> kuku::Result<Option<Input>> {
        let Self::Send {
            text,
            timing,
            bootstrap_skill,
            config_patch,
            create_agent_if_missing,
            ..
        } = self
        else {
            return Ok(None);
        };
        let mut input = Input::new(command_id, address, text);
        input.source = http_input_source();
        input.bootstrap_skill.clone_from(bootstrap_skill);
        input.timing = match timing {
            SendTiming::Steer => InputTiming::NextStep,
            SendTiming::NextTurn => InputTiming::NextTurn,
        };
        if config_patch.is_some() || *create_agent_if_missing {
            let digest = Sha256::digest(serde_json::to_vec(&(
                config_patch,
                create_agent_if_missing,
            ))?);
            input.correlation = Some(format!("http:send:{digest:x}"));
        }
        Ok(Some(input))
    }
}

pub(crate) fn http_input_source() -> InputSource {
    InputSource::Host("http".into())
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SendTiming {
    #[default]
    Steer,
    NextTurn,
}

#[derive(Serialize)]
pub(crate) struct CommandResult {
    ok: bool,
    pub(crate) receipt: Receipt,
    replayed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<InputStatus>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
}

impl CommandResult {
    pub(crate) fn new(receipt: Receipt, replayed: bool, status: Option<InputStatus>) -> Self {
        Self {
            ok: true,
            receipt,
            replayed,
            status,
            warnings: Vec::new(),
        }
    }

    pub(crate) fn with_warnings(mut self, warnings: Vec<String>) -> Self {
        self.warnings = warnings;
        self
    }
}

/// Encode a live runtime event as one line of the shared CLI event format.
pub fn serialize_event(event: &UiEvent) -> Option<String> {
    Some(format!("{}\n", event_value(event)?))
}

pub(crate) fn event_agent(payload: &EventPayload) -> Option<&str> {
    match payload {
        EventPayload::ModelResponse { conversation, .. }
        | EventPayload::ModelError { conversation, .. }
        | EventPayload::ToolCall { conversation, .. }
        | EventPayload::ToolResult { conversation, .. } => conversation.as_deref(),
        EventPayload::ConversationOpened { conversation, .. }
        | EventPayload::ConversationBound { conversation, .. }
        | EventPayload::PromptSnapshot { conversation, .. }
        | EventPayload::MessageUser { conversation, .. }
        | EventPayload::MessageAssistant { conversation, .. }
        | EventPayload::TurnStarted { conversation, .. }
        | EventPayload::TurnCompleted { conversation, .. }
        | EventPayload::TurnCancelled { conversation, .. }
        | EventPayload::TurnInterrupted { conversation, .. }
        | EventPayload::ConversationRollback { conversation, .. }
        | EventPayload::ConversationRollbackUndone { conversation, .. }
        | EventPayload::ModelRecovery { conversation, .. }
        | EventPayload::ContextSkills { conversation, .. } => Some(conversation),
        EventPayload::Unknown(value) => value["agent"]
            .as_str()
            .or_else(|| value["conversation"].as_str())
            .or_else(|| value["record"]["address"].as_str())
            .or_else(|| value["record"]["input"]["target"].as_str()),
        _ => None,
    }
}

pub(crate) fn agent_summary(agent: &AgentSnapshot) -> Value {
    json!({
        "address": agent.address,
        "status": agent.status,
        "pending_inputs": agent.pending_inputs,
        "admitted_task": agent.admitted_task,
        "queued_task": agent.queued_task,
        "execution_id": agent.execution_id,
        "turn": agent.turn,
        "permission": agent.permission,
        "question": agent.question,
        "last_output_preview": agent.last_output.as_deref().map(agent_preview),
        "last_error_preview": agent.last_error.as_deref().map(agent_preview),
    })
}

pub(crate) fn history_entry(event: &StoredEvent, turns: &TurnLookup) -> Option<Value> {
    let agent = event_agent(&event.payload).unwrap_or("main");
    let mut summary = None;
    let mut entry = match &event.payload {
        EventPayload::MessageUser {
            turn, text, from, ..
        } => {
            json!({"type":"user","turn":turn,"text":text,"from":from})
        }
        EventPayload::ModelResponse {
            turn,
            text,
            request_id,
            thinking,
            ..
        } if !text.is_empty() || thinking.as_ref().is_some_and(|text| !text.is_empty()) => {
            let mut entry =
                json!({"type":"assistant","turn":turn,"text":text,"request_id":request_id});
            if let Some(summary) = thinking.as_ref().filter(|text| !text.is_empty()) {
                entry["reasoning_preview"] = json!(summary);
            }
            entry
        }
        EventPayload::ModelError {
            turn,
            kind,
            message,
            request_id,
            ..
        } => {
            json!({"type":"error","turn":turn,"code":kind,"text":message,"request_id":request_id})
        }
        EventPayload::ModelRecovery { turn, .. } => json!({
            "type":"retry",
            "turn":turn,
            "retry":retry_metadata(&event.payload)?,
        }),
        EventPayload::ToolCall {
            turn,
            tool_call_id,
            tool,
            args,
            ..
        } => {
            json!({"type":"tool_call","turn":turn,"tool_call_id":tool_call_id,"tool":tool,"args":args})
        }
        EventPayload::ToolResult {
            turn,
            tool_call_id,
            status,
            summary,
            model_content,
            truncated,
            structured,
            ..
        } => json!({
            "type":"tool_result",
            "turn":turn,
            "tool_call_id":tool_call_id,
            "status":status,
            "summary":summary,
            "model_content":model_content,
            "truncated":truncated,
            "structured":structured,
            "output_id":structured.as_ref().and_then(output_id),
        }),
        EventPayload::TurnCancelled {
            turn,
            reason,
            stop_event_id: None,
            ..
        } if !turns.explicitly_stopped(agent, *turn) => {
            json!({"type":"status","turn":turn,"status":"cancelled","text":reason})
        }
        EventPayload::TurnInterrupted { turn, reason, .. }
            if !turns.explicitly_stopped(agent, *turn) =>
        {
            json!({"type":"status","turn":turn,"status":"interrupted","text":reason})
        }
        EventPayload::Unknown(value) if value["kind"] == "runtime.operation" => {
            let record = &value["record"];
            match record["operation"].as_str()? {
                "turn_settled" => {
                    summary = Some(turns.summary_at(event.id)?);
                    json!({"type":"turn_summary"})
                }
                "agent_stopped" => {
                    summary = turns.summary_at(event.id);
                    interaction_entry(record)?
                }
                _ => interaction_entry(record)?,
            }
        }
        EventPayload::Unknown(value) if value["kind"] == "runtime.model_retry" => json!({
            "type":"retry",
            "turn":value["turn"],
            "retry":retry_metadata(&event.payload)?,
        }),
        _ => return None,
    };
    entry["id"] = json!(event.id);
    entry["agent"] = json!(agent);
    if let Some((turn, summary)) = summary {
        entry["turn"] = json!(turn);
        entry["turn_summary"] = json!(summary);
    }
    if let Some(number) = entry["turn"]
        .as_u64()
        .and_then(|turn| turns.number(agent, turn))
    {
        entry["turn_number"] = json!(number);
    }
    Some(entry)
}

pub(crate) fn history_entry_for_agent(
    event: &StoredEvent,
    turns: &TurnLookup,
    agent: &str,
) -> Option<Value> {
    let mut entry = history_entry(event, turns)?;
    entry["agent"] = json!(agent);
    if let Some(turn) = entry["turn"].as_u64() {
        entry["turn_number"] = turns
            .number(agent, turn)
            .map_or(Value::Null, |number| json!(number));
    }
    Some(entry)
}

pub(crate) fn retry_metadata(payload: &EventPayload) -> Option<Value> {
    match payload {
        EventPayload::Unknown(record) if record["kind"] == "runtime.model_retry" => Some(json!({
            "source":"runtime.model_retry",
            "phase":record.get("phase").and_then(Value::as_str).unwrap_or("started"),
            "attempt":record["attempt"],
            "failed_attempt":record["failed_attempt"],
            "max_attempts":record["max_attempts"],
            "failed_kind":record["failed_kind"],
            "reason":record["reason"],
            "request_id":record["request_id"],
            "from_request_id":record["from_request_id"],
            "to_request_id":record["to_request_id"],
            "delay_ms":record["delay_ms"],
            "ts":record["ts"],
            "will_retry":record["will_retry"],
            "message":record["message"],
            "status":record["status"],
            "stop_reason":record["stop_reason"],
        })),
        EventPayload::ModelRecovery {
            ts,
            from_request_id,
            to_request_id,
            reason,
            attempt,
            max_attempts,
            ..
        } => Some(json!({
            "source":"model.recovery",
            "phase":"started",
            "attempt":attempt,
            "max_attempts":max_attempts,
            "reason":reason,
            "from_request_id":from_request_id,
            "to_request_id":to_request_id,
            "ts":ts,
        })),
        _ => None,
    }
}

pub(crate) fn committed_frame(
    event: &StoredEvent,
    target: Option<&str>,
    include_agent_activity: bool,
    turns: &TurnLookup,
) -> Value {
    let kind = event.payload.kind_name();
    let agent = event_agent(&event.payload);
    let mut value = json!({"type":"committed","id":event.id,"kind":kind});
    if let Some(agent) = agent {
        value["agent"] = json!(agent);
    }
    let matches = target.is_none_or(|target| agent.unwrap_or("main") == target);
    if matches {
        if let Some(entry) = history_entry(event, turns) {
            value["entry"] = entry;
        }
        match &event.payload {
            EventPayload::ModelResponse {
                turn, request_id, ..
            }
            | EventPayload::ModelError {
                turn, request_id, ..
            } => {
                value["turn"] = json!(turn);
                value["request_id"] = json!(request_id);
            }
            EventPayload::Unknown(record) if kind == "runtime.step_prepared" => {
                value["turn"] = record["turn"].clone();
                value["request_id"] = record["request_id"].clone();
            }
            EventPayload::Unknown(record) if kind == "runtime.tool_dispatched" => {
                value["tool_call_id"] = record["tool_call_id"].clone();
            }
            _ => {}
        }
    }
    if !matches && include_agent_activity {
        match &event.payload {
            EventPayload::ToolResult { tool_call_id, .. } => {
                value["tool_call_id"] = json!(tool_call_id);
            }
            EventPayload::Unknown(payload) if kind == "runtime.tool_dispatched" => {
                value["tool_call_id"] = payload["tool_call_id"].clone();
            }
            _ => {}
        }
    }
    if let EventPayload::Unknown(payload) = &event.payload {
        if kind == "runtime.operation" && payload["record"]["operation"] != "session_closed" {
            value["state_changed"] = json!(true);
            let record = &payload["record"];
            value["operation"] = record["operation"].clone();
            if matches {
                match record["operation"].as_str() {
                    Some("inputs_accepted") => {
                        value["delivery"] =
                            json!({"status":"accepted","input_ids":record["input_ids"]});
                        if record["turn"].is_null() {
                            value["turn"] = json!(event.id);
                            if let Some(start) =
                                turns.started_at_ms(agent.unwrap_or("main"), event.id)
                            {
                                value["turn_started_at_ms"] = json!(start);
                            }
                        }
                    }
                    Some("input_withdrawal") if record["status"] == "withdrawn" => {
                        value["delivery"] =
                            json!({"status":"withdrawn","input_id":record["input_command_id"]});
                    }
                    Some("input_replacement") => {
                        value["delivery"] = json!({
                            "status": "replaced",
                            "input_id": record["input_command_id"],
                            "replacement_id": record["command"]["command_id"],
                        });
                    }
                    _ => {}
                }
            }
            if !matches && include_agent_activity && record["operation"] == "agent_stopped" {
                value["cleanup_pending"] = record["cleanup_pending"].clone();
            }
            if record["operation"] == "turn_settled" {
                value["settlement"] = if !matches && include_agent_activity {
                    json!({
                        "status":record["status"],"turn":record["turn"],
                        "output":record["output"].as_str().map(agent_preview),
                        "error":record["error"].as_str().map(agent_preview),
                    })
                } else {
                    json!({
                        "status":record["status"],"turn":record["turn"],"output":record["output"],
                        "error":record["error"],"usage":record["usage"],
                    })
                };
            }
        }
    }
    value
}

pub(crate) fn live_frame(
    event: &UiEvent,
    agent: &str,
    execution_id: u64,
    turn: u64,
    cursor: u64,
    turn_number: Option<u64>,
    agent_scoped: bool,
) -> Option<Value> {
    let event = if agent_scoped {
        match event {
            UiEvent::ThinkingDelta { text } if !text.is_empty() => {
                json!({"type":"reasoning_preview", "content":text})
            }
            _ => event_value(event)?,
        }
    } else {
        event_value(event)?
    };
    let mut frame = live_payload(event, agent, execution_id, turn, cursor);
    if let Some(number) = turn_number {
        frame["turn_number"] = json!(number);
    }
    Some(frame)
}

pub(crate) fn agent_activity_frame(
    event: &UiEvent,
    agent: &str,
    execution_id: u64,
    turn: u64,
    cursor: u64,
) -> Option<Value> {
    let event = match event {
        UiEvent::TextDelta { text } if !text.is_empty() => {
            json!({"type":"text","content":agent_preview(text)})
        }
        UiEvent::ThinkingDelta { .. }
        | UiEvent::ModelProgress { .. }
        | UiEvent::ModelRequest { .. }
        | UiEvent::ModelRecovery { .. } => peer_event_value(event)?,
        _ => return None,
    };
    Some(live_payload(event, agent, execution_id, turn, cursor))
}

pub(crate) fn reply_preview_frame(
    agent: &str,
    cursor: u64,
    reply: &ReplyPreview,
    turn_number: Option<u64>,
) -> Value {
    let event = json!({
        "type": "reply_preview",
        "phase": reply.phase,
        "content": reply.text,
        "truncated": reply.truncated,
        "reasoning_preview": reply.reasoning_preview,
        "reasoning_preview_truncated": reply.reasoning_preview_truncated,
    });
    let mut frame = live_payload(event, agent, reply.execution_id, reply.turn, cursor);
    if let Some(number) = turn_number {
        frame["turn_number"] = json!(number);
    }
    frame
}

fn live_payload(event: Value, agent: &str, execution_id: u64, turn: u64, cursor: u64) -> Value {
    json!({
        "type":"live",
        "agent":agent,
        "execution_id":execution_id,
        "turn":turn,
        "cursor":cursor,
        "event":event,
    })
}

fn agent_preview(text: &str) -> &str {
    const MAX_PREVIEW_BYTES: usize = 512;
    &text[..text.floor_char_boundary(text.len().min(MAX_PREVIEW_BYTES))]
}

fn output_id(structured: &Value) -> Option<&str> {
    structured["output_id"]
        .as_str()
        .or_else(|| structured["output_ref"]["id"].as_str())
}

fn interaction_entry(record: &Value) -> Option<Value> {
    let request = &record["request"];
    match record["operation"].as_str()? {
        "question_requested" => Some(json!({
            "type":"question",
            "turn":request["turn"],
            "text":request["text"],
            "request":request,
        })),
        "permission_waiting" => Some(json!({
            "type":"permission",
            "turn":request["turn"],
            "text":request["summary"],
            "request":request,
        })),
        "question_replied" => Some(json!({
            "type":"question_reply",
            "request_id":record["request_id"],
            "text":record["input"]["text"],
        })),
        "permission_replied" => Some(json!({
            "type":"permission_reply",
            "request_id":record["request_id"],
            "allow":record["allow"],
        })),
        "agent_stopped" if record["interruption"].is_object() => {
            let stopped = &record["interruption"];
            Some(json!({
                "type":"status",
                "status":"interrupted",
                "execution_id":stopped["execution_id"],
                "turn":stopped["turn"],
                "text":stopped["text"],
                "truncated":stopped["truncated"].as_bool().unwrap_or(false),
            }))
        }
        _ => None,
    }
}

fn peer_event_value(event: &UiEvent) -> Option<Value> {
    match event {
        UiEvent::ThinkingDelta { text } => (!text.is_empty()).then(|| json!({"type":"thinking"})),
        _ => event_value(event),
    }
}

fn event_value(event: &UiEvent) -> Option<Value> {
    let mut value = kuku::wire::to_wire(event)?;
    match event {
        UiEvent::ToolStart {
            kind: kuku::query::ToolKind::Agent { conversation, .. },
            ..
        } => value["conversation"] = json!(conversation.as_str()),
        UiEvent::PermissionRequested { request }
        | UiEvent::ToolOutput {
            event: kuku::query::ToolEvent::PermissionRequested { request },
            ..
        } => {
            value["conversation"] = json!(request.conversation.as_str());
            value["turn"] = json!(request.turn);
        }
        UiEvent::Done { output, .. } => value["conversation"] = json!(output.conversation.as_str()),
        _ => {}
    }
    Some(value)
}

#[cfg(test)]
mod retry_tests {
    use kuku::event::EventPayload;
    use serde_json::json;

    use super::retry_metadata;

    #[test]
    fn legacy_retry_projection_defaults_to_started_and_preserves_metadata() {
        let payload = EventPayload::Unknown(json!({
            "kind":"runtime.model_retry","agent":"main","turn":7,
            "request_id":"request-2","failed_kind":"transport","attempt":2,
            "delay_ms":5000,"ts":"2025-01-01T00:00:00Z"
        }));
        let retry = retry_metadata(&payload).unwrap();
        assert_eq!("runtime.model_retry", retry["source"]);
        assert_eq!("started", retry["phase"]);
        assert_eq!(2, retry["attempt"]);
        assert_eq!("transport", retry["failed_kind"]);
        assert_eq!("request-2", retry["request_id"]);
        assert_eq!(5000, retry["delay_ms"]);
        assert_eq!("2025-01-01T00:00:00Z", retry["ts"]);
    }

    #[test]
    fn retry_projection_retains_future_phase_and_authoritative_stop_decision() {
        let payload = EventPayload::Unknown(json!({
            "kind":"runtime.model_retry","agent":"main","turn":7,
            "phase":"future_phase","attempt":2,"failed_attempt":1,
            "max_attempts":4,"failed_kind":"future_kind","will_retry":false,
            "stop_reason":"future_policy","message":"Service unavailable","status":503
        }));
        let retry = retry_metadata(&payload).unwrap();
        assert_eq!("future_phase", retry["phase"]);
        assert_eq!("future_kind", retry["failed_kind"]);
        assert_eq!(false, retry["will_retry"]);
        assert_eq!("future_policy", retry["stop_reason"]);
        assert_eq!(503, retry["status"]);
    }

    #[test]
    fn recovery_projection_preserves_its_own_attempt_and_request_ids() {
        let payload = EventPayload::ModelRecovery {
            conversation: "main".into(),
            turn: 7,
            ts: "2025-01-01T00:00:00Z".into(),
            from_request_id: "failed-request".into(),
            to_request_id: "recovery-request".into(),
            reason: kuku::event::ModelStopReason::Length,
            attempt: 2,
            max_attempts: 4,
            failed_max_output_tokens: 4096,
            retry_max_output_tokens: 2048,
            output_tokens_total: Some(4096),
            discarded_tool_calls: 0,
            notice: "recovered".into(),
            prompt_path: "prompt.md".into(),
            prompt_hash: "hash".into(),
        };
        let retry = retry_metadata(&payload).unwrap();
        assert_eq!("model.recovery", retry["source"]);
        assert_eq!("started", retry["phase"]);
        assert_eq!(2, retry["attempt"]);
        assert_eq!(4, retry["max_attempts"]);
        assert_eq!("length", retry["reason"]);
        assert_eq!("failed-request", retry["from_request_id"]);
        assert_eq!("recovery-request", retry["to_request_id"]);
    }
}
