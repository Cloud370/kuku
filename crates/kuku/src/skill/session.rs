#![allow(dead_code)]

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use crate::config::DiscoveryConfig;
use crate::context::revert::filter_rolled_back_events;
use crate::event::{EventPayload, StoredEvent};
use crate::plugin::PluginRegistry;
use crate::skill::definition::SkillSource;

use super::registry::SkillRegistry;

pub fn build_registry_snapshot_for_host(
    kuku_home: &Path,
    workspace: &Path,
    config: &crate::config::Config,
) -> crate::error::Result<SkillRegistry> {
    let extra_skill_dirs = if config.plugin.enabled {
        Vec::new()
    } else {
        package_skill_dirs(kuku_home, workspace)?
    };

    let plugin_registry = if config.plugin.enabled {
        Some(
            crate::plugin::PluginRegistry::builder()
                .load_packages(kuku_home, workspace)?
                .build()?,
        )
    } else {
        None
    };

    build_registry_snapshot(
        workspace,
        &config.discovery,
        plugin_registry.as_ref(),
        &extra_skill_dirs,
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TurnSkillSnapshot {
    pub registry: SkillRegistry,
    pub bootstrap_loaded: Vec<String>,
}

pub(crate) fn build_registry_snapshot(
    workspace: &Path,
    discovery_config: &DiscoveryConfig,
    plugin_registry: Option<&PluginRegistry>,
    extra_skill_dirs: &[(std::path::PathBuf, SkillSource)],
) -> crate::error::Result<SkillRegistry> {
    let mut builder = SkillRegistry::builder().build_with_discovery(workspace, discovery_config)?;
    for (skill_dir, source) in extra_skill_dirs {
        builder = builder.load_from_dir(skill_dir, *source)?;
    }
    if let Some(plugin_registry) = plugin_registry {
        for (skill_dir, tier) in plugin_registry.skill_dirs() {
            builder = builder.load_from_dir(skill_dir, (*tier).into())?;
        }
    }
    Ok(builder.build())
}

pub(crate) fn package_skill_dirs(
    kuku_home: &Path,
    workspace: &Path,
) -> crate::error::Result<Vec<(std::path::PathBuf, SkillSource)>> {
    Ok(
        crate::plugin::loader::collect_skill_dirs(&crate::plugin::loader::discover_packages(
            kuku_home, workspace,
        )?)
        .into_iter()
        .map(|(path, tier)| (path, tier.into()))
        .collect(),
    )
}

pub(crate) fn restore_turn_snapshot(
    events: &[StoredEvent],
    turn: u64,
) -> Option<TurnSkillSnapshot> {
    let filtered = filter_rolled_back_events(events);
    restore_turn_snapshot_from_filtered(&filtered, turn)
}

pub(crate) fn previous_snapshot_before_turn(
    events: &[StoredEvent],
    turn: u64,
) -> Option<TurnSkillSnapshot> {
    let filtered = filter_rolled_back_events(events);
    filtered
        .iter()
        .rev()
        .find_map(|event| match &event.payload {
            EventPayload::ContextSkills {
                turn: event_turn,
                registry,
                bootstrap_loaded,
                ..
            } if *event_turn < turn => Some(TurnSkillSnapshot {
                registry: registry.clone(),
                bootstrap_loaded: bootstrap_loaded.clone(),
            }),
            _ => None,
        })
}

pub(crate) fn loaded_skill_names(events: &[StoredEvent]) -> Vec<String> {
    let filtered = filter_rolled_back_events(events);
    let mut loaded = BTreeSet::<String>::new();
    let mut pending_use_skill = HashMap::<String, String>::new();

    for event in filtered {
        match &event.payload {
            EventPayload::ContextSkills {
                bootstrap_loaded, ..
            } => {
                loaded.extend(bootstrap_loaded.iter().cloned());
            }
            EventPayload::ToolCall {
                tool_call_id,
                tool,
                args,
                ..
            } if tool == "use_skill" => {
                if let Some(skill_name) = args.get("skill_name").and_then(|value| value.as_str()) {
                    pending_use_skill.insert(tool_call_id.clone(), skill_name.to_string());
                }
            }
            EventPayload::ToolResult {
                tool_call_id,
                status,
                truncated,
                ..
            } if status == "ok" => {
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

fn restore_turn_snapshot_from_filtered(
    events: &[&StoredEvent],
    turn: u64,
) -> Option<TurnSkillSnapshot> {
    events.iter().rev().find_map(|event| match &event.payload {
        EventPayload::ContextSkills {
            turn: event_turn,
            registry,
            bootstrap_loaded,
            ..
        } if *event_turn == turn => Some(TurnSkillSnapshot {
            registry: registry.clone(),
            bootstrap_loaded: bootstrap_loaded.clone(),
        }),
        _ => None,
    })
}

fn event_turn(payload: &EventPayload) -> Option<u64> {
    match payload {
        EventPayload::ContextSkills { turn, .. }
        | EventPayload::ContextSources { turn, .. }
        | EventPayload::Handoff { turn, .. }
        | EventPayload::ModelError { turn, .. }
        | EventPayload::ModelResponse { turn, .. }
        | EventPayload::PermissionAllow { turn, .. }
        | EventPayload::PermissionDeny { turn, .. }
        | EventPayload::PermissionRequested { turn, .. }
        | EventPayload::ToolCall { turn, .. }
        | EventPayload::ToolResult { turn, .. }
        | EventPayload::TurnEnd { turn, .. }
        | EventPayload::TurnRollback { turn, .. }
        | EventPayload::TurnRollbackUndo { turn, .. }
        | EventPayload::TurnStart { turn, .. }
        | EventPayload::UserInput { turn, .. } => Some(*turn),
        EventPayload::ContextPrelude { .. }
        | EventPayload::SessionMeta { .. }
        | EventPayload::Unknown(_) => None,
    }
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

    fn registry(names: &[&str]) -> SkillRegistry {
        names
            .iter()
            .fold(SkillRegistry::builder(), |builder, name| {
                builder.with_definition(definition(name))
            })
            .build()
    }

    fn event(id: u64, payload: EventPayload) -> StoredEvent {
        StoredEvent { id, payload }
    }

    #[test]
    fn restores_current_and_previous_turn_snapshots() {
        let alpha = registry(&["alpha"]);
        let beta = registry(&["beta"]);
        let events = vec![
            event(
                1,
                EventPayload::ContextSkills {
                    turn: 1,
                    ts: "t1".to_string(),
                    registry: alpha.clone(),
                    bootstrap_loaded: vec!["bootstrap-alpha".to_string()],
                },
            ),
            event(
                2,
                EventPayload::ContextSkills {
                    turn: 2,
                    ts: "t2".to_string(),
                    registry: beta.clone(),
                    bootstrap_loaded: vec!["bootstrap-beta".to_string()],
                },
            ),
        ];

        assert_eq!(
            restore_turn_snapshot(&events, 2),
            Some(TurnSkillSnapshot {
                registry: beta,
                bootstrap_loaded: vec!["bootstrap-beta".to_string()],
            })
        );
        assert_eq!(
            previous_snapshot_before_turn(&events, 2),
            Some(TurnSkillSnapshot {
                registry: alpha,
                bootstrap_loaded: vec!["bootstrap-alpha".to_string()],
            })
        );
    }

    #[test]
    fn ignores_rolled_back_snapshots_when_restoring() {
        let keep = registry(&["keep"]);
        let rollback = registry(&["rollback"]);
        let events = vec![
            event(
                1,
                EventPayload::ContextSkills {
                    turn: 1,
                    ts: "t1".to_string(),
                    registry: keep.clone(),
                    bootstrap_loaded: vec!["keep".to_string()],
                },
            ),
            event(
                2,
                EventPayload::TurnStart {
                    turn: 2,
                    ts: "t2".to_string(),
                },
            ),
            event(
                3,
                EventPayload::ContextSkills {
                    turn: 2,
                    ts: "t3".to_string(),
                    registry: rollback,
                    bootstrap_loaded: vec!["rollback".to_string()],
                },
            ),
            event(
                4,
                EventPayload::TurnRollback {
                    turn: 3,
                    ts: "t4".to_string(),
                    target_turn: 2,
                    scope: RollbackScope::ConversationOnly,
                },
            ),
        ];

        assert_eq!(restore_turn_snapshot(&events, 2), None);
        assert_eq!(
            previous_snapshot_before_turn(&events, 3),
            Some(TurnSkillSnapshot {
                registry: keep,
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
                    tool_call_id: "tool_ok".to_string(),
                    status: "ok".to_string(),
                    summary: "loaded skill: alpha".to_string(),
                    model_content: "alpha".to_string(),
                    truncated: false,
                    structured: None,
                },
            ),
            event(
                4,
                EventPayload::ToolCall {
                    turn: 3,
                    ts: "t4".to_string(),
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
                    tool_call_id: "tool_blocked".to_string(),
                    status: "error".to_string(),
                    summary: "blocked".to_string(),
                    model_content: "blocked".to_string(),
                    truncated: false,
                    structured: None,
                },
            ),
        ];

        assert_eq!(
            loaded_skill_names(&events),
            vec!["alpha".to_string(), "bootstrap-beta".to_string(),]
        );
    }

    #[test]
    fn loaded_skill_names_ignores_blocked_use_skill_results() {
        let events = vec![
            event(
                1,
                EventPayload::ContextSkills {
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
                    tool_call_id: "tool_alpha".to_string(),
                    status: "ok".to_string(),
                    summary: "loaded skill: alpha".to_string(),
                    model_content: "alpha".to_string(),
                    truncated: false,
                    structured: None,
                },
            ),
            event(
                4,
                EventPayload::ToolCall {
                    turn: 4,
                    ts: "t4".to_string(),
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
                    tool_call_id: "tool_beta".to_string(),
                    status: "blocked".to_string(),
                    summary: "blocked".to_string(),
                    model_content: "blocked".to_string(),
                    truncated: false,
                    structured: None,
                },
            ),
        ];

        assert_eq!(loaded_skill_names(&events), vec!["alpha".to_string()]);
    }

    #[test]
    fn loaded_skill_names_ignores_truncated_use_skill_results() {
        let events = vec![
            event(
                1,
                EventPayload::ContextSkills {
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
                    tool_call_id: "tool_alpha".to_string(),
                    status: "ok".to_string(),
                    summary: "loaded skill: alpha".to_string(),
                    model_content: "partial alpha".to_string(),
                    truncated: true,
                    structured: None,
                },
            ),
        ];

        assert!(loaded_skill_names(&events).is_empty());
    }

    #[test]
    fn loaded_skill_names_ignores_error_and_cancelled_use_skill_results() {
        let events = vec![
            event(
                1,
                EventPayload::ContextSkills {
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
                    tool_call_id: "tool_alpha".to_string(),
                    status: "ok".to_string(),
                    summary: "loaded skill: alpha".to_string(),
                    model_content: "alpha".to_string(),
                    truncated: false,
                    structured: None,
                },
            ),
            event(
                4,
                EventPayload::ToolCall {
                    turn: 5,
                    ts: "t4".to_string(),
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
                    tool_call_id: "tool_beta".to_string(),
                    status: "error".to_string(),
                    summary: "failed".to_string(),
                    model_content: "failed".to_string(),
                    truncated: false,
                    structured: None,
                },
            ),
            event(
                6,
                EventPayload::ToolCall {
                    turn: 5,
                    ts: "t6".to_string(),
                    tool_call_id: "tool_gamma".to_string(),
                    request_id: "req_5".to_string(),
                    index: 2,
                    tool: "use_skill".to_string(),
                    args: json!({ "skill_name": "gamma" }),
                },
            ),
        ];

        assert_eq!(loaded_skill_names(&events), vec!["alpha".to_string()]);
    }

    #[test]
    fn loaded_skill_names_ignore_trailing_rollback_admin_events() {
        let events = vec![
            event(
                1,
                EventPayload::ContextSkills {
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
                    tool_call_id: "tool_alpha".to_string(),
                    status: "ok".to_string(),
                    summary: "loaded skill: alpha".to_string(),
                    model_content: "alpha".to_string(),
                    truncated: false,
                    structured: None,
                },
            ),
            event(
                4,
                EventPayload::TurnRollback {
                    turn: 9,
                    ts: "t4".to_string(),
                    target_turn: 2,
                    scope: RollbackScope::ConversationOnly,
                },
            ),
            event(
                5,
                EventPayload::TurnRollbackUndo {
                    turn: 10,
                    ts: "t5".to_string(),
                    rollback_event_id: 4,
                },
            ),
        ];

        assert_eq!(loaded_skill_names(&events), vec!["alpha".to_string()]);
    }

    #[test]
    fn loaded_skill_names_remain_cumulative_across_turns() {
        let events = vec![
            event(
                1,
                EventPayload::ContextSkills {
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
                    tool_call_id: "tool_alpha".to_string(),
                    status: "ok".to_string(),
                    summary: "loaded skill: alpha".to_string(),
                    model_content: "alpha".to_string(),
                    truncated: false,
                    structured: None,
                },
            ),
            event(
                4,
                EventPayload::ContextSkills {
                    turn: 2,
                    ts: "t4".to_string(),
                    registry: registry(&["beta"]),
                    bootstrap_loaded: vec!["beta".to_string()],
                },
            ),
        ];

        assert_eq!(
            loaded_skill_names(&events),
            vec!["alpha".to_string(), "beta".to_string()]
        );
    }
}
