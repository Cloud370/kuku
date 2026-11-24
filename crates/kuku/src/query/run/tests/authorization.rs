use super::{
    RunState, UiEvent, assert_blocked_tool_result, make_queued_run, make_skill_queued_run,
    write_blocking_pre_hook,
};
use crate::permission::PermissionMode;

#[tokio::test]
async fn queued_deny_persists_blocked_tool_result() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let mut run = make_queued_run(events_path.clone(), dir.path());
    if let RunState::Pending(pending) = &mut run.state {
        pending.query.permission_mode = PermissionMode::ReadOnly;
    }

    assert!(matches!(
        run.next().await.unwrap(),
        Some(UiEvent::ToolStart { .. })
    ));
    let event = run.next().await.unwrap();

    assert!(
        matches!(event, Some(UiEvent::ToolEnd { id, status, result, .. }) if id == "tool_queued" && status == "blocked" && result == Some(crate::tool::ToolResultEnvelope::blocked_marker()))
    );
    assert_blocked_tool_result(&events_path, "read-only mode permits reads only");
}

#[tokio::test]
async fn queued_pre_hook_block_persists_blocked_tool_result() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let pkg_dir = dir.path().join(".kuku").join("packages").join("test-hook");
    write_blocking_pre_hook(&pkg_dir, "blocked by hook");
    let mut run = make_queued_run(events_path.clone(), dir.path());
    if let RunState::Pending(pending) = &mut run.state {
        pending.query.permission_mode = PermissionMode::Ask;
        pending.plugin_registry = Some(std::sync::Arc::new(
            crate::plugin::PluginRegistry::builder()
                .load_packages(dir.path(), dir.path())
                .unwrap()
                .build()
                .unwrap(),
        ));
    }

    assert!(matches!(
        run.next().await.unwrap(),
        Some(UiEvent::ToolStart { .. })
    ));
    let event = run.next().await.unwrap();

    assert!(
        matches!(event, Some(UiEvent::ToolEnd { id, status, result, .. }) if id == "tool_queued" && status == "blocked" && result == Some(crate::tool::ToolResultEnvelope::blocked_marker()))
    );
    assert_blocked_tool_result(&events_path, "blocked by hook");
}

#[tokio::test]
async fn inline_skill_tools_do_not_bypass_resolved_registry_membership() {
    let dir = tempfile::tempdir().unwrap();
    let registry = vec![crate::tool::ToolDefinition {
        name: "run_command".to_string(),
        description: "test command".to_string(),
        input_schema: serde_json::json!({"type": "object"}),
        read_only: false,
        max_result_chars: 8000,
        risk: "command".to_string(),
    }];

    for tool_name in ["use_skill", "list_skills", "search_skills"] {
        let events_path = dir.path().join(format!("{tool_name}.jsonl"));
        std::fs::write(&events_path, "").unwrap();
        let mut run = make_skill_queued_run(events_path, dir.path(), registry.clone(), tool_name);

        assert!(matches!(
            run.next().await.unwrap(),
            Some(UiEvent::ToolStart { .. })
        ));
        let event = run.next().await.unwrap();

        assert!(
            matches!(event, Some(UiEvent::ToolEnd { status, summary, .. }) if status == "error" && summary == "unknown tool")
        );
    }
}
