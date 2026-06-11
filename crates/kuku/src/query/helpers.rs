use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::context::{InstructionSource, MemorySource};
use crate::conversation::address::ConversationAddress;
use crate::error::Result;
use crate::event::{EventPayload, EventStore};
use crate::provider::types::ProviderKind;
use crate::session::{global_memory_path, project_memory_path};

use super::types::{PendingRun, PermissionChoice, PermissionRequest};

pub(super) fn is_inline_skill_tool(name: &str) -> bool {
    matches!(name, "list_skills" | "search_skills" | "use_skill")
}

pub(super) fn resolved_tool_available(pending: &PendingRun, name: &str) -> bool {
    if is_inline_skill_tool(name) && pending.query.disable_skills {
        return false;
    }
    if let Some(resolved) = pending.resolved.as_ref() {
        return resolved.registry.iter().any(|tool| tool.name == name);
    }
    if let Some(registry) = pending.tool_registry_override.as_ref() {
        return registry.iter().any(|tool| tool.name == name);
    }
    crate::tool::builtin_registry(!pending.query.disable_agents, !pending.query.disable_skills)
        .iter()
        .any(|tool| tool.name == name)
}

// ---------- Permission helpers ----------

pub(super) fn permission_rule(
    kuku_home: &std::path::Path,
    workspace: &std::path::Path,
    name: &str,
    args: &serde_json::Value,
) -> String {
    format!(
        "{}({})",
        name,
        permission_candidate(kuku_home, workspace, name, args)
    )
}

pub(super) fn permission_candidate(
    kuku_home: &std::path::Path,
    workspace: &std::path::Path,
    name: &str,
    args: &serde_json::Value,
) -> String {
    match name {
        "run_command" => args
            .get("command")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string(),
        "remember_memory" | "forget_memory" => match args
            .get("scope")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
        {
            Some("global") => global_memory_path(kuku_home).display().to_string(),
            Some("project") => project_memory_path(kuku_home, workspace)
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            _ => String::new(),
        },
        _ => args
            .get("path")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string(),
    }
}

pub(super) fn display_summary(
    tool: &str,
    args: &serde_json::Value,
    max_len: Option<usize>,
) -> String {
    let raw = match tool {
        "find_files" => {
            let path = args.get("path").and_then(|v| v.as_str());
            let pattern = args.get("pattern").and_then(|v| v.as_str());
            match (path, pattern) {
                (Some(p), Some(pat)) => format!("path: {:?}, pattern: {:?}", p, pat),
                (Some(p), None) => format!("path: {:?}", p),
                (None, Some(pat)) => format!("path: \"\", pattern: {:?}", pat),
                (None, None) => return tool.to_string(),
            }
        }
        "read_file" => args
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(tool)
            .to_string(),
        "edit_file" | "write_file" => args
            .get("brief")
            .and_then(|v| v.as_str())
            .unwrap_or(tool)
            .to_string(),
        "search_text" => {
            let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            let path = args.get("path").and_then(|v| v.as_str());
            match path {
                Some(p) => format!("{:?} in {}", pattern, p),
                None => format!("{:?}", pattern),
            }
        }
        "run_command" => args
            .get("brief")
            .and_then(|v| v.as_str())
            .unwrap_or(tool)
            .to_string(),
        "agent" => args
            .get("to")
            .and_then(|v| v.as_str())
            .unwrap_or("agent")
            .to_string(),
        "use_skill" => args
            .get("skill_name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string(),
        "list_skills" => {
            let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0);
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20);
            format!("offset: {offset}, limit: {limit}")
        }
        "search_skills" => args
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("search skills")
            .to_string(),
        "remember_memory" | "forget_memory" => args
            .get("text")
            .and_then(|v| v.as_str())
            .map(|t| format!("{:?}", t))
            .unwrap_or_else(|| tool.to_string()),
        _ => tool.to_string(),
    };
    match max_len {
        Some(max) if raw.chars().count() > max => {
            let truncated: String = raw.chars().take(max).collect();
            format!("{}...", truncated)
        }
        _ => raw,
    }
}

