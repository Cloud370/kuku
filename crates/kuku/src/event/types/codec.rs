use serde_json::{Map, Value};

use super::payload::EventPayload;

impl EventPayload {
    pub(super) fn from_json_object(object: &Map<String, Value>) -> Option<Self> {
        let kind = object.get("kind").and_then(Value::as_str)?;
        let value = Value::Object(object.clone());
        match kind {
            "task.ledger" => Some(Self::TaskLedger(
                serde_json::from_value(serde_json::json!({
                    "record_type": object.get("record_type")?.clone(),
                    "record": object.get("record")?.clone(),
                }))
                .ok()?,
            )),
            "context.sources" => Some(Self::ContextSources {
                request: serde_json::from_value(object.get("request")?.clone()).ok()?,
                turn: u64_field(object, "turn")?,
                ts: string_field(object, "ts")?,
                project_instruction_sources: serde_json::from_value(
                    object.get("project_instruction_sources")?.clone(),
                )
                .ok()?,
                memory_sources: serde_json::from_value(object.get("memory_sources")?.clone())
                    .ok()?,
            }),
            "context.skills" => Some(Self::ContextSkills {
                conversation: string_field(object, "conversation")?,
                turn: u64_field(object, "turn")?,
                ts: string_field(object, "ts")?,
                registry: object.get("registry")?.clone(),
                bootstrap_loaded: serde_json::from_value(object.get("bootstrap_loaded")?.clone())
                    .ok()?,
            }),
            "model.response" => Some(Self::ModelResponse {
                request: serde_json::from_value(object.get("request")?.clone()).ok()?,
                turn: u64_field(object, "turn")?,
                ts: string_field(object, "ts")?,
                text: string_field(object, "text")?,
                thinking: optional_string_field(object, "thinking"),
                input_tokens_total: optional_u32_field(object, "input_tokens_total"),
            }),
            "model.error" => Some(Self::ModelError {
                request: serde_json::from_value(object.get("request")?.clone()).ok()?,
                turn: u64_field(object, "turn")?,
                ts: string_field(object, "ts")?,
                kind: string_field(object, "error_kind")?,
                message: string_field(object, "message")?,
            }),
            "tool.call" => Some(Self::ToolCall {
                request: serde_json::from_value(object.get("request")?.clone()).ok()?,
                turn: u64_field(object, "turn")?,
                ts: string_field(object, "ts")?,
                conversation: optional_string_field(object, "conversation"),
                tool_call_id: string_field(object, "tool_call_id")?,
                index: u64_field(object, "index")?,
                tool: string_field(object, "tool")?,
                args: object.get("args")?.clone(),
            }),
            "permission.allow" => Some(Self::PermissionAllow {
                execution: serde_json::from_value(object.get("execution")?.clone()).ok()?,
                turn: u64_field(object, "turn")?,
                ts: string_field(object, "ts")?,
                tool_call_id: string_field(object, "tool_call_id")?,
                tool: string_field(object, "tool")?,
                scope: string_field(object, "scope")?,
                matcher: string_field(object, "matcher")?,
                source: string_field(object, "source")?,
            }),
            "permission.requested" => Some(Self::PermissionRequested {
                execution: serde_json::from_value(object.get("execution")?.clone()).ok()?,
                turn: u64_field(object, "turn")?,
                ts: string_field(object, "ts")?,
                tool_call_id: string_field(object, "tool_call_id")?,
                tool: string_field(object, "tool")?,
                risk: string_field(object, "risk")?,
                summary: string_field(object, "summary")?,
                candidate: string_field(object, "candidate")?,
                source: string_field(object, "source")?,
            }),
            "permission.deny" => Some(Self::PermissionDeny {
                execution: serde_json::from_value(object.get("execution")?.clone()).ok()?,
                turn: u64_field(object, "turn")?,
                ts: string_field(object, "ts")?,
                tool_call_id: string_field(object, "tool_call_id")?,
                tool: string_field(object, "tool")?,
                reason: string_field(object, "reason")?,
                source: string_field(object, "source")?,
            }),
            "tool.result" => Some(Self::ToolResult {
                execution: serde_json::from_value(object.get("execution")?.clone()).ok()?,
                turn: u64_field(object, "turn")?,
                ts: string_field(object, "ts")?,
                conversation: optional_string_field(object, "conversation"),
                tool_call_id: string_field(object, "tool_call_id")?,
                status: string_field(object, "status")?,
                summary: string_field(object, "summary")?,
                model_content: string_field(object, "model_content")?,
                truncated: bool_field(object, "truncated")?,
                files_read: vec_string_field(object, "files_read"),
                files_changed: vec_string_field(object, "files_changed"),
                commands_run: vec_string_field(object, "commands_run"),
                memory_changed: object.get("memory_changed").cloned(),
                structured: object.get("structured").cloned(),
            }),
            "handoff" => Some(Self::Handoff {
                execution: serde_json::from_value(object.get("execution")?.clone()).ok()?,
                turn: u64_field(object, "turn")?,
                ts: string_field(object, "ts")?,
                request_id: string_field(object, "request_id")?,
                summary: string_field(object, "summary")?,
                keep_turns: usize_field(object, "keep_turns")?,
            }),
            "session.created" => Some(Self::SessionCreated {
                ts: string_field(object, "ts")?,
                schema_version: u32_field(object, "schema_version")?,
                session_id: string_field(object, "session_id")?,
                created_at: string_field(object, "created_at")?,
                kuku_version: string_field(object, "kuku_version")?,
            }),
            "conversation.opened" => Some(Self::ConversationOpened {
                ts: string_field(object, "ts")?,
                conversation: string_field(object, "conversation")?,
            }),
            "conversation.bound" => Some(Self::ConversationBound {
                ts: string_field(object, "ts")?,
                conversation: string_field(object, "conversation")?,
                binding_id: string_field(object, "binding_id")?,
            }),
            "prompt.snapshot" => Some(Self::PromptSnapshot {
                ts: string_field(object, "ts")?,
                conversation: string_field(object, "conversation")?,
                binding_id: string_field(object, "binding_id")?,
                snapshot_id: string_field(object, "snapshot_id")?,
                turn: u64_field(object, "turn")?,
                messages: serde_json::from_value(object.get("messages")?.clone()).ok()?,
                project_instruction_sources: serde_json::from_value(
                    object.get("project_instruction_sources")?.clone(),
                )
                .ok()?,
                memory_sources: serde_json::from_value(object.get("memory_sources")?.clone())
                    .ok()?,
                prompt_asset_sources: serde_json::from_value(
                    object.get("prompt_asset_sources")?.clone(),
                )
                .ok()?,
                skills: object.get("skills")?.clone(),
                bootstrap_loaded: serde_json::from_value(object.get("bootstrap_loaded")?.clone())
                    .ok()?,
                provider: string_field(object, "provider")?,
                model: string_field(object, "model")?,
                renderer: serde_json::from_value(object.get("renderer")?.clone()).ok()?,
                tool_registry: Box::new(
                    serde_json::from_value(object.get("tool_registry")?.clone()).ok()?,
                ),
                agent_registry: optional_json_field(object, "agent_registry")
                    .and_then(|value| serde_json::from_value(value).ok()),
                skill_registry: Box::new(
                    optional_json_field(object, "skill_registry")
                        .and_then(|value| serde_json::from_value(value).ok()),
                ),
                plugin_registry: Box::new(
                    optional_json_field(object, "plugin_registry")
                        .and_then(|value| serde_json::from_value(value).ok()),
                ),
                capabilities: serde_json::from_value(object.get("capabilities")?.clone()).ok()?,
            }),
            "message.user" => Some(Self::MessageUser {
                execution: serde_json::from_value(object.get("execution")?.clone()).ok()?,
                ts: string_field(object, "ts")?,
                conversation: string_field(object, "conversation")?,
                turn: u64_field(object, "turn")?,
                text: string_field(object, "text")?,
                from: optional_string_field(object, "from"),
                via_tool_call_id: optional_string_field(object, "via_tool_call_id"),
            }),
            "message.assistant" => Some(Self::MessageAssistant {
                execution: serde_json::from_value(object.get("execution")?.clone()).ok()?,
                ts: string_field(object, "ts")?,
                conversation: string_field(object, "conversation")?,
                turn: u64_field(object, "turn")?,
                message_id: string_field(object, "message_id")?,
                text: string_field(object, "text")?,
            }),
            "turn.started" => Some(Self::TurnStarted {
                execution: serde_json::from_value(object.get("execution")?.clone()).ok()?,
                ts: string_field(object, "ts")?,
                conversation: string_field(object, "conversation")?,
                turn: u64_field(object, "turn")?,
            }),
            "turn.completed" => Some(Self::TurnCompleted {
                execution: serde_json::from_value(object.get("execution")?.clone()).ok()?,
                ts: string_field(object, "ts")?,
                conversation: string_field(object, "conversation")?,
                turn: u64_field(object, "turn")?,
            }),
            "turn.cancelled" => Some(Self::TurnCancelled {
                execution: serde_json::from_value(object.get("execution")?.clone()).ok()?,
                ts: string_field(object, "ts")?,
                conversation: string_field(object, "conversation")?,
                turn: u64_field(object, "turn")?,
                reason: string_field(object, "reason")?,
            }),
            "turn.interrupted" => Some(Self::TurnInterrupted {
                execution: serde_json::from_value(object.get("execution")?.clone()).ok()?,
                ts: string_field(object, "ts")?,
                conversation: string_field(object, "conversation")?,
                turn: u64_field(object, "turn")?,
                reason: string_field(object, "reason")?,
            }),
            "conversation.rollback" => Some(Self::ConversationRollback {
                ts: string_field(object, "ts")?,
                conversation: string_field(object, "conversation")?,
                to_turn: u64_field(object, "to_turn")?,
                to_event_id: u64_field(object, "to_event_id")?,
                scope: serde_json::from_value(object.get("scope")?.clone()).ok()?,
            }),
            "conversation.rollback.undone" => Some(Self::ConversationRollbackUndone {
                ts: string_field(object, "ts")?,
                conversation: string_field(object, "conversation")?,
                rollback_event_id: u64_field(object, "rollback_event_id")?,
            }),
            _ => Some(Self::Unknown(value)),
        }
    }

