use std::path::Path;

use crate::config::{
    Config, DiscoveryConfig, HandoffConfig, LogsConfig, PluginConfig, UpdateConfig,
};
use crate::event::{EventPayload, EventStore};
use crate::query::types::RunState;
use crate::query::Query;

fn test_config() -> Config {
    Config {
        tiers: std::collections::BTreeMap::new(),
        providers: std::collections::BTreeMap::new(),
        default_tier: "balanced".to_string(),
        discovery: DiscoveryConfig::default(),
        handoff: HandoffConfig::default(),
        logs: LogsConfig::default(),
        plugin: PluginConfig::default(),
        update: UpdateConfig::default(),
    }
}

fn write_skill(skill_dir: &Path, name: &str, description: &str) {
    std::fs::create_dir_all(skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        format!(
            "---\nname: {name}\ndescription: {description}\n---\n\n# {name}\n\n{description} body\n"
        ),
    )
    .unwrap();
}

#[test]
fn startup_prune_options_exclude_active_runtime_path() {
    let active_path = Path::new("/tmp/kuku/logs/runtime/2026-06-06.jsonl");

    let options = super::startup_prune_options(active_path);

    assert!(options.excludes_active_path(active_path));
}

#[tokio::test]
async fn resumed_turn_restores_persisted_skill_snapshot_instead_of_live_disk() {
    let workspace = tempfile::tempdir().unwrap();
    let kuku_home = tempfile::tempdir().unwrap();
    let config = test_config();
    let session_id = "resume-skills";
    let skill_dir = workspace
        .path()
        .join(".kuku")
        .join("skills")
        .join("resume-skill");

    write_skill(&skill_dir, "resume-skill", "persisted description");
    let persisted_registry =
        crate::skill::session::build_registry_snapshot(workspace.path(), &config.discovery, None)
            .unwrap();

    let events_path =
        crate::session::session_events_path(kuku_home.path(), workspace.path(), session_id)
            .unwrap();
    let mut store = EventStore::open(&events_path).unwrap();
    store
        .append(EventPayload::SessionCreated {
            ts: "2026-06-07T00:00:00Z".to_string(),
            schema_version: 2,
            session_id: session_id.to_string(),
            created_at: "2026-06-07T00:00:00Z".to_string(),
            kuku_version: env!("CARGO_PKG_VERSION").to_string(),
        })
        .unwrap();
    store
        .append(EventPayload::TurnStarted {
            turn: 1,
            ts: "2026-06-07T00:00:01Z".to_string(),
            conversation: "main".to_string(),
        })
        .unwrap();
    store
        .append(EventPayload::MessageUser {
            turn: 1,
            ts: "2026-06-07T00:00:02Z".to_string(),
            conversation: "main".to_string(),
            text: "resume this turn".to_string(),
            from: None,
            via_tool_call_id: None,
        })
        .unwrap();
    store
        .append(EventPayload::ContextSkills {
            conversation: "main".to_string(),
            turn: 1,
            ts: "2026-06-07T00:00:03Z".to_string(),
            registry: serde_json::to_value(&persisted_registry).unwrap(),
            bootstrap_loaded: vec![],
        })
        .unwrap();
    store
        .append(EventPayload::ToolCall {
            turn: 1,
            ts: "2026-06-07T00:00:04Z".to_string(),
            conversation: None,
            tool_call_id: "tool_1".to_string(),
            request_id: "req_1".to_string(),
            index: 0,
            tool: "write".to_string(),
            args: serde_json::json!({ "path": "foo.txt" }),
        })
        .unwrap();
    store
        .append(EventPayload::PermissionRequested {
            turn: 1,
            ts: "2026-06-07T00:00:05Z".to_string(),
            tool_call_id: "tool_1".to_string(),
            tool: "write".to_string(),
            risk: "modifies_files".to_string(),
            summary: "write foo.txt".to_string(),
            candidate: "foo.txt".to_string(),
            source: "tool_policy".to_string(),
        })
        .unwrap();

    write_skill(&skill_dir, "resume-skill", "mutated live description");

    let mut query = Query::new("ignored")
        .session(session_id)
        .workspace(workspace.path())
        .config(config);
    query.captured_kuku_home = Some(kuku_home.path().to_path_buf());

    let mut run = query.start().await.unwrap();

    let RunState::WaitingForPermission(ref mut waiting) = run.state else {
        panic!("expected resumed waiting state, got {:?}", run.state);
    };
    let skill_registry = waiting
        .pending
        .skill_registry
        .as_ref()
        .expect("restored skill snapshot");
    let skill = skill_registry
        .get("resume-skill")
        .expect("persisted skill should exist");
    assert_eq!(skill.description, "persisted description");

    let use_skill = crate::provider::types::ProviderToolCall {
        id: "tool_use_skill".to_string(),
        name: "use_skill".to_string(),
        args: serde_json::json!({ "skill_name": "resume-skill" }),
        index: 1,
    };
    let result = crate::query::tool_exec::execute_tool_call(&mut waiting.pending, &use_skill)
        .await
        .unwrap();
    assert_eq!(result.status, "ok");
    assert!(result.model_content.contains("persisted description body"));
    assert!(!result
        .model_content
        .contains("mutated live description body"));
}

