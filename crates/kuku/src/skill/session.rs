use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use crate::config::DiscoveryConfig;
use crate::context::revert::filter_rolled_back_events;
use crate::conversation::binding::BindingSource;
use crate::event::{EventPayload, StoredEvent};
use crate::plugin::PluginRegistry;

use super::registry::SkillRegistry;

/// Build a skill registry including plugin-contributed skills.
pub fn build_registry_snapshot_for_host(
    kuku_home: &Path,
    workspace: &Path,
    config: &crate::config::Config,
) -> crate::error::Result<SkillRegistry> {
    let plugin_registry = if config.plugin.enabled {
        Some(
            crate::plugin::PluginRegistry::builder()
                .load_packages(kuku_home, workspace)?
                .build()?,
        )
    } else {
        None
    };

    build_registry_snapshot(workspace, &config.discovery, plugin_registry.as_ref())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TurnSkillSnapshot {
    pub conversation: String,
    pub registry: SkillRegistry,
    pub bootstrap_loaded: Vec<String>,
}

pub(crate) fn build_registry_snapshot(
    workspace: &Path,
    discovery_config: &DiscoveryConfig,
    plugin_registry: Option<&PluginRegistry>,
) -> crate::error::Result<SkillRegistry> {
    let mut builder = SkillRegistry::builder().build_with_discovery(workspace, discovery_config)?;
    if let Some(plugin_registry) = plugin_registry {
        for (skill_dir, tier) in plugin_registry.skill_dirs() {
            builder = builder.load_from_dir(skill_dir, (*tier).into())?;
        }
    }
    Ok(builder.build())
}

pub(crate) fn restore_turn_snapshot(
    events: &[StoredEvent],
    conversation: &str,
    turn: u64,
) -> Option<TurnSkillSnapshot> {
    let filtered = filter_rolled_back_events(events);
    restore_turn_snapshot_from_filtered(&filtered, conversation, turn)
}

pub(crate) fn previous_snapshot_before_turn(
    events: &[StoredEvent],
    conversation: &str,
    turn: u64,
) -> Option<TurnSkillSnapshot> {
    let filtered = filter_rolled_back_events(events);
    filtered
        .iter()
        .rev()
        .find_map(|event| match &event.payload {
            EventPayload::ContextSkills {
                conversation: event_conversation,
                turn: event_turn,
                registry,
                bootstrap_loaded,
                ..
            } if *event_turn < turn && event_conversation == conversation => {
                let registry: SkillRegistry = serde_json::from_value(registry.clone()).ok()?;
                Some(TurnSkillSnapshot {
                    conversation: event_conversation.clone(),
                    registry,
                    bootstrap_loaded: bootstrap_loaded.clone(),
                })
            }
            _ => None,
        })
}

pub(crate) fn loaded_skill_names(events: &[StoredEvent], conversation: &str) -> Vec<String> {
    let filtered = filter_rolled_back_events(events);
    let mut loaded = BTreeSet::<String>::new();
    let mut pending_use_skill = HashMap::<String, String>::new();

    for event in filtered {
        match &event.payload {
            EventPayload::ContextSkills {
                conversation: event_conversation,
                bootstrap_loaded,
                ..
            } if event_conversation == conversation => {
                loaded.extend(bootstrap_loaded.iter().cloned());
            }
            EventPayload::ToolCall {
                conversation: Some(event_conversation),
                tool_call_id,
                tool,
                args,
                ..
            } if tool == "use_skill" && event_conversation == conversation => {
                if let Some(skill_name) = args.get("skill_name").and_then(|value| value.as_str()) {
                    pending_use_skill.insert(tool_call_id.clone(), skill_name.to_string());
                }
            }
            EventPayload::ToolResult {
                conversation: Some(event_conversation),
                tool_call_id,
                status,
                truncated,
                ..
            } if status == "ok" && event_conversation == conversation => {
                if *truncated {
                    continue;
                }
                if let Some(skill_name) = pending_use_skill.get(tool_call_id) {
                    loaded.insert(skill_name.clone());
                }
            }
            _ => {}
        }
    }

    loaded.into_iter().collect()
}

pub(crate) fn binding_sources_for_skills(
    registry: &SkillRegistry,
    skill_names: &[String],
) -> Vec<BindingSource> {
    skill_names
        .iter()
        .filter_map(|skill_name| {
            let definition = registry.get(skill_name)?;
            Some(BindingSource {
                kind: "skill_definition".to_string(),
                source: definition
                    .source_path
                    .clone()
                    .unwrap_or_else(|| definition.name.clone()),
                hash: definition.hash.clone(),
            })
        })
        .collect()
}

fn restore_turn_snapshot_from_filtered(
    events: &[&StoredEvent],
    conversation: &str,
    turn: u64,
) -> Option<TurnSkillSnapshot> {
    events.iter().rev().find_map(|event| match &event.payload {
        EventPayload::ContextSkills {
            conversation: event_conversation,
            turn: event_turn,
            registry,
            bootstrap_loaded,
            ..
        } if *event_turn == turn && event_conversation == conversation => {
            let registry: SkillRegistry = serde_json::from_value(registry.clone()).ok()?;
            Some(TurnSkillSnapshot {
                conversation: event_conversation.clone(),
                registry,
                bootstrap_loaded: bootstrap_loaded.clone(),
            })
        }
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::event::RollbackScope;
    use crate::skill::definition::{SkillDefinition, SkillSource};

    fn definition(name: &str) -> SkillDefinition {
        let mut definition = SkillDefinition {
            name: name.to_string(),
            description: format!("{name} description"),
            instructions: format!("{name} instructions"),
            source: SkillSource::Project,
            hash: String::new(),
            source_path: Some(format!("/skills/{name}")),
            allowed_tools: None,
            disallowed_tools: None,
            max_turns: None,
            model: None,
            license: None,
            compatibility: None,
            metadata: serde_json::Value::Null,
        };
        definition.hash = definition.compute_hash();
        definition
    }

    fn skill_registry(names: &[&str]) -> SkillRegistry {
        names
            .iter()
            .fold(SkillRegistry::builder(), |builder, name| {
                builder.with_definition(definition(name))
            })
            .build()
    }

    fn registry(names: &[&str]) -> serde_json::Value {
        serde_json::to_value(skill_registry(names)).unwrap()
    }

    fn event(id: u64, payload: EventPayload) -> StoredEvent {
        StoredEvent { id, payload }
    }

    #[test]
    fn restores_current_and_previous_turn_snapshots() {
        let events = vec![
            event(
                1,
                EventPayload::ContextSkills {
                    conversation: "main".to_string(),
                    turn: 1,
                    ts: "t1".to_string(),
                    registry: registry(&["alpha"]),
                    bootstrap_loaded: vec!["bootstrap-alpha".to_string()],
                },
            ),
            event(
                2,
                EventPayload::ContextSkills {
                    conversation: "main".to_string(),
                    turn: 2,
                    ts: "t2".to_string(),
                    registry: registry(&["beta"]),
                    bootstrap_loaded: vec!["bootstrap-beta".to_string()],
                },
            ),
        ];

        assert_eq!(
            restore_turn_snapshot(&events, "main", 2),
            Some(TurnSkillSnapshot {
                conversation: "main".to_string(),
                registry: skill_registry(&["beta"]),
                bootstrap_loaded: vec!["bootstrap-beta".to_string()],
            })
        );
        assert_eq!(
            previous_snapshot_before_turn(&events, "main", 2),
            Some(TurnSkillSnapshot {
                conversation: "main".to_string(),
                registry: skill_registry(&["alpha"]),
                bootstrap_loaded: vec!["bootstrap-alpha".to_string()],
            })
        );
    }

    #[test]
    fn ignores_rolled_back_snapshots_when_restoring() {
        let events = vec![
            event(
                1,
                EventPayload::ContextSkills {
                    conversation: "main".to_string(),
                    turn: 1,
                    ts: "t1".to_string(),
                    registry: registry(&["keep"]),
                    bootstrap_loaded: vec!["keep".to_string()],
                },
            ),
            event(
                2,
                EventPayload::TurnStarted {
                    turn: 2,
                    ts: "t2".to_string(),
                    conversation: "main".to_string(),
                },
            ),
            event(
                3,
                EventPayload::ContextSkills {
                    conversation: "main".to_string(),
                    turn: 2,
                    ts: "t3".to_string(),
                    registry: registry(&["rollback"]),
                    bootstrap_loaded: vec!["rollback".to_string()],
                },
            ),
            event(
                4,
                EventPayload::ConversationRollback {
                    ts: "t4".to_string(),
                    conversation: "main".to_string(),
                    to_turn: 2,
                    to_event_id: 3,
                    scope: RollbackScope::ConversationOnly,
                },
            ),
        ];

        assert_eq!(restore_turn_snapshot(&events, "main", 2), None);
        assert_eq!(
            previous_snapshot_before_turn(&events, "main", 3),
            Some(TurnSkillSnapshot {
                conversation: "main".to_string(),
                registry: skill_registry(&["keep"]),
                bootstrap_loaded: vec!["keep".to_string()],
            })
        );
    }

    #[test]
    fn loaded_skill_names_only_include_successful_use_skill_results() {
        let events = vec![
            event(
                1,
                EventPayload::ContextSkills {
                    conversation: "main".to_string(),
                    turn: 3,
                    ts: "t1".to_string(),
                    registry: registry(&["alpha", "beta"]),
                    bootstrap_loaded: vec!["bootstrap-beta".to_string(), "alpha".to_string()],
                },
            ),
            event(
                2,
                EventPayload::ToolCall {
                    turn: 3,
                    ts: "t2".to_string(),
                    conversation: Some("main".to_string()),
                    tool_call_id: "tool_ok".to_string(),
                    request_id: "req_3".to_string(),
                    index: 0,
                    tool: "use_skill".to_string(),
                    args: json!({ "skill_name": "alpha" }),
                },
            ),
            event(
                3,
                EventPayload::ToolResult {
                    turn: 3,
                    ts: "t3".to_string(),
                    conversation: Some("main".to_string()),
                    tool_call_id: "tool_ok".to_string(),
                    status: "ok".to_string(),
                    summary: "loaded skill: alpha".to_string(),
                    model_content: "alpha".to_string(),
                    truncated: false,
                    files_read: Vec::new(),
                    files_changed: Vec::new(),
                    commands_run: Vec::new(),
                    memory_changed: None,
                    structured: None,
                },
            ),
            event(
                4,
                EventPayload::ToolCall {
                    turn: 3,
                    ts: "t4".to_string(),
                    conversation: Some("main".to_string()),
                    tool_call_id: "tool_blocked".to_string(),
                    request_id: "req_3".to_string(),
                    index: 1,
                    tool: "use_skill".to_string(),
                    args: json!({ "skill_name": "beta" }),
                },
            ),
            event(
                5,
                EventPayload::ToolResult {
                    turn: 3,
                    ts: "t5".to_string(),
                    conversation: Some("main".to_string()),
                    tool_call_id: "tool_blocked".to_string(),
                    status: "error".to_string(),
                    summary: "blocked".to_string(),
                    model_content: "blocked".to_string(),
                    truncated: false,
                    files_read: Vec::new(),
                    files_changed: Vec::new(),
                    commands_run: Vec::new(),
                    memory_changed: None,
                    structured: None,
                },
            ),
        ];

        assert_eq!(
            loaded_skill_names(&events, "main"),
            vec!["alpha".to_string(), "bootstrap-beta".to_string(),]
        );
    }

    #[test]
    fn loaded_skill_names_ignores_blocked_use_skill_results() {
        let events = vec![
            event(
                1,
                EventPayload::ContextSkills {
                    conversation: "main".to_string(),
                    turn: 4,
                    ts: "t1".to_string(),
                    registry: registry(&["alpha", "beta"]),
                    bootstrap_loaded: vec![],
                },
            ),
            event(
                2,
                EventPayload::ToolCall {
                    turn: 4,
                    ts: "t2".to_string(),
                    conversation: Some("main".to_string()),
                    tool_call_id: "tool_alpha".to_string(),
                    request_id: "req_4".to_string(),
                    index: 0,
                    tool: "use_skill".to_string(),
                    args: json!({ "skill_name": "alpha" }),
                },
            ),
            event(
                3,
                EventPayload::ToolResult {
                    turn: 4,
                    ts: "t3".to_string(),
                    conversation: Some("main".to_string()),
                    tool_call_id: "tool_alpha".to_string(),
                    status: "ok".to_string(),
                    summary: "loaded skill: alpha".to_string(),
                    model_content: "alpha".to_string(),
                    truncated: false,
                    files_read: Vec::new(),
                    files_changed: Vec::new(),
                    commands_run: Vec::new(),
                    memory_changed: None,
                    structured: None,
                },
            ),
            event(
                4,
                EventPayload::ToolCall {
                    turn: 4,
                    ts: "t4".to_string(),
                    conversation: Some("main".to_string()),
                    tool_call_id: "tool_beta".to_string(),
                    request_id: "req_4".to_string(),
                    index: 1,
                    tool: "use_skill".to_string(),
                    args: json!({ "skill_name": "beta" }),
                },
            ),
            event(
                5,
                EventPayload::ToolResult {
                    turn: 4,
                    ts: "t5".to_string(),
                    conversation: Some("main".to_string()),
                    tool_call_id: "tool_beta".to_string(),
                    status: "blocked".to_string(),
                    summary: "blocked".to_string(),
                    model_content: "blocked".to_string(),
                    truncated: false,
                    files_read: Vec::new(),
                    files_changed: Vec::new(),
                    commands_run: Vec::new(),
                    memory_changed: None,
                    structured: None,
                },
            ),
        ];

        assert_eq!(
            loaded_skill_names(&events, "main"),
            vec!["alpha".to_string()]
        );
    }

    #[test]
    fn loaded_skill_names_ignores_truncated_use_skill_results() {
        let events = vec![
            event(
                1,
                EventPayload::ContextSkills {
                    conversation: "main".to_string(),
                    turn: 4,
                    ts: "t1".to_string(),
                    registry: registry(&["alpha"]),
                    bootstrap_loaded: vec![],
                },
            ),
            event(
                2,
                EventPayload::ToolCall {
                    turn: 4,
                    ts: "t2".to_string(),
                    conversation: Some("main".to_string()),
                    tool_call_id: "tool_alpha".to_string(),
                    request_id: "req_4".to_string(),
                    index: 0,
                    tool: "use_skill".to_string(),
                    args: json!({ "skill_name": "alpha" }),
                },
            ),
            event(
                3,
                EventPayload::ToolResult {
                    turn: 4,
                    ts: "t3".to_string(),
                    conversation: Some("main".to_string()),
                    tool_call_id: "tool_alpha".to_string(),
                    status: "ok".to_string(),
                    summary: "loaded skill: alpha".to_string(),
                    model_content: "partial alpha".to_string(),
                    truncated: true,
                    files_read: Vec::new(),
                    files_changed: Vec::new(),
                    commands_run: Vec::new(),
                    memory_changed: None,
                    structured: None,
                },
            ),
        ];

        assert!(loaded_skill_names(&events, "main").is_empty());
    }

    #[test]
    fn loaded_skill_names_ignores_error_and_cancelled_use_skill_results() {
        let events = vec![
            event(
                1,
                EventPayload::ContextSkills {
                    conversation: "main".to_string(),
                    turn: 5,
                    ts: "t1".to_string(),
                    registry: registry(&["alpha", "beta", "gamma"]),
                    bootstrap_loaded: vec![],
                },
            ),
            event(
                2,
                EventPayload::ToolCall {
                    turn: 5,
                    ts: "t2".to_string(),
                    conversation: Some("main".to_string()),
                    tool_call_id: "tool_alpha".to_string(),
                    request_id: "req_5".to_string(),
                    index: 0,
                    tool: "use_skill".to_string(),
                    args: json!({ "skill_name": "alpha" }),
                },
            ),
            event(
                3,
                EventPayload::ToolResult {
                    turn: 5,
                    ts: "t3".to_string(),
                    conversation: Some("main".to_string()),
                    tool_call_id: "tool_alpha".to_string(),
                    status: "ok".to_string(),
                    summary: "loaded skill: alpha".to_string(),
                    model_content: "alpha".to_string(),
                    truncated: false,
                    files_read: Vec::new(),
                    files_changed: Vec::new(),
                    commands_run: Vec::new(),
                    memory_changed: None,
                    structured: None,
                },
            ),
            event(
                4,
                EventPayload::ToolCall {
                    turn: 5,
                    ts: "t4".to_string(),
                    conversation: Some("main".to_string()),
                    tool_call_id: "tool_beta".to_string(),
                    request_id: "req_5".to_string(),
                    index: 1,
                    tool: "use_skill".to_string(),
                    args: json!({ "skill_name": "beta" }),
                },
            ),
            event(
                5,
                EventPayload::ToolResult {
                    turn: 5,
                    ts: "t5".to_string(),
                    conversation: Some("main".to_string()),
                    tool_call_id: "tool_beta".to_string(),
                    status: "error".to_string(),
                    summary: "failed".to_string(),
                    model_content: "failed".to_string(),
                    truncated: false,
                    files_read: Vec::new(),
                    files_changed: Vec::new(),
                    commands_run: Vec::new(),
                    memory_changed: None,
                    structured: None,
                },
            ),
            event(
                6,
                EventPayload::ToolCall {
                    turn: 5,
                    ts: "t6".to_string(),
                    conversation: Some("main".to_string()),
                    tool_call_id: "tool_gamma".to_string(),
                    request_id: "req_5".to_string(),
                    index: 2,
                    tool: "use_skill".to_string(),
                    args: json!({ "skill_name": "gamma" }),
                },
            ),
        ];

        assert_eq!(
            loaded_skill_names(&events, "main"),
            vec!["alpha".to_string()]
        );
    }

    #[test]
    fn loaded_skill_names_ignore_trailing_rollback_admin_events() {
        let events = vec![
            event(
                1,
                EventPayload::ContextSkills {
                    conversation: "main".to_string(),
                    turn: 4,
                    ts: "t1".to_string(),
                    registry: registry(&["alpha"]),
                    bootstrap_loaded: vec![],
                },
            ),
            event(
                2,
                EventPayload::ToolCall {
                    turn: 4,
                    ts: "t2".to_string(),
                    conversation: Some("main".to_string()),
                    tool_call_id: "tool_alpha".to_string(),
                    request_id: "req_4".to_string(),
                    index: 0,
                    tool: "use_skill".to_string(),
                    args: json!({ "skill_name": "alpha" }),
                },
            ),
            event(
                3,
                EventPayload::ToolResult {
                    turn: 4,
                    ts: "t3".to_string(),
                    conversation: Some("main".to_string()),
                    tool_call_id: "tool_alpha".to_string(),
                    status: "ok".to_string(),
                    summary: "loaded skill: alpha".to_string(),
                    model_content: "alpha".to_string(),
                    truncated: false,
                    files_read: Vec::new(),
                    files_changed: Vec::new(),
                    commands_run: Vec::new(),
                    memory_changed: None,
                    structured: None,
                },
            ),
            event(
                4,
                EventPayload::ConversationRollback {
                    ts: "t4".to_string(),
                    conversation: "main".to_string(),
                    to_turn: 2,
                    to_event_id: 3,
                    scope: RollbackScope::ConversationOnly,
                },
            ),
            event(
                5,
                EventPayload::ConversationRollbackUndone {
                    ts: "t5".to_string(),
                    conversation: "main".to_string(),
                    rollback_event_id: 4,
                },
            ),
        ];

        assert_eq!(
            loaded_skill_names(&events, "main"),
            vec!["alpha".to_string()]
        );
    }

    #[test]
    fn loaded_skill_names_remain_cumulative_across_turns() {
        let events = vec![
            event(
                1,
                EventPayload::ContextSkills {
                    conversation: "main".to_string(),
                    turn: 1,
                    ts: "t1".to_string(),
                    registry: registry(&["alpha"]),
                    bootstrap_loaded: vec![],
                },
            ),
            event(
                2,
                EventPayload::ToolCall {
                    turn: 1,
                    ts: "t2".to_string(),
                    conversation: Some("main".to_string()),
                    tool_call_id: "tool_alpha".to_string(),
                    request_id: "req_1".to_string(),
                    index: 0,
                    tool: "use_skill".to_string(),
                    args: json!({ "skill_name": "alpha" }),
                },
            ),
            event(
                3,
                EventPayload::ToolResult {
                    turn: 1,
                    ts: "t3".to_string(),
                    conversation: Some("main".to_string()),
                    tool_call_id: "tool_alpha".to_string(),
                    status: "ok".to_string(),
                    summary: "loaded skill: alpha".to_string(),
                    model_content: "alpha".to_string(),
                    truncated: false,
                    files_read: Vec::new(),
                    files_changed: Vec::new(),
                    commands_run: Vec::new(),
                    memory_changed: None,
                    structured: None,
                },
            ),
            event(
                4,
                EventPayload::ContextSkills {
                    conversation: "main".to_string(),
                    turn: 2,
                    ts: "t4".to_string(),
                    registry: registry(&["beta"]),
                    bootstrap_loaded: vec!["beta".to_string()],
                },
            ),
        ];

        assert_eq!(
            loaded_skill_names(&events, "main"),
            vec!["alpha".to_string(), "beta".to_string()]
        );
    }

    #[test]
    fn loaded_skill_names_are_conversation_scoped() {
        let events = vec![
            event(
                1,
                EventPayload::ContextSkills {
                    conversation: "main".to_string(),
                    turn: 1,
                    ts: "t1".to_string(),
                    registry: registry(&["review"]),
                    bootstrap_loaded: vec![],
                },
            ),
            event(
                2,
                EventPayload::ContextSkills {
                    conversation: "review".to_string(),
                    turn: 1,
                    ts: "t2".to_string(),
                    registry: registry(&["review"]),
                    bootstrap_loaded: vec![],
                },
            ),
            event(
                3,
                EventPayload::ToolCall {
                    turn: 1,
                    ts: "t3".to_string(),
                    conversation: Some("review".to_string()),
                    tool_call_id: "tool_review".to_string(),
                    request_id: "req_1".to_string(),
                    index: 0,
                    tool: "use_skill".to_string(),
                    args: json!({ "skill_name": "review" }),
                },
            ),
            event(
                4,
                EventPayload::ToolResult {
                    turn: 1,
                    ts: "t4".to_string(),
                    conversation: Some("review".to_string()),
                    tool_call_id: "tool_review".to_string(),
                    status: "ok".to_string(),
                    summary: "loaded skill: review".to_string(),
                    model_content: "review".to_string(),
                    truncated: false,
                    files_read: Vec::new(),
                    files_changed: Vec::new(),
                    commands_run: Vec::new(),
                    memory_changed: None,
                    structured: None,
                },
            ),
        ];

        assert!(loaded_skill_names(&events, "main").is_empty());
        assert_eq!(
            loaded_skill_names(&events, "review"),
            vec!["review".to_string()]
        );
    }
}
