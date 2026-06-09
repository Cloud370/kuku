use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sha2::Digest;

use crate::agent::catalog::render_agent_directory;
use crate::agent::registry::AgentRegistry;
use crate::conversation::address::ConversationAddress;
use crate::conversation::reducer::{conversation_events, reduce_conversations, TurnTerminal};
use crate::event::{EventPayload, StoredEvent};
use crate::skill::session::loaded_skill_names;

use super::types::{
    ContextBudgetTier, ContextDriftEntry, ContextDriftStatus, ConversationInboxMessage, Notice,
    NoticeKind, NoticeSeverity, OpenConversationEntry,
};

pub(crate) struct NoticeAssemblyInput<'a> {
    pub(crate) workspace: &'a Path,
    pub(crate) events: &'a [StoredEvent],
    pub(crate) context_budget_tier: ContextBudgetTier,
    pub(crate) conversation: &'a ConversationAddress,
    pub(crate) agent_registry: Option<&'a AgentRegistry>,
}

struct TrackedFileSnapshot {
    path: String,
    hash: String,
}

pub(crate) fn build_runtime_notices(input: NoticeAssemblyInput<'_>) -> Vec<Notice> {
    let mut notices = Vec::new();

    if input.conversation.is_main() {
        if let Some(notice) = build_agent_directory_notice(input.agent_registry, input.events) {
            notices.push(notice);
        }
        if let Some(notice) = build_open_conversations_notice(input.events, input.conversation) {
            notices.push(notice);
        }
    }

    if let Some(notice) = build_inbox_notice(input.events, input.conversation) {
        notices.push(notice);
    }

    if let Some(notice) = build_loaded_skills_notice(input.events, input.conversation) {
        notices.push(notice);
    }

    if let Some(notice) = build_pending_permission_notice(input.events, input.conversation) {
        notices.push(notice);
    }

    if let Some(notice) = build_interrupted_turn_notice(input.events, input.conversation) {
        notices.push(notice);
    }

    if let Some(notice) =
        build_context_drift_notice(input.workspace, input.events, input.context_budget_tier)
    {
        notices.push(notice);
    }

    notices
}

fn build_agent_directory_notice(
    registry: Option<&AgentRegistry>,
    events: &[StoredEvent],
) -> Option<Notice> {
    let registry = registry?;
    let open_conversations = reduce_conversations(events)
        .into_iter()
        .filter(|state| !state.address.is_main())
        .count();
    let directory = render_agent_directory(registry, open_conversations)?;
    Some(Notice {
        kind: NoticeKind::AgentDirectory { directory },
        severity: NoticeSeverity::Info,
    })
}

fn build_open_conversations_notice(
    events: &[StoredEvent],
    target: &ConversationAddress,
) -> Option<Notice> {
    let entries = reduce_conversations(events)
        .into_iter()
        .filter(|state| !state.address.is_main() && &state.address != target)
        .map(|state| OpenConversationEntry {
            conversation: state.address,
            summary: match state.last_terminal {
                Some((turn, TurnTerminal::Completed)) => format!("turn {turn} completed"),
                Some((turn, TurnTerminal::Cancelled)) => format!("turn {turn} cancelled"),
                Some((turn, TurnTerminal::Interrupted)) => format!("turn {turn} interrupted"),
                None if state.active_turn.is_some() => "turn in progress".to_string(),
                None => "opened".to_string(),
            },
        })
        .collect::<Vec<_>>();

    if entries.is_empty() {
        None
    } else {
        Some(Notice {
            kind: NoticeKind::OpenConversations { entries },
            severity: NoticeSeverity::Info,
        })
    }
}

fn build_inbox_notice(events: &[StoredEvent], target: &ConversationAddress) -> Option<Notice> {
    let messages = conversation_events(events, target)
        .into_iter()
        .filter_map(|event| match &event.payload {
            EventPayload::MessageUser {
                conversation,
                text,
                from,
                ..
            } if conversation == target.as_str() => Some(ConversationInboxMessage {
                from: from
                    .as_deref()
                    .and_then(|value| ConversationAddress::parse(value).ok()),
                text: text.clone(),
            }),
            _ => None,
        })
        .collect::<Vec<_>>();

    if messages.is_empty() {
        None
    } else {
        Some(Notice {
            kind: NoticeKind::ConversationInbox { messages },
            severity: NoticeSeverity::Info,
        })
    }
}