#[tokio::test]
async fn resumed_turn_ignores_new_bootstrap_skill_input_and_restores_snapshot() {
    let workspace = tempfile::tempdir().unwrap();
    let kuku_home = tempfile::tempdir().unwrap();
    let config = test_config();
    let session_id = "resume-bootstrap";
    let skill_dir = workspace
        .path()
        .join(".kuku")
        .join("skills")
        .join("resume-skill");

    write_skill(&skill_dir, "resume-skill", "resume description");
    let persisted_registry =
        crate::skill::session::build_registry_snapshot(workspace.path(), &config.discovery, None)
            .unwrap();

    let events_path =
        crate::session::session_events_path(kuku_home.path(), workspace.path(), session_id)
            .unwrap();
    let mut store = EventStore::open(&events_path).unwrap();
    store
        .append(EventPayload::SessionCreated {
            ts: "2026-06-07T00:00:00Z".to_string(),
            schema_version: 2,
            session_id: session_id.to_string(),
            created_at: "2026-06-07T00:00:00Z".to_string(),
            kuku_version: env!("CARGO_PKG_VERSION").to_string(),
        })
        .unwrap();
    store
        .append(EventPayload::TurnStarted {
            turn: 1,
            ts: "2026-06-07T00:00:01Z".to_string(),
            conversation: "main".to_string(),
        })
        .unwrap();
    store
        .append(EventPayload::MessageUser {
            turn: 1,
            ts: "2026-06-07T00:00:02Z".to_string(),
            conversation: "main".to_string(),
            text: "resume this turn".to_string(),
            from: None,
            via_tool_call_id: None,
        })
        .unwrap();
    store
        .append(EventPayload::ContextSkills {
            conversation: "main".to_string(),
            turn: 1,
            ts: "2026-06-07T00:00:02Z".to_string(),
            registry: serde_json::to_value(&persisted_registry).unwrap(),
            bootstrap_loaded: vec!["resume-skill".to_string()],
        })
        .unwrap();
    store
        .append(EventPayload::ToolCall {
            turn: 1,
            ts: "2026-06-07T00:00:03Z".to_string(),
            conversation: None,
            tool_call_id: "tool_1".to_string(),
            request_id: "req_1".to_string(),
            index: 0,
            tool: "write".to_string(),
            args: serde_json::json!({ "path": "foo.txt" }),
        })
        .unwrap();
    store
        .append(EventPayload::PermissionRequested {
            turn: 1,
            ts: "2026-06-07T00:00:04Z".to_string(),
            tool_call_id: "tool_1".to_string(),
            tool: "write".to_string(),
            risk: "modifies_files".to_string(),
            summary: "write foo.txt".to_string(),
            candidate: "foo.txt".to_string(),
            source: "tool_policy".to_string(),
        })
        .unwrap();

    let mut query = Query::new("ignored")
        .session(session_id)
        .workspace(workspace.path())
        .config(config)
        .bootstrap_skill(
            "bootstrap-skill",
            "<!-- loaded: /skills/bootstrap-skill -->\n\nbootstrap body".to_string(),
        );
    query.captured_kuku_home = Some(kuku_home.path().to_path_buf());

    let run = query.start().await.unwrap();

    let RunState::WaitingForPermission(ref waiting) = run.state else {
        panic!("expected resumed waiting state");
    };
    let bootstrap_skill = waiting
        .pending
        .bootstrap_skill
        .as_ref()
        .expect("restored bootstrap skill");
    assert_eq!(waiting.pending.turn, 1);
    assert_eq!(bootstrap_skill.name.as_deref(), Some("resume-skill"));
    assert!(bootstrap_skill.body.contains("resume description body"));
    assert!(!bootstrap_skill.body.contains("bootstrap body"));
}

