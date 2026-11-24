use std::path::{Path, PathBuf};

use sha2::Digest;

use crate::conversation::address::ConversationAddress;
use crate::event::{EventPayload, StoredEvent};
use crate::skill::session::loaded_skill_names;

use super::types::{
    ContextBudgetTier, ContextDriftEntry, ContextDriftStatus, Notice, NoticeKind, NoticeSeverity,
};

pub(crate) struct NoticeAssemblyInput<'a> {
    pub(crate) workspace: &'a Path,
    pub(crate) events: &'a [StoredEvent],
    pub(crate) context_budget_tier: ContextBudgetTier,
    pub(crate) conversation: &'a ConversationAddress,
}

pub(crate) fn build_runtime_notices(input: NoticeAssemblyInput<'_>) -> Vec<Notice> {
    let mut notices = Vec::new();

    if let Some(notice) = build_loaded_skills_notice(input.events, input.conversation) {
        notices.push(notice);
    }

    // pending_permission and interrupted_turn notices were removed (Phase 8).
    // Host UI derives these from raw events instead.

    if let Some(notice) = build_context_drift_notice(
        input.workspace,
        input.events,
        input.conversation.as_str(),
        input.context_budget_tier,
    ) {
        notices.push(notice);
    }

    notices
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

fn latest_prompt_snapshot<'a>(
    events: &'a [StoredEvent],
    conversation: &str,
) -> Option<&'a EventPayload> {
    events
        .iter()
        .rev()
        .find(|e| matches!(&e.payload, EventPayload::PromptSnapshot { conversation: c, .. } if c == conversation))
        .map(|e| &e.payload)
}

fn build_context_drift_notice(
    workspace: &Path,
    events: &[StoredEvent],
    conversation: &str,
    tier: ContextBudgetTier,
) -> Option<Notice> {
    let snapshot = latest_prompt_snapshot(events, conversation)?;
    let (project_sources, memory_sources, asset_sources) = match snapshot {
        EventPayload::PromptSnapshot {
            project_instruction_sources,
            memory_sources,
            prompt_asset_sources,
            ..
        } => (
            project_instruction_sources,
            memory_sources,
            prompt_asset_sources,
        ),
        _ => return None,
    };

    let mut entries = Vec::new();
    for source in project_sources
        .iter()
        .chain(memory_sources.iter())
        .chain(asset_sources.iter())
    {
        let path = PathBuf::from(&source.path);
        let label = path
            .strip_prefix(workspace)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        match std::fs::read(&path) {
            Ok(current_bytes) => {
                let current_hash = content_hash_bytes(&current_bytes);
                if current_hash == source.hash {
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

fn content_hash_bytes(bytes: &[u8]) -> String {
    let digest = sha2::Sha256::digest(bytes);
    format!("sha256:{digest:x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::provenance::FileSource;
    use crate::event::{EventPayload, StoredEvent};
    use crate::notice::render::render_notice_body;
    use crate::prompt::builtin_prompt_catalog;
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
        let rendered = render_notice_body(&notice, &builtin_prompt_catalog())
            .unwrap()
            .expect("should render");

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

    fn make_prompt_snapshot(
        id: u64,
        conversation: &str,
        project_sources: Vec<FileSource>,
        memory_sources: Vec<FileSource>,
    ) -> StoredEvent {
        StoredEvent {
            id,
            payload: EventPayload::PromptSnapshot {
                ts: "2026-01-01T00:00:00Z".to_string(),
                conversation: conversation.to_string(),
                snapshot_id: "snap_1".to_string(),
                turn: 1,
                messages: vec![],
                project_instruction_sources: project_sources,
                memory_sources,
                prompt_asset_sources: vec![],
                skills: serde_json::Value::Null,
                bootstrap_loaded: vec![],
                provider: "test".to_string(),
                model: "test".to_string(),
                renderer: crate::context::provenance::PromptRendererIdentity {
                    provider: "test".to_string(),
                    renderer: "test".to_string(),
                },
                agent_registry: None,
                tool_registry: Box::new(crate::context::provenance::ToolRegistryProvenance {
                    hash: "count:0".to_string(),
                    names: vec![],
                    tool_count: 0,
                }),
                skill_registry: Box::new(None),
                plugin_registry: Box::new(None),
                capabilities: crate::context::provenance::PromptCapabilityMetadata {
                    context_budget_tier: "normal".to_string(),
                    max_context_tokens: None,
                    remaining_input_tokens: None,
                },
            },
        }
    }

    #[test]
    fn prompt_snapshot_drift_detected_when_file_changed() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("AGENTS.md");
        std::fs::write(&file, "before").unwrap();

        let events = vec![make_prompt_snapshot(
            1,
            "main",
            vec![FileSource {
                path: file.display().to_string(),
                hash: content_hash_bytes(b"before"),
            }],
            vec![],
        )];

        std::fs::write(&file, "after").unwrap();

        let notices = build_runtime_notices(NoticeAssemblyInput {
            workspace: temp.path(),
            events: &events,
            context_budget_tier: ContextBudgetTier::Normal,
            conversation: &ConversationAddress::MAIN,
        });
        let rendered = render_notice_body(&notices[0], &builtin_prompt_catalog())
            .unwrap()
            .unwrap();

        assert_eq!(notices.len(), 1);
        assert!(rendered.contains("AGENTS.md"));
    }

    #[test]
    fn prompt_snapshot_no_drift_when_hashes_match() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("AGENTS.md");
        std::fs::write(&file, "stable").unwrap();

        let events = vec![make_prompt_snapshot(
            1,
            "main",
            vec![FileSource {
                path: file.display().to_string(),
                hash: content_hash_bytes(b"stable"),
            }],
            vec![],
        )];

        let notices = build_runtime_notices(NoticeAssemblyInput {
            workspace: temp.path(),
            events: &events,
            context_budget_tier: ContextBudgetTier::Normal,
            conversation: &ConversationAddress::MAIN,
        });

        assert!(notices.is_empty(), "no drift when hashes match");
    }

    #[test]
    fn prompt_snapshot_drift_detects_deleted_file() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("deleted.md");

        let events = vec![make_prompt_snapshot(
            1,
            "main",
            vec![FileSource {
                path: file.display().to_string(),
                hash: content_hash_bytes(b"existed"),
            }],
            vec![],
        )];

        let notices = build_runtime_notices(NoticeAssemblyInput {
            workspace: temp.path(),
            events: &events,
            context_budget_tier: ContextBudgetTier::Normal,
            conversation: &ConversationAddress::MAIN,
        });
        let rendered = render_notice_body(&notices[0], &builtin_prompt_catalog())
            .unwrap()
            .unwrap();

        assert_eq!(notices.len(), 1);
        assert!(rendered.contains("deleted.md"));
    }

    #[test]
    fn prompt_snapshot_drift_for_memory_source() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("memory.md");
        std::fs::write(&file, "old memory").unwrap();

        let events = vec![make_prompt_snapshot(
            1,
            "main",
            vec![],
            vec![FileSource {
                path: file.display().to_string(),
                hash: content_hash_bytes(b"old memory"),
            }],
        )];

        std::fs::write(&file, "new memory").unwrap();

        let notices = build_runtime_notices(NoticeAssemblyInput {
            workspace: temp.path(),
            events: &events,
            context_budget_tier: ContextBudgetTier::Normal,
            conversation: &ConversationAddress::MAIN,
        });
        let rendered = render_notice_body(&notices[0], &builtin_prompt_catalog())
            .unwrap()
            .unwrap();

        assert_eq!(notices.len(), 1);
        assert!(rendered.contains("memory.md"));
    }
}
