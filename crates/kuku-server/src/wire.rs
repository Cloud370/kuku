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
        input.source = InputSource::Host("http".into());
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
}

impl CommandResult {
    pub(crate) fn new(receipt: Receipt, replayed: bool, status: Option<InputStatus>) -> Self {
        Self {
            ok: true,
            receipt,
            replayed,
            status,
        }
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

pub(crate) fn history_entry(
    event: &StoredEvent,
    display_number: impl FnOnce(&str, u64) -> Option<u64>,
) -> Option<Value> {
    let agent = event_agent(&event.payload).unwrap_or("main");
    let mut entry = match &event.payload {
        EventPayload::MessageUser {
            turn, text, from, ..
        } => {
            json!({"type":"user","turn":turn,"text":text,"from":from})
        }
        EventPayload::ModelResponse {
            turn,
            text,
            thinking,
            request_id,
            ..
        } if !text.is_empty()
            || thinking
                .as_ref()
                .is_some_and(|thinking| !thinking.is_empty()) =>
        {
            json!({"type":"assistant","turn":turn,"text":text,"thinking":thinking,"request_id":request_id})
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
        EventPayload::TurnCancelled { turn, reason, .. } => {
            json!({"type":"status","turn":turn,"status":"cancelled","text":reason})
        }
        EventPayload::TurnInterrupted { turn, reason, .. } => {
            json!({"type":"status","turn":turn,"status":"interrupted","text":reason})
        }
        EventPayload::Unknown(value) if value["kind"] == "runtime.operation" => {
            interaction_entry(&value["record"])?
        }
        _ => return None,
    };
    entry["id"] = json!(event.id);
    entry["agent"] = json!(agent);
    if let Some(number) = entry["turn"]
        .as_u64()
        .and_then(|turn| display_number(agent, turn))
    {
        entry["turn_number"] = json!(number);
    }
    Some(entry)
}

pub(crate) fn committed_frame(
    event: &StoredEvent,
    target: Option<&str>,
    include_agent_activity: bool,
    display_number: impl FnOnce(&str, u64) -> Option<u64>,
) -> Value {
    let kind = event.payload.kind_name();
    let agent = event_agent(&event.payload);
    let mut value = json!({"type":"committed","id":event.id,"kind":kind});
    if let Some(agent) = agent {
        value["agent"] = json!(agent);
    }
    let matches = target.is_none_or(|target| agent.unwrap_or("main") == target);
    if matches {
        if let Some(entry) = history_entry(event, display_number) {
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
) -> Option<Value> {
    let mut frame = live_payload(event_value(event)?, agent, execution_id, turn, cursor);
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
        UiEvent::TextDelta { text } => json!({"type":"text","content":agent_preview(text)}),
        UiEvent::ThinkingDelta { text } => json!({"type":"thinking","content":agent_preview(text)}),
        UiEvent::ModelRequest { .. } | UiEvent::ModelRecovery { .. } => event_value(event)?,
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
        "content": reply.text,
        "truncated": reply.truncated,
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
        _ => None,
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