#[tokio::test]
async fn resumed_turn_restores_bootstrap_skill_body_from_snapshot() {
    let workspace = tempfile::tempdir().unwrap();
    let kuku_home = tempfile::tempdir().unwrap();
    let config = test_config();
    let session_id = "resume-bootstrap-body";
    let skill_dir = workspace
        .path()
        .join(".kuku")
        .join("skills")
        .join("resume-skill");

    write_skill(&skill_dir, "resume-skill", "resume description");
    let persisted_registry =
        crate::skill::session::build_registry_snapshot(workspace.path(), &config.discovery, None)
            .unwrap();

    let events_path =
        crate::session::session_events_path(kuku_home.path(), workspace.path(), session_id)
            .unwrap();
    let mut store = EventStore::open(&events_path).unwrap();
    store
        .append(EventPayload::SessionCreated {
            ts: "2026-06-07T00:00:00Z".to_string(),
            schema_version: 2,
            session_id: session_id.to_string(),
            created_at: "2026-06-07T00:00:00Z".to_string(),
            kuku_version: env!("CARGO_PKG_VERSION").to_string(),
        })
        .unwrap();
    store
        .append(EventPayload::TurnStarted {
            turn: 1,
            ts: "2026-06-07T00:00:01Z".to_string(),
            conversation: "main".to_string(),
        })
        .unwrap();
    store
        .append(EventPayload::MessageUser {
            turn: 1,
            ts: "2026-06-07T00:00:02Z".to_string(),
            conversation: "main".to_string(),
            text: "resume this turn".to_string(),
            from: None,
            via_tool_call_id: None,
        })
        .unwrap();
    store
        .append(EventPayload::ContextSkills {
            conversation: "main".to_string(),
            turn: 1,
            ts: "2026-06-07T00:00:03Z".to_string(),
            registry: serde_json::to_value(&persisted_registry).unwrap(),
            bootstrap_loaded: vec!["resume-skill".to_string()],
        })
        .unwrap();
    store
        .append(EventPayload::ToolCall {
            turn: 1,
            ts: "2026-06-07T00:00:04Z".to_string(),
            conversation: None,
            tool_call_id: "tool_1".to_string(),
            request_id: "req_1".to_string(),
            index: 0,
            tool: "write".to_string(),
            args: serde_json::json!({ "path": "foo.txt" }),
        })
        .unwrap();
    store
        .append(EventPayload::PermissionRequested {
            turn: 1,
            ts: "2026-06-07T00:00:05Z".to_string(),
            tool_call_id: "tool_1".to_string(),
            tool: "write".to_string(),
            risk: "modifies_files".to_string(),
            summary: "write foo.txt".to_string(),
            candidate: "foo.txt".to_string(),
            source: "tool_policy".to_string(),
        })
        .unwrap();

    let mut query = Query::new("ignored")
        .session(session_id)
        .workspace(workspace.path())
        .config(config);
    query.captured_kuku_home = Some(kuku_home.path().to_path_buf());

    let run = query.start().await.unwrap();

    let RunState::WaitingForPermission(ref waiting) = run.state else {
        panic!("expected resumed waiting state");
    };
    let bootstrap_skill = waiting
        .pending
        .bootstrap_skill
        .as_ref()
        .expect("restored bootstrap skill");
    assert_eq!(bootstrap_skill.name.as_deref(), Some("resume-skill"));
    assert!(bootstrap_skill.body.contains("resume description body"));
}