pub(super) fn permission_scope(choice: PermissionChoice) -> &'static str {
    match choice {
        PermissionChoice::Once | PermissionChoice::Deny => "once",
        PermissionChoice::Session => "session",
        PermissionChoice::Project => "project",
    }
}

pub(super) fn gate_source_name(source: crate::permission::GateSource) -> &'static str {
    match source {
        crate::permission::GateSource::HardGuard => "hard_guard",
        crate::permission::GateSource::ProjectPolicy => "project_policy",
        crate::permission::GateSource::SessionGrant => "session_grant",
        crate::permission::GateSource::TrustPosture => "trust_posture",
        crate::permission::GateSource::Host => "host",
        crate::permission::GateSource::DefaultAsk => "default_ask",
    }
}

pub(super) fn gate_choice(source: &crate::permission::GateSource) -> PermissionChoice {
    use crate::permission::GateSource;
    match source {
        GateSource::ProjectPolicy => PermissionChoice::Project,
        GateSource::SessionGrant => PermissionChoice::Session,
        _ => PermissionChoice::Once,
    }
}

// ---------- Event append helpers ----------

fn append_event(events_path: &std::path::Path, payload: EventPayload) -> Result<()> {
    let mut store = EventStore::open(events_path)?;
    store.append(payload)?;
    Ok(())
}

pub(super) fn append_permission_request(
    events_path: &std::path::Path,
    _conversation: &ConversationAddress,
    turn: u64,
    request: &PermissionRequest,
) -> Result<()> {
    append_event(
        events_path,
        EventPayload::PermissionRequested {
            turn,
            ts: now_timestamp()?,
            tool_call_id: request.tool_call_id.clone(),
            tool: request.tool.clone(),
            risk: request.risk.clone(),
            summary: request.summary.clone(),
            candidate: request.candidate.clone(),
            source: request.source.clone(),
        },
    )
}

pub(super) fn append_turn_started(
    events_path: &std::path::Path,
    conversation: &ConversationAddress,
    turn: u64,
) -> Result<()> {
    append_event(
        events_path,
        EventPayload::TurnStarted {
            ts: now_timestamp()?,
            conversation: conversation.as_str().to_string(),
            turn,
        },
    )
}

pub(super) fn append_message_user_with_sender(
    events_path: &std::path::Path,
    conversation: &ConversationAddress,
    turn: u64,
    text: &str,
    from: Option<&ConversationAddress>,
    via_tool_call_id: Option<&str>,
) -> Result<()> {
    append_event(
        events_path,
        EventPayload::MessageUser {
            turn,
            ts: now_timestamp()?,
            conversation: conversation.as_str().to_string(),
            text: text.to_string(),
            from: from.map(|address| address.as_str().to_string()),
            via_tool_call_id: via_tool_call_id.map(ToOwned::to_owned),
        },
    )
}

pub(super) fn append_permission_decision(
    events_path: &std::path::Path,
    turn: u64,
    tool_call_id: &str,
    choice: PermissionChoice,
    source: &str,
    rule: &str,
) -> Result<()> {
    let payload = match choice {
        PermissionChoice::Deny => EventPayload::PermissionDeny {
            turn,
            ts: now_timestamp()?,
            tool_call_id: tool_call_id.to_string(),
            tool: rule.split('(').next().unwrap_or("").trim().to_string(),
            reason: rule.to_string(),
            source: source.to_string(),
        },
        PermissionChoice::Once | PermissionChoice::Session | PermissionChoice::Project => {
            EventPayload::PermissionAllow {
                turn,
                ts: now_timestamp()?,
                tool_call_id: tool_call_id.to_string(),
                tool: rule.split('(').next().unwrap_or("").trim().to_string(),
                scope: permission_scope(choice).to_string(),
                matcher: rule.to_string(),
                source: source.to_string(),
            }
        }
    };
    append_event(events_path, payload)
}

