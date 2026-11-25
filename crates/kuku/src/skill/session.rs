use std::collections::{BTreeSet, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::context::{CanonicalMessage, MessageBlock};
use crate::error::{Error, Result};

use crate::config::DiscoveryConfig;
use crate::context::revert::filter_rolled_back_events;
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
    Ok(builder.with_docs().build())
}

/// Render the exact material admitted by both explicit selection and use_skill.
pub(crate) fn admission_body(registry: &SkillRegistry, name: &str) -> Option<String> {
    let def = registry.get(name)?;
    let location = def
        .source_path
        .clone()
        .unwrap_or_else(|| format!("{}:{}", def.source.as_str(), def.name));
    let bundle = (def.source == super::definition::SkillSource::Builtin)
        .then(|| registry.docs_bundle())
        .flatten()
        .unwrap_or("");
    Some(format!(
        "<!-- loaded: {location} -->\n<!-- skill-revision: {} {} {bundle} -->\n\n{}",
        def.source.as_str(),
        def.hash,
        def.instructions
    ))
}

pub(crate) fn has_admission(message: &CanonicalMessage, body: &str) -> bool {
    message.blocks.iter().any(|block| match block {
        MessageBlock::Text(text) => {
            text == body
                || text
                    .match_indices("<kuku_skill_input>")
                    .any(|(offset, tag)| {
                        let rest = &text[offset + tag.len()..];
                        rest.strip_prefix(body)
                            .is_some_and(|end| end.starts_with("</kuku_skill_input>"))
                    })
        }
        // The provider replay intentionally strips most structured tool facts.
        // Match the actual full body rather than metadata erased by that projection.
        MessageBlock::ToolResult(result) => {
            result.status == "ok" && !result.truncated && result.model_content == body
        }
        _ => false,
    })
}

fn catalog_path(home: &Path, digest: &str) -> PathBuf {
    home.join("skill-catalogs").join(format!("{digest}.json"))
}

/// Record just a reference in the journal. The immutable snapshot is shared by
/// unchanged Turns; reopening a Turn never reads changed workspace files in its place.
pub(crate) fn persist_catalog(home: &Path, registry: &SkillRegistry) -> Result<Value> {
    let bytes = serde_json::to_vec(registry)?;
    let digest = format!("{:x}", Sha256::digest(&bytes));
    let path = catalog_path(home, &digest);
    if !path.is_file() {
        let parent = path.parent().expect("catalog has a parent");
        crate::util::private_directory(parent)?;
        let mut staged = tempfile::NamedTempFile::new_in(parent)?;
        staged.write_all(&bytes)?;
        staged.as_file().sync_all()?;
        match staged.persist_noclobber(&path) {
            Ok(_) => {
                #[cfg(unix)]
                std::fs::File::open(parent)?.sync_all()?;
            }
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.error.into()),
        }
    }
    Ok(json!({"snapshot": digest}))
}

fn read_catalog(home: &Path, record: &Value) -> Result<SkillRegistry> {
    let Some(digest) = record.get("snapshot") else {
        return Ok(serde_json::from_value(record.clone())?); // Legacy inline snapshots.
    };
    let digest = digest
        .as_str()
        .filter(|value| value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit()))
        .ok_or_else(|| Error::InvalidEventStream("invalid Skill catalog reference".into()))?;
    let bytes = std::fs::read(catalog_path(home, digest))?;
    if format!("{:x}", Sha256::digest(&bytes)) != digest {
        return Err(Error::InvalidEventStream(
            "Skill catalog failed integrity check".into(),
        ));
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn prepared_input_skills(value: &serde_json::Value) -> impl Iterator<Item = &str> {
    value["input_skills"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
}

pub(crate) fn restore_turn_snapshot(
    home: &Path,
    events: &[StoredEvent],
    conversation: &str,
    turn: u64,
) -> Result<Option<TurnSkillSnapshot>> {
    find_snapshot(home, events, conversation, |candidate| candidate == turn)
}

pub(crate) fn previous_snapshot_before_turn(
    home: &Path,
    events: &[StoredEvent],
    conversation: &str,
    turn: u64,
) -> Result<Option<TurnSkillSnapshot>> {
    find_snapshot(home, events, conversation, |candidate| candidate < turn)
}

fn find_snapshot(
    home: &Path,
    events: &[StoredEvent],
    conversation: &str,
    matches_turn: impl Fn(u64) -> bool,
) -> Result<Option<TurnSkillSnapshot>> {
    let filtered = filter_rolled_back_events(events);
    for event in filtered.into_iter().rev() {
        if let EventPayload::ContextSkills {
            conversation: agent,
            turn,
            registry,
            bootstrap_loaded,
            ..
        } = &event.payload
            && agent == conversation
            && matches_turn(*turn)
        {
            return Ok(Some(TurnSkillSnapshot {
                conversation: agent.clone(),
                registry: read_catalog(home, registry)?,
                bootstrap_loaded: bootstrap_loaded.clone(),
            }));
        }
    }
    Ok(None)
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
            EventPayload::Unknown(value)
                if value["kind"] == "runtime.step_prepared" && value["agent"] == conversation =>
            {
                loaded.extend(prepared_input_skills(value).map(str::to_owned));
            }
            _ => {}
        }
    }

    loaded.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
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
            restore_turn_snapshot(Path::new("unused"), &events, "main", 2).unwrap(),
            Some(TurnSkillSnapshot {
                conversation: "main".to_string(),
                registry: skill_registry(&["beta"]),
                bootstrap_loaded: vec!["bootstrap-beta".to_string()],
            })
        );
        assert_eq!(
            previous_snapshot_before_turn(Path::new("unused"), &events, "main", 2).unwrap(),
            Some(TurnSkillSnapshot {
                conversation: "main".to_string(),
                registry: skill_registry(&["alpha"]),
                bootstrap_loaded: vec!["bootstrap-alpha".to_string()],
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