fn build_loaded_skills_notice(
    events: &[StoredEvent],
    target: &ConversationAddress,
) -> Option<Notice> {
    let skills = loaded_skill_names(events, target.as_str());
    if skills.is_empty() {
        None
    } else {
        Some(Notice {
            kind: NoticeKind::LoadedSkills { skills },
            severity: NoticeSeverity::Info,
        })
    }
}

fn build_pending_permission_notice(
    events: &[StoredEvent],
    target: &ConversationAddress,
) -> Option<Notice> {
    let request = pending_permission_for_conversation(events, target)?;
    Some(Notice {
        kind: NoticeKind::PendingPermission { request },
        severity: NoticeSeverity::Info,
    })
}

fn build_interrupted_turn_notice(
    events: &[StoredEvent],
    target: &ConversationAddress,
) -> Option<Notice> {
    conversation_events(events, target)
        .into_iter()
        .rev()
        .find_map(|event| match &event.payload {
            EventPayload::TurnInterrupted {
                conversation,
                turn,
                reason,
                ..
            } if conversation == target.as_str() => Some(Notice {
                kind: NoticeKind::InterruptedTurn {
                    conversation: target.clone(),
                    turn: *turn,
                    reason: reason.clone(),
                },
                severity: NoticeSeverity::Info,
            }),
            _ => None,
        })
}

fn pending_permission_for_conversation(
    events: &[StoredEvent],
    target: &ConversationAddress,
) -> Option<crate::query::PermissionRequest> {
    let mut tool_conversations = BTreeMap::<String, ConversationAddress>::new();
    let mut decisions = std::collections::BTreeSet::<String>::new();
    let mut results = std::collections::BTreeSet::<String>::new();
    let mut pending = Vec::<crate::query::PermissionRequest>::new();

    for event in crate::context::revert::filter_rolled_back_events(events) {
        match &event.payload {
            EventPayload::ToolCall {
                conversation: Some(conversation),
                tool_call_id,
                ..
            } => {
                if let Ok(address) = ConversationAddress::parse(conversation) {
                    tool_conversations.insert(tool_call_id.clone(), address);
                }
            }
            EventPayload::PermissionRequested {
                turn,
                tool_call_id,
                tool,
                risk,
                summary,
                candidate,
                source,
                ..
            } => {
                pending.push(crate::query::PermissionRequest {
                    id: tool_call_id.clone(),
                    conversation: tool_conversations
                        .get(tool_call_id)
                        .cloned()
                        .unwrap_or(ConversationAddress::MAIN),
                    turn: *turn,
                    tool_call_id: tool_call_id.clone(),
                    tool: tool.clone(),
                    risk: risk.clone(),
                    summary: summary.clone(),
                    candidate: candidate.clone(),
                    source: source.clone(),
                });
            }
            EventPayload::PermissionAllow { tool_call_id, .. }
            | EventPayload::PermissionDeny { tool_call_id, .. } => {
                decisions.insert(tool_call_id.clone());
            }
            EventPayload::ToolResult { tool_call_id, .. } => {
                results.insert(tool_call_id.clone());
            }
            _ => {}
        }
    }

    pending.into_iter().rev().find(|request| {
        request.conversation == *target
            && !decisions.contains(&request.tool_call_id)
            && !results.contains(&request.tool_call_id)
    })
}