pub(super) fn append_model_error(
    events_path: &std::path::Path,
    turn: u64,
    request_id: String,
    kind: &str,
    message: &str,
) -> Result<()> {
    append_event(
        events_path,
        EventPayload::ModelError {
            turn,
            ts: now_timestamp()?,
            request_id,
            kind: kind.to_string(),
            message: message.to_string(),
        },
    )
}

fn has_terminal_event(
    events_path: &std::path::Path,
    conversation: &ConversationAddress,
    turn: u64,
) -> Result<bool> {
    Ok(EventStore::replay(events_path)?
        .iter()
        .any(|event| match &event.payload {
            EventPayload::TurnCompleted {
                conversation: event_conversation,
                turn: event_turn,
                ..
            }
            | EventPayload::TurnCancelled {
                conversation: event_conversation,
                turn: event_turn,
                ..
            }
            | EventPayload::TurnInterrupted {
                conversation: event_conversation,
                turn: event_turn,
                ..
            } => *event_turn == turn && event_conversation == conversation.as_str(),
            _ => false,
        }))
}

pub(super) fn append_turn_completed(
    events_path: &std::path::Path,
    conversation: &ConversationAddress,
    turn: u64,
) -> Result<()> {
    if has_terminal_event(events_path, conversation, turn)? {
        return Ok(());
    }
    append_event(
        events_path,
        EventPayload::TurnCompleted {
            ts: now_timestamp()?,
            conversation: conversation.as_str().to_string(),
            turn,
        },
    )
}

pub(super) fn append_turn_cancelled(
    events_path: &std::path::Path,
    conversation: &ConversationAddress,
    turn: u64,
    reason: &str,
) -> Result<()> {
    if has_terminal_event(events_path, conversation, turn)? {
        return Ok(());
    }
    append_event(
        events_path,
        EventPayload::TurnCancelled {
            ts: now_timestamp()?,
            conversation: conversation.as_str().to_string(),
            turn,
            reason: reason.to_string(),
        },
    )
}

pub(super) fn append_turn_interrupted(
    events_path: &std::path::Path,
    conversation: &ConversationAddress,
    turn: u64,
    reason: &str,
) -> Result<()> {
    if has_terminal_event(events_path, conversation, turn)? {
        return Ok(());
    }
    append_event(
        events_path,
        EventPayload::TurnInterrupted {
            ts: now_timestamp()?,
            conversation: conversation.as_str().to_string(),
            turn,
            reason: reason.to_string(),
        },
    )
}

pub(super) fn append_interrupted_active_turn(
    events_path: &std::path::Path,
    events: &[crate::event::StoredEvent],
    conversation: &ConversationAddress,
    reason: &str,
) -> Result<()> {
    if let Some(active_turn) = crate::conversation::active_turn(events, conversation) {
        append_turn_interrupted(events_path, conversation, active_turn.turn, reason)?;
    }
    Ok(())
}

// ---------- Env / utility helpers ----------

pub(super) fn validate_existing_session(events: &[crate::event::StoredEvent]) -> Result<()> {
    if events.is_empty() {
        return Ok(());
    }

    match events.first().map(|event| &event.payload) {
        Some(EventPayload::SessionCreated { .. }) => Ok(()),
        _ => Err(crate::error::Error::InvalidEventStream(
            "first event must be session.created".to_string(),
        )),
    }
}

pub(super) fn next_turn(events: &[crate::event::StoredEvent]) -> u64 {
    events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::TurnStarted { turn, .. } => Some(*turn),
            _ => None,
        })
        .max()
        .unwrap_or(0)
        + 1
}

pub(super) fn platform_label() -> &'static str {
    std::env::consts::OS
}