    pub(super) fn to_new_json(&self, id: u64) -> serde_json::Result<Value> {
        match self {
            Self::TaskLedger(record) => {
                let encoded = serde_json::to_value(record)?;
                Ok(serde_json::json!({
                    "id": id,
                    "kind": "task.ledger",
                    "record_type": encoded.get("record_type"),
                    "record": encoded.get("record"),
                }))
            }
            Self::Unknown(value) => Ok(value.clone()),
            Self::SessionCreated {
                ts,
                schema_version,
                session_id,
                created_at,
                kuku_version,
            } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "session.created",
                "schema_version": schema_version,
                "session_id": session_id,
                "created_at": created_at,
                "kuku_version": kuku_version,
            })),
            Self::ContextSources {
                request,
                turn,
                ts,
                project_instruction_sources,
                memory_sources,
            } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "context.sources",
                "turn": turn,
                "request": request,
                "project_instruction_sources": project_instruction_sources,
                "memory_sources": memory_sources,
            })),
            Self::ContextSkills {
                conversation,
                turn,
                ts,
                registry,
                bootstrap_loaded,
            } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "context.skills",
                "conversation": conversation,
                "turn": turn,
                "registry": registry,
                "bootstrap_loaded": bootstrap_loaded,
            })),
            Self::TurnStarted {
                execution,
                turn,
                ts,
                conversation,
            } => {
                let mut map = Map::new();
                map.insert("id".into(), Value::from(id));
                map.insert("ts".into(), Value::from(ts.clone()));
                map.insert("kind".into(), Value::from("turn.started"));
                map.insert("turn".into(), Value::from(*turn));
                map.insert("conversation".into(), Value::from(conversation.clone()));
                map.insert("execution".into(), serde_json::to_value(execution)?);
                Ok(Value::Object(map))
            }
            Self::MessageUser {
                execution,
                turn,
                ts,
                text,
                conversation,
                from,
                via_tool_call_id,
            } => {
                let mut map = Map::new();
                map.insert("id".into(), Value::from(id));
                map.insert("ts".into(), Value::from(ts.clone()));
                map.insert("kind".into(), Value::from("message.user"));
                map.insert("turn".into(), Value::from(*turn));
                map.insert("text".into(), Value::from(text.clone()));
                map.insert("conversation".into(), Value::from(conversation.clone()));
                map.insert("execution".into(), serde_json::to_value(execution)?);
                if let Some(from) = from {
                    map.insert("from".into(), Value::from(from.clone()));
                }
                if let Some(via_tool_call_id) = via_tool_call_id {
                    map.insert(
                        "via_tool_call_id".into(),
                        Value::from(via_tool_call_id.clone()),
                    );
                }
                Ok(Value::Object(map))
            }
            Self::ModelResponse {
                request,
                turn,
                ts,
                text,
                thinking,
                input_tokens_total,
            } => {
                let mut map = Map::new();
                map.insert("id".into(), Value::from(id));
                map.insert("ts".into(), Value::from(ts.clone()));
                map.insert("kind".into(), Value::from("model.response"));
                map.insert("turn".into(), Value::from(*turn));
                map.insert("request".into(), serde_json::to_value(request)?);
                map.insert("text".into(), Value::from(text.clone()));
                if let Some(thinking) = thinking {
                    map.insert("thinking".into(), Value::from(thinking.clone()));
                }
                if let Some(input_tokens_total) = input_tokens_total {
                    map.insert(
                        "input_tokens_total".into(),
                        Value::from(*input_tokens_total),
                    );
                }
                Ok(Value::Object(map))
            }
            Self::ModelError {
                request,
                turn,
                ts,
                kind,
                message,
            } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "model.error",
                "turn": turn,
                "request": request,
                "error_kind": kind,
                "message": message,
            })),
            Self::ToolCall {
                request,
                turn,
                ts,
                conversation,
                tool_call_id,
                index,
                tool,
                args,
            } => {
                let mut map = Map::new();
                map.insert("id".into(), Value::from(id));
                map.insert("ts".into(), Value::from(ts.clone()));
                map.insert("kind".into(), Value::from("tool.call"));
                map.insert("turn".into(), Value::from(*turn));
                map.insert("tool_call_id".into(), Value::from(tool_call_id.clone()));
                map.insert("request".into(), serde_json::to_value(request)?);
                map.insert("index".into(), Value::from(*index));
                map.insert("tool".into(), Value::from(tool.clone()));
                map.insert("args".into(), args.clone());
                if let Some(conversation) = conversation.as_ref() {
                    map.insert("conversation".into(), Value::from(conversation.clone()));
                }
                Ok(Value::Object(map))
            }
            Self::PermissionAllow {
                execution,
                turn,
                ts,
                tool_call_id,
                tool,
                scope,
                matcher,
                source,
            } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "permission.allow",
                "turn": turn,
                "tool_call_id": tool_call_id,
                "tool": tool,
                "scope": scope,
                "matcher": matcher,
                "source": source,
                "execution": execution,
            })),
            Self::PermissionRequested {
                execution,
                turn,
                ts,
                tool_call_id,
                tool,
                risk,
                summary,
                candidate,
                source,
            } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "permission.requested",
                "turn": turn,
                "tool_call_id": tool_call_id,
                "tool": tool,
                "risk": risk,
                "summary": summary,
                "candidate": candidate,
                "source": source,
                "execution": execution,
            })),
            Self::PermissionDeny {
                execution,
                turn,
                ts,
                tool_call_id,
                tool,
                reason,
                source,
            } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "permission.deny",
                "turn": turn,
                "tool_call_id": tool_call_id,
                "tool": tool,
                "reason": reason,
                "source": source,
                "execution": execution,
            })),
            Self::ToolResult {
                execution,
                turn,
                ts,
                conversation,
                tool_call_id,
                status,
                summary,
                model_content,
                truncated,
                files_read,
                files_changed,
                commands_run,
                memory_changed,
                structured,
            } => {
                let mut map = Map::new();
                map.insert("id".into(), Value::from(id));
                map.insert("ts".into(), Value::from(ts.clone()));
                map.insert("kind".into(), Value::from("tool.result"));
                map.insert("turn".into(), Value::from(*turn));
                map.insert("execution".into(), serde_json::to_value(execution)?);
                map.insert("tool_call_id".into(), Value::from(tool_call_id.clone()));
                map.insert("status".into(), Value::from(status.clone()));
                map.insert("summary".into(), Value::from(summary.clone()));
                map.insert("model_content".into(), Value::from(model_content.clone()));
                map.insert("truncated".into(), Value::from(*truncated));
                if let Some(conversation) = conversation.as_ref() {
                    map.insert("conversation".into(), Value::from(conversation.clone()));
                }
                if !files_read.is_empty() {
                    map.insert("files_read".into(), serde_json::to_value(files_read)?);
                }
                if !files_changed.is_empty() {
                    map.insert("files_changed".into(), serde_json::to_value(files_changed)?);
                }
                if !commands_run.is_empty() {
                    map.insert("commands_run".into(), serde_json::to_value(commands_run)?);
                }
                if let Some(memory_changed) = memory_changed {
                    map.insert("memory_changed".into(), memory_changed.clone());
                }
                if let Some(structured) = structured {
                    map.insert("structured".into(), structured.clone());
                }
                Ok(Value::Object(map))
            }
            Self::Handoff {
                execution,
                turn,
                ts,
                request_id,
                summary,
                keep_turns,
            } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "handoff",
                "turn": turn,
                "request_id": request_id,
                "summary": summary,
                "keep_turns": keep_turns,
                "execution": execution,
            })),
            Self::TurnCompleted {
                execution,
                turn,
                ts,
                conversation,
            } => {
                let mut map = Map::new();
                map.insert("id".into(), Value::from(id));
                map.insert("ts".into(), Value::from(ts.clone()));
                map.insert("kind".into(), Value::from("turn.completed"));
                map.insert("turn".into(), Value::from(*turn));
                map.insert("conversation".into(), Value::from(conversation.clone()));
                map.insert("execution".into(), serde_json::to_value(execution)?);
                Ok(Value::Object(map))
            }
            Self::ConversationOpened { ts, conversation } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "conversation.opened",
                "conversation": conversation,
            })),
            Self::ConversationBound {
                ts,
                conversation,
                binding_id,
            } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "conversation.bound",
                "conversation": conversation,
                "binding_id": binding_id,
            })),
            Self::PromptSnapshot {
                ts,
                conversation,
                binding_id,
                snapshot_id,
                turn,
                messages,
                project_instruction_sources,
                memory_sources,
                prompt_asset_sources,
                skills,
                bootstrap_loaded,
                provider,
                model,
                renderer,
                tool_registry,
                agent_registry,
                skill_registry,
                plugin_registry,
                capabilities,
            } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "prompt.snapshot",
                "conversation": conversation,
                "binding_id": binding_id,
                "snapshot_id": snapshot_id,
                "turn": turn,
                "messages": messages,
                "project_instruction_sources": project_instruction_sources,
                "memory_sources": memory_sources,
                "prompt_asset_sources": prompt_asset_sources,
                "skills": skills,
                "bootstrap_loaded": bootstrap_loaded,
                "provider": provider,
                "model": model,
                "renderer": renderer,
                "tool_registry": tool_registry,
                "agent_registry": agent_registry,
            "skill_registry": skill_registry,
                "plugin_registry": plugin_registry,
                "capabilities": capabilities,
            })),
            Self::MessageAssistant {
                execution,
                ts,
                conversation,
                turn,
                message_id,
                text,
            } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "message.assistant",
                "conversation": conversation,
                "turn": turn,
                "message_id": message_id,
                "text": text,
                "execution": execution,
            })),
            Self::TurnCancelled {
                execution,
                ts,
                conversation,
                turn,
                reason,
            } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "turn.cancelled",
                "conversation": conversation,
                "turn": turn,
                "reason": reason,
                "execution": execution,
            })),
            Self::TurnInterrupted {
                execution,
                ts,
                conversation,
                turn,
                reason,
            } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "turn.interrupted",
                "conversation": conversation,
                "turn": turn,
                "reason": reason,
                "execution": execution,
            })),
            Self::ConversationRollback {
                ts,
                conversation,
                to_turn,
                to_event_id,
                scope,
            } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "conversation.rollback",
                "conversation": conversation,
                "to_turn": to_turn,
                "to_event_id": to_event_id,
                "scope": scope,
            })),
            Self::ConversationRollbackUndone {
                ts,
                conversation,
                rollback_event_id,
            } => Ok(serde_json::json!({
                "id": id,
                "ts": ts,
                "kind": "conversation.rollback.undone",
                "conversation": conversation,
                "rollback_event_id": rollback_event_id,
            })),
        }
    }
}

fn string_field(object: &Map<String, Value>, key: &str) -> Option<String> {
    object.get(key)?.as_str().map(ToOwned::to_owned)
}

fn optional_string_field(object: &Map<String, Value>, key: &str) -> Option<String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn optional_json_field(object: &Map<String, Value>, key: &str) -> Option<Value> {
    object.get(key).cloned().filter(|value| !value.is_null())
}

fn u64_field(object: &Map<String, Value>, key: &str) -> Option<u64> {
    object.get(key)?.as_u64()
}

fn usize_field(object: &Map<String, Value>, key: &str) -> Option<usize> {
    usize::try_from(object.get(key)?.as_u64()?).ok()
}

fn u32_field(object: &Map<String, Value>, key: &str) -> Option<u32> {
    u32::try_from(object.get(key)?.as_u64()?).ok()
}

fn optional_u32_field(object: &Map<String, Value>, key: &str) -> Option<u32> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
}

fn bool_field(object: &Map<String, Value>, key: &str) -> Option<bool> {
    object.get(key)?.as_bool()
}

fn vec_string_field(object: &Map<String, Value>, key: &str) -> Vec<String> {
    object
        .get(key)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}