#[tokio::test]
async fn fresh_turn_persists_named_bootstrap_skill_loads() {
    let workspace = tempfile::tempdir().unwrap();
    let kuku_home = tempfile::tempdir().unwrap();
    let config = test_config();
    let skill_dir = workspace
        .path()
        .join(".kuku")
        .join("skills")
        .join("bootstrap-skill");

    write_skill(&skill_dir, "bootstrap-skill", "bootstrap description");

    let mut query = Query::new("bootstrap this turn")
        .workspace(workspace.path())
        .config(config)
        .bootstrap_skill(
            "bootstrap-skill",
            "<!-- loaded: /skills/bootstrap-skill -->\n\nbootstrap body".to_string(),
        );
    query.captured_kuku_home = Some(kuku_home.path().to_path_buf());

    let run = query.start().await.unwrap();
    let session_id = run.session_id().to_string();
    drop(run);

    let events_path =
        crate::session::session_events_path(kuku_home.path(), workspace.path(), &session_id)
            .unwrap();
    let events = EventStore::replay(&events_path).unwrap();
    let context_skills = events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::ContextSkills {
                bootstrap_loaded, ..
            } => Some(bootstrap_loaded.clone()),
            _ => None,
        })
        .expect("context.skills event");

    assert_eq!(context_skills, vec!["bootstrap-skill".to_string()]);
    assert_eq!(
        crate::skill::session::loaded_skill_names(&events, "main"),
        vec!["bootstrap-skill".to_string()]
    );
}

#[tokio::test]
async fn fresh_turn_excludes_package_skills_when_plugins_are_disabled() {
    let workspace = tempfile::tempdir().unwrap();
    let kuku_home = tempfile::tempdir().unwrap();
    let mut config = test_config();
    config.plugin.enabled = false;

    let package_root = workspace
        .path()
        .join(".kuku")
        .join("packages")
        .join("pkg-with-skill");
    std::fs::create_dir_all(package_root.join("skills").join("packaged-skill")).unwrap();
    std::fs::write(
        package_root.join("kuku.toml"),
        "[package]\nname = \"pkg-with-skill\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    std::fs::write(
        package_root
            .join("skills")
            .join("packaged-skill")
            .join("SKILL.md"),
        "---\nname: packaged-skill\ndescription: From package\n---\n\n# Packaged\n\npackage skill body\n",
    )
    .unwrap();

    let mut query = Query::new("show skills")
        .workspace(workspace.path())
        .config(config);
    query.captured_kuku_home = Some(kuku_home.path().to_path_buf());

    let run = query.start().await.unwrap();
    let session_id = run.session_id().to_string();
    drop(run);

    let events_path =
        crate::session::session_events_path(kuku_home.path(), workspace.path(), &session_id)
            .unwrap();
    let events = EventStore::replay(&events_path).unwrap();
    let registry: crate::skill::registry::SkillRegistry = events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::ContextSkills { registry, .. } => Some(
                serde_json::from_value::<crate::skill::registry::SkillRegistry>(registry.clone())
                    .unwrap(),
            ),
            _ => None,
        })
        .expect("context.skills event");

    assert!(registry.get("packaged-skill").is_none());
}

#[tokio::test]
async fn locked_session_does_not_write_new_turn_events() {
    let workspace = tempfile::tempdir().unwrap();
    let kuku_home = tempfile::tempdir().unwrap();
    let session_id = "locked-session";
    let lock_path =
        crate::session::session_lock_path(kuku_home.path(), workspace.path(), session_id);
    let lock_content = format!("{}\n2026-06-07T00:00:00Z\n", std::process::id());
    std::fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
    std::fs::write(&lock_path, lock_content).unwrap();

    let mut query = Query::new("should not persist")
        .session(session_id)
        .workspace(workspace.path())
        .config(test_config());
    query.captured_kuku_home = Some(kuku_home.path().to_path_buf());

    let error = query.start().await.unwrap_err();
    assert!(matches!(error, crate::error::Error::SessionLocked { .. }));

    let events_path =
        crate::session::session_events_path(kuku_home.path(), workspace.path(), session_id)
            .unwrap();
    let events = EventStore::replay(&events_path).unwrap();
    assert!(
        events.is_empty(),
        "locked session start wrote events before failing"
    );
}