pub(super) fn current_date_string() -> String {
    OffsetDateTime::now_utc().date().to_string()
}

pub(super) fn now_timestamp() -> Result<String> {
    Ok(OffsetDateTime::now_utc().format(&Rfc3339)?)
}

pub(super) fn load_project_instruction_sources(
    workspace: &std::path::Path,
) -> Result<Vec<InstructionSource>> {
    let mut sources = Vec::new();
    for (name, kind) in [("AGENTS.md", "agents"), ("CLAUDE.md", "claude")] {
        let path = workspace.join(name);
        if let Ok(content) = std::fs::read_to_string(&path) {
            sources.push(InstructionSource {
                path: path.display().to_string(),
                kind: kind.to_string(),
                hash: crate::tool::builtin::common::content_hash(content.as_bytes()),
                content,
            });
        }
    }
    Ok(sources)
}

pub(super) fn load_memory_sources(
    kuku_home: &std::path::Path,
    workspace: &std::path::Path,
) -> Result<(Option<MemorySource>, Option<MemorySource>)> {
    let global_memory = std::fs::read_to_string(global_memory_path(kuku_home))
        .ok()
        .map(|content| MemorySource {
            path: global_memory_path(kuku_home).display().to_string(),
            hash: crate::tool::builtin::common::content_hash(content.as_bytes()),
            content,
        });

    let project_path = project_memory_path(kuku_home, workspace)?;
    let project_memory = std::fs::read_to_string(&project_path)
        .ok()
        .map(|content| MemorySource {
            path: project_path.display().to_string(),
            hash: crate::tool::builtin::common::content_hash(content.as_bytes()),
            content,
        });

    Ok((global_memory, project_memory))
}

pub(super) fn last_input_tokens(
    kind: &ProviderKind,
    events: &[crate::event::StoredEvent],
) -> Option<u32> {
    let _ = kind;
    events.iter().rev().find_map(|event| match &event.payload {
        EventPayload::ModelResponse {
            input_tokens_total, ..
        } => *input_tokens_total,
        _ => None,
    })
}

#[cfg(test)]
pub(super) fn extract_input_tokens(kind: &ProviderKind, usage: &serde_json::Value) -> Option<u32> {
    let total = match kind {
        ProviderKind::Anthropic | ProviderKind::OpenAiResponses => {
            let input = usage
                .get("input_tokens")
                .and_then(serde_json::Value::as_u64)?;
            let cache_read = usage
                .get("cache_read_input_tokens")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let cache_creation = usage
                .get("cache_creation_input_tokens")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            input + cache_read + cache_creation
        }
        ProviderKind::OpenAiCompatible => usage
            .get("prompt_tokens")
            .and_then(serde_json::Value::as_u64)?,
    };
    u32::try_from(total).ok().filter(|&v| v > 0)
}

#[cfg(test)]
mod tests {
    use super::extract_input_tokens;
    use crate::provider::types::ProviderKind;

    #[test]
    fn extract_input_tokens_sums_anthropic_cache_fields() {
        let usage = serde_json::json!({
            "input_tokens": 845,
            "output_tokens": 106,
            "cache_read_input_tokens": 181888,
            "cache_creation_input_tokens": 0,
        });
        let result = extract_input_tokens(&ProviderKind::Anthropic, &usage);
        assert_eq!(result, Some(845 + 181888));
    }

    #[test]
    fn extract_input_tokens_uses_openai_prompt_tokens() {
        let usage = serde_json::json!({
            "prompt_tokens": 500,
            "completion_tokens": 100,
            "total_tokens": 600,
        });
        let result = extract_input_tokens(&ProviderKind::OpenAiCompatible, &usage);
        assert_eq!(result, Some(500));
    }