fn build_context_drift_notice(
    workspace: &Path,
    events: &[StoredEvent],
    tier: ContextBudgetTier,
) -> Option<Notice> {
    let tracked = rebuild_tracked_file_snapshots(events);
    if tracked.is_empty() {
        return None;
    }

    let mut entries = Vec::new();
    for snapshot in tracked.values() {
        let path = PathBuf::from(&snapshot.path);
        let label = path
            .strip_prefix(workspace)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        match std::fs::read(&path) {
            Ok(current_bytes) => {
                let current_hash = content_hash_bytes(&current_bytes);
                if current_hash == snapshot.hash {
                    continue;
                }
                entries.push(ContextDriftEntry {
                    path: label,
                    status: ContextDriftStatus::Updated,
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                entries.push(ContextDriftEntry {
                    path: label,
                    status: ContextDriftStatus::Deleted,
                });
            }
            Err(_) => continue,
        }
    }

    if entries.is_empty() {
        return None;
    }

    let max = max_context_drift_entries(tier);
    entries.truncate(max);

    Some(Notice {
        kind: NoticeKind::ContextDrift { entries },
        severity: NoticeSeverity::Info,
    })
}

fn max_context_drift_entries(tier: ContextBudgetTier) -> usize {
    match tier {
        ContextBudgetTier::Tight => 4,
        ContextBudgetTier::Normal => 12,
        ContextBudgetTier::Roomy => 32,
    }
}

fn rebuild_tracked_file_snapshots(events: &[StoredEvent]) -> BTreeMap<String, TrackedFileSnapshot> {
    let filtered = crate::context::revert::filter_rolled_back_events(events);
    let mut tracked = BTreeMap::new();
    let mut saw_context_sources = false;

    for event in filtered {
        match &event.payload {
            EventPayload::ContextSources {
                project_instruction_sources,
                memory_sources,
                ..
            } => {
                saw_context_sources = true;
                update_tracked_snapshot_from_context_sources(
                    &mut tracked,
                    project_instruction_sources,
                    memory_sources,
                );
            }
            EventPayload::ToolResult {
                status,
                structured: Some(structured),
                ..
            } if saw_context_sources && status == "ok" => {
                update_tracked_snapshot_from_tool_result(&mut tracked, structured);
            }
            _ => {}
        }
    }

    tracked
}

fn update_tracked_snapshot_from_context_sources(
    tracked: &mut BTreeMap<String, TrackedFileSnapshot>,
    project_instruction_sources: &[crate::context::FileSource],
    memory_sources: &[crate::context::FileSource],
) {
    for source in project_instruction_sources {
        tracked.insert(
            source.path.clone(),
            TrackedFileSnapshot {
                path: source.path.clone(),
                hash: source.hash.clone(),
            },
        );
    }
    for source in memory_sources {
        tracked.insert(
            source.path.clone(),
            TrackedFileSnapshot {
                path: source.path.clone(),
                hash: source.hash.clone(),
            },
        );
    }
}

fn update_tracked_snapshot_from_tool_result(
    tracked: &mut BTreeMap<String, TrackedFileSnapshot>,
    structured: &serde_json::Value,
) {
    let Some(kind) = structured.get("kind").and_then(serde_json::Value::as_str) else {
        return;
    };
    match kind {
        "file_content" => {
            let is_full = structured
                .get("is_full_file_snapshot")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            if !is_full {
                return;
            }
            let Some(path) = structured
                .get("canonical_path")
                .and_then(serde_json::Value::as_str)
            else {
                return;
            };
            let Some(hash) = structured
                .get("content_hash")
                .and_then(serde_json::Value::as_str)
            else {
                return;
            };
            tracked.insert(
                path.to_string(),
                TrackedFileSnapshot {
                    path: path.to_string(),
                    hash: hash.to_string(),
                },
            );
        }
        "file_edit" | "file_write" | "memory_write" | "forget_memory" => {
            let Some(path) = structured
                .get("canonical_path")
                .and_then(serde_json::Value::as_str)
            else {
                return;
            };
            let Some(existing) = tracked.get_mut(path) else {
                return;
            };
            let Some(hash) = structured
                .get("content_hash_after")
                .or_else(|| structured.get("content_hash"))
                .and_then(serde_json::Value::as_str)
            else {
                return;
            };
            existing.hash = hash.to_string();
        }
        _ => {}
    }
}

fn content_hash_bytes(bytes: &[u8]) -> String {
    let digest = sha2::Sha256::digest(bytes);
    format!("sha256:{digest:x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::provenance::FileSource;
    use crate::event::{EventPayload, RollbackScope, StoredEvent};
    use crate::notice::render::render_notice_body;

    fn make_entry(index: usize) -> ContextDriftEntry {
        ContextDriftEntry {
            path: format!("file-{index}.md"),
            status: ContextDriftStatus::Updated,
        }
    }

    #[test]
    fn tight_budget_truncates_context_drift_entries() {
        let entries: Vec<ContextDriftEntry> = (0..10).map(make_entry).collect();
        let max = max_context_drift_entries(ContextBudgetTier::Tight);
        assert_eq!(max, 4);

        let mut truncated = entries;
        truncated.truncate(max);
        let notice = Notice {
            kind: NoticeKind::ContextDrift { entries: truncated },
            severity: NoticeSeverity::Info,
        };
        let rendered = render_notice_body(&notice).expect("should render");

        assert!(rendered.contains("Changed tracked files:"));
        assert!(rendered.contains("file-0.md"));
        assert!(rendered.contains("file-3.md"));
        assert!(!rendered.contains("file-4.md"));
        assert!(!rendered.contains("current preview:"));
        assert!(!rendered.contains("line 17"));
    }

    #[test]
    fn normal_budget_allows_more_entries_than_tight() {
        assert!(
            max_context_drift_entries(ContextBudgetTier::Normal)
                > max_context_drift_entries(ContextBudgetTier::Tight)
        );
        assert!(
            max_context_drift_entries(ContextBudgetTier::Roomy)
                > max_context_drift_entries(ContextBudgetTier::Normal)
        );
    }

    #[test]
    fn tracked_files_follow_latest_context_sources_after_rollback_filtering() {
        let temp = tempfile::tempdir().unwrap();
        let tracked = temp.path().join("AGENTS.md");
        std::fs::write(&tracked, "before").unwrap();

        let events = vec![
            StoredEvent {
                id: 1,
                payload: EventPayload::TurnStarted {
                    turn: 1,
                    ts: "t1".to_string(),
                    conversation: "main".to_string(),
                },
            },
            StoredEvent {
                id: 2,
                payload: EventPayload::ContextSources {
                    turn: 1,
                    ts: "t1".to_string(),
                    request_id: "req_1".to_string(),
                    project_instruction_sources: vec![FileSource {
                        path: tracked.display().to_string(),
                        hash: content_hash_bytes(b"before"),
                    }],
                    memory_sources: vec![],
                },
            },
            StoredEvent {
                id: 3,
                payload: EventPayload::TurnCompleted {
                    turn: 1,
                    ts: "t1".to_string(),
                    conversation: "main".to_string(),
                },
            },
            StoredEvent {
                id: 4,
                payload: EventPayload::TurnStarted {
                    turn: 2,
                    ts: "t2".to_string(),
                    conversation: "main".to_string(),
                },
            },
            StoredEvent {
                id: 5,
                payload: EventPayload::ContextSources {
                    turn: 2,
                    ts: "t2".to_string(),
                    request_id: "req_2".to_string(),
                    project_instruction_sources: vec![FileSource {
                        path: tracked.display().to_string(),
                        hash: content_hash_bytes(b"rolled back"),
                    }],
                    memory_sources: vec![],
                },
            },
            StoredEvent {
                id: 6,
                payload: EventPayload::TurnCompleted {
                    turn: 2,
                    ts: "t2".to_string(),
                    conversation: "main".to_string(),
                },
            },
            StoredEvent {
                id: 7,
                payload: EventPayload::ConversationRollback {
                    ts: "t3".to_string(),
                    conversation: "main".to_string(),
                    to_turn: 2,
                    to_event_id: 6,
                    scope: RollbackScope::ConversationOnly,
                },
            },
        ];

        std::fs::write(&tracked, "after").unwrap();

        let notices = build_runtime_notices(NoticeAssemblyInput {
            workspace: temp.path(),
            events: &events,
            context_budget_tier: ContextBudgetTier::Normal,
            conversation: &ConversationAddress::MAIN,
            agent_registry: None,
        });
        let rendered = render_notice_body(&notices[0]).unwrap();

        assert_eq!(notices.len(), 1);
        assert!(rendered.contains("AGENTS.md"));
        assert!(!rendered.contains("rolled back"));
    }

    #[test]
    fn latest_context_sources_baseline_wins_over_older_tool_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let tracked = temp.path().join("AGENTS.md");
        std::fs::write(&tracked, "baseline-new").unwrap();

        let events = vec![
            StoredEvent {
                id: 1,
                payload: EventPayload::ToolResult {
                    turn: 1,
                    ts: "t1".to_string(),
                    conversation: None,
                    tool_call_id: "tool_1".to_string(),
                    status: "ok".to_string(),
                    summary: "write".to_string(),
                    model_content: String::new(),
                    truncated: false,
                    files_read: Vec::new(),
                    files_changed: Vec::new(),
                    commands_run: Vec::new(),
                    memory_changed: None,
                    structured: Some(serde_json::json!({
                        "kind": "file_write",
                        "canonical_path": tracked.display().to_string(),
                        "content_hash_after": content_hash_bytes(b"old-write")
                    })),
                },
            },
            StoredEvent {
                id: 2,
                payload: EventPayload::ContextSources {
                    turn: 2,
                    ts: "t2".to_string(),
                    request_id: "req_2".to_string(),
                    project_instruction_sources: vec![FileSource {
                        path: tracked.display().to_string(),
                        hash: content_hash_bytes(b"baseline-new"),
                    }],
                    memory_sources: vec![],
                },
            },
        ];

        let notices = build_runtime_notices(NoticeAssemblyInput {
            workspace: temp.path(),
            events: &events,
            context_budget_tier: ContextBudgetTier::Normal,
            conversation: &ConversationAddress::MAIN,
            agent_registry: None,
        });

        assert!(
            notices.is_empty(),
            "older tool mutation must not overwrite newer context.sources baseline"
        );
    }

    #[test]
    fn newer_context_sources_preserve_distinct_tool_tracked_files() {
        let temp = tempfile::tempdir().unwrap();
        let instructions = temp.path().join("AGENTS.md");
        let read_file = temp.path().join("notes.md");
        std::fs::write(&instructions, "instructions").unwrap();
        std::fs::write(&read_file, "changed").unwrap();

        let events = vec![
            StoredEvent {
                id: 1,
                payload: EventPayload::ContextSources {
                    turn: 1,
                    ts: "t1".to_string(),
                    request_id: "req_1".to_string(),
                    project_instruction_sources: vec![FileSource {
                        path: instructions.display().to_string(),
                        hash: content_hash_bytes(b"instructions"),
                    }],
                    memory_sources: vec![],
                },
            },
            StoredEvent {
                id: 2,
                payload: EventPayload::ToolResult {
                    turn: 1,
                    ts: "t2".to_string(),
                    conversation: None,
                    tool_call_id: "tool_1".to_string(),
                    status: "ok".to_string(),
                    summary: "read".to_string(),
                    model_content: String::new(),
                    truncated: false,
                    files_read: Vec::new(),
                    files_changed: Vec::new(),
                    commands_run: Vec::new(),
                    memory_changed: None,
                    structured: Some(serde_json::json!({
                        "kind": "file_content",
                        "canonical_path": read_file.display().to_string(),
                        "content_hash": content_hash_bytes(b"original"),
                        "is_full_file_snapshot": true
                    })),
                },
            },
            StoredEvent {
                id: 3,
                payload: EventPayload::ContextSources {
                    turn: 1,
                    ts: "t3".to_string(),
                    request_id: "req_2".to_string(),
                    project_instruction_sources: vec![FileSource {
                        path: instructions.display().to_string(),
                        hash: content_hash_bytes(b"instructions"),
                    }],
                    memory_sources: vec![],
                },
            },
        ];

        let notices = build_runtime_notices(NoticeAssemblyInput {
            workspace: temp.path(),
            events: &events,
            context_budget_tier: ContextBudgetTier::Normal,
            conversation: &ConversationAddress::MAIN,
            agent_registry: None,
        });
        let rendered = render_notice_body(&notices[0]).unwrap();

        assert_eq!(notices.len(), 1);
        assert!(rendered.contains("notes.md"));
    }

    #[test]
    fn forget_memory_updates_tracked_snapshot_hash() {
        let temp = tempfile::tempdir().unwrap();
        let tracked = temp.path().join("memory.md");
        std::fs::write(&tracked, "after forget").unwrap();

        let events = vec![
            StoredEvent {
                id: 1,
                payload: EventPayload::ContextSources {
                    turn: 1,
                    ts: "t1".to_string(),
                    request_id: "req_1".to_string(),
                    project_instruction_sources: vec![],
                    memory_sources: vec![FileSource {
                        path: tracked.display().to_string(),
                        hash: content_hash_bytes(b"before forget"),
                    }],
                },
            },
            StoredEvent {
                id: 2,
                payload: EventPayload::ToolResult {
                    turn: 1,
                    ts: "t2".to_string(),
                    conversation: None,
                    tool_call_id: "tool_2".to_string(),
                    status: "ok".to_string(),
                    summary: "forget".to_string(),
                    model_content: String::new(),
                    truncated: false,
                    files_read: Vec::new(),
                    files_changed: Vec::new(),
                    commands_run: Vec::new(),
                    memory_changed: None,
                    structured: Some(serde_json::json!({
                        "kind": "forget_memory",
                        "canonical_path": tracked.display().to_string(),
                        "content_hash_after": content_hash_bytes(b"after forget")
                    })),
                },
            },
        ];

        let notices = build_runtime_notices(NoticeAssemblyInput {
            workspace: temp.path(),
            events: &events,
            context_budget_tier: ContextBudgetTier::Normal,
            conversation: &ConversationAddress::MAIN,
            agent_registry: None,
        });

        assert!(
            notices.is_empty(),
            "forget_memory should advance the tracked hash to the post-tool value"
        );
    }
}