    #[test]
    fn extract_input_tokens_returns_none_for_empty_usage() {
        let usage = serde_json::json!({});
        assert_eq!(extract_input_tokens(&ProviderKind::Anthropic, &usage), None);
        assert_eq!(
            extract_input_tokens(&ProviderKind::OpenAiCompatible, &usage),
            None
        );
    }

    use super::display_summary;

    #[test]
    fn display_summary_find_files_with_path() {
        let args = serde_json::json!({"path": "."});
        let s = display_summary("find_files", &args, None);
        assert_eq!(s, "path: \".\"");
    }

    #[test]
    fn display_summary_find_files_with_pattern() {
        let args = serde_json::json!({"path": ".", "pattern": "*.rs"});
        let s = display_summary("find_files", &args, None);
        assert_eq!(s, "path: \".\", pattern: \"*.rs\"");
    }

    #[test]
    fn display_summary_read_file() {
        let args = serde_json::json!({"path": "src/main.rs"});
        let s = display_summary("read_file", &args, None);
        assert_eq!(s, "src/main.rs");
    }

    #[test]
    fn display_summary_search_text() {
        let args = serde_json::json!({"pattern": "fn query", "path": "crates/"});
        let s = display_summary("search_text", &args, None);
        assert_eq!(s, "\"fn query\" in crates/");
    }

    #[test]
    fn display_summary_run_command() {
        let args = serde_json::json!({"command": "cargo build --release", "timeout": 60, "brief": "build release"});
        let s = display_summary("run_command", &args, None);
        assert_eq!(s, "build release");
    }

    #[test]
    fn display_summary_run_command_truncated() {
        let args = serde_json::json!({"command": "echo hi", "timeout": 60, "brief": "cargo build --release --features=full,test --target=x86_64-unknown-linux-gnu"});
        let s = display_summary("run_command", &args, Some(30));
        assert!(s.ends_with("..."), "should end with ...");
        let cjk_args = serde_json::json!({"command": "echo hi", "timeout": 60, "brief": "构建项目/src/文件.txt"});
        let s2 = display_summary("run_command", &cjk_args, Some(5));
        assert!(s2.ends_with("..."), "CJK truncation should not panic: {s2}");
    }

    #[test]
    fn display_summary_remember_memory() {
        let args =
            serde_json::json!({"scope": "global", "kind": "what_is_true", "text": "kuku is Rust"});
        let s = display_summary("remember_memory", &args, None);
        assert_eq!(s, "\"kuku is Rust\"");
    }

    #[test]
    fn display_summary_forget_memory() {
        let args = serde_json::json!({"scope": "project", "text": "kuku is Rust"});
        let s = display_summary("forget_memory", &args, None);
        assert_eq!(s, "\"kuku is Rust\"");
    }

    #[test]
    fn display_summary_edit_file() {
        let args = serde_json::json!({"path": "src/main.rs", "old_text": "old", "new_text": "new", "brief": "rename main"});
        let s = display_summary("edit_file", &args, None);
        assert_eq!(s, "rename main");
    }

    #[test]
    fn display_summary_write_file() {
        let args = serde_json::json!({"path": "src/lib.rs", "content": "fn main() {}", "brief": "create lib"});
        let s = display_summary("write_file", &args, None);
        assert_eq!(s, "create lib");
    }

    #[test]
    fn display_summary_unknown_tool() {
        let args = serde_json::json!({"foo": "bar"});
        let s = display_summary("unknown_tool", &args, None);
        assert_eq!(s, "unknown_tool");
    }

    #[test]
    fn display_summary_empty_args() {
        let args = serde_json::json!({});
        let s = display_summary("find_files", &args, None);
        assert_eq!(s, "find_files");
    }

    #[test]
    fn display_summary_find_files_with_only_pattern() {
        let args = serde_json::json!({"pattern": "*.rs"});
        let s = display_summary("find_files", &args, None);
        assert_eq!(
            s, "path: \"\", pattern: \"*.rs\"",
            "missing path defaults to empty"
        );
    }
}
