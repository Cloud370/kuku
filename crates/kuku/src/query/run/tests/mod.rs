use super::{PendingPermission, PermissionRequest, QueuedToolCall, Run, RunState};
use crate::config::SecretString;
use crate::event::{EventPayload, EventStore};
use crate::provider::types::{ProviderKind, ProviderToolCall, ResolvedProvider};
use crate::query::types::{CumulativeUsage, PendingRun, ResolvedRuntime};

fn test_config() -> crate::config::Config {
    crate::config::Config {
        tiers: std::collections::BTreeMap::new(),
        providers: std::collections::BTreeMap::new(),
        default_tier: "balanced".to_string(),
        discovery: crate::config::DiscoveryConfig::default(),
        handoff: crate::config::HandoffConfig::default(),
        logs: crate::config::LogsConfig::default(),
        plugin: crate::config::PluginConfig::default(),
        update: crate::config::UpdateConfig::default(),
    }
}

fn test_execution_scope() -> crate::event::ExecutionScope {
    crate::event::ExecutionScope {
        workspace_id: crate::event::WorkspaceId::try_new().unwrap(),
        task_id: crate::event::TaskId::try_new().unwrap(),
        run_id: crate::event::RunId::try_new().unwrap(),
        turn_id: crate::event::TurnId::try_new().unwrap(),
        conversation_id: crate::event::ConversationId::try_new().unwrap(),
        turn_index: 1,
    }
}

fn make_cancelled_run(events_path: std::path::PathBuf, turn: u64) -> Run {
    let (slot_event_tx, slot_event_rx) = tokio::sync::mpsc::channel(16);
    Run {
        execution_scope: test_execution_scope(),
        session_id: "test".to_string(),
        state: RunState::Cancelled {
            events_path: events_path.clone(),
            turn,
        },
        slots: std::collections::HashMap::new(),
        slot_event_tx,
        slot_event_rx,
        cancel_token: std::sync::Arc::new(tokio::sync::Notify::new()),
        lock_path: std::path::PathBuf::new(),
        deferred_runtime_logs: std::collections::VecDeque::new(),
    }
}

fn make_test_pending(
    events_path: std::path::PathBuf,
    dir: &std::path::Path,
    cancel_token: std::sync::Arc<tokio::sync::Notify>,
) -> PendingRun {
    PendingRun {
        session_id: "test".to_string(),
        query: crate::query::types::Query::new("test").execution_scope(test_execution_scope()),
        conversation: crate::conversation::address::ConversationAddress::MAIN,
        events_path,
        kuku_home: dir.to_path_buf(),
        workspace: dir.to_path_buf(),
        policy_path: dir.join("policy.md"),
        turn: 1,
        request_num: 1,
        cumulative: CumulativeUsage::default(),
        resolved: None,
        queued_tool_calls: std::collections::VecDeque::new(),
        resumed_permission_requests: std::collections::VecDeque::new(),
        config: std::sync::Arc::new(test_config()),
        prompts_dir: None,
        agent_registry: None,
        skill_registry: None,
        previous_skill_registry: None,
        bootstrap_skill: None,
        frozen_turn_prefix: crate::query::types::TurnPrefixFreeze::default(),
        child_session_count: 0,
        agent_binding_id: None,
        tool_registry_override: None,
        pending_events: std::collections::VecDeque::new(),
        pending_error: None,
        catalog: crate::prompt::builtin_prompt_catalog(),
        cancel_token,
        handoff_triggered: false,
        handoff_keep_turns: test_config().handoff().keep_turns,
        plugin_registry: None,
        hook_context: Vec::new(),
        force_continue_count: 0,
        model_request_count: 0,
        thinking_duration_ms: 0,
        tool_rounds: 0,
        tool_calls: 0,
        tool_names: Vec::new(),
        tool_denied: 0,
        tool_errors: 0,
        runtime_log_writer: crate::log::BufferedLogWriter::new(dir.join("runtime.jsonl")),
    }
}

fn make_waiting_run(
    events_path: std::path::PathBuf,
    dir: &std::path::Path,
    request_id: &str,
    request_tool_call_id: &str,
    queued_tool_call_id: &str,
) -> Run {
    let pending = make_test_pending(
        events_path,
        dir,
        std::sync::Arc::new(tokio::sync::Notify::new()),
    );
    let mut pending = pending;
    pending.queued_tool_calls.push_back(QueuedToolCall {
        tool_call: ProviderToolCall {
            id: queued_tool_call_id.to_string(),
            name: "run_command".to_string(),
            args: serde_json::json!({"command": "printf hi", "timeout": 60, "brief": "print hi"}),
            index: 0,
        },
        display_summary: "print hi".to_string(),
    });
    let (slot_event_tx, slot_event_rx) = tokio::sync::mpsc::channel(16);
    Run {
        execution_scope: test_execution_scope(),
        session_id: "test".to_string(),
        state: RunState::WaitingForPermission(Box::new(PendingPermission {
            pending,
            request: PermissionRequest {
                id: request_id.to_string(),
                conversation: crate::conversation::address::ConversationAddress::MAIN,
                turn: 1,
                tool_call_id: request_tool_call_id.to_string(),
                tool: "run_command".to_string(),
                risk: "command".to_string(),
                summary: "print hi".to_string(),
                candidate: "printf hi".to_string(),
                source: "default_ask".to_string(),
            },
        })),
        slots: std::collections::HashMap::new(),
        slot_event_tx,
        slot_event_rx,
        cancel_token: std::sync::Arc::new(tokio::sync::Notify::new()),
        lock_path: std::path::PathBuf::new(),
        deferred_runtime_logs: std::collections::VecDeque::new(),
    }
}

fn test_resolved_runtime() -> ResolvedRuntime {
    ResolvedRuntime {
        config: ResolvedProvider {
            kind: ProviderKind::OpenAiCompatible,
            model: "test-model".to_string(),
            base_url: "https://example.test".to_string(),
            api_key: SecretString::new("test-key"),
            max_context_tokens: 1000,
            max_output_tokens: 1000,
            think_level: crate::config::ThinkLevel::Off,
            thinking: crate::config::ResolvedThinking::default(),
        },
        registry: vec![crate::tool::ToolDefinition {
            name: "run_command".to_string(),
            description: "test command".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
            read_only: false,
            max_result_chars: 8000,
            risk: "command".to_string(),
        }],
    }
}

fn make_queued_run(events_path: std::path::PathBuf, dir: &std::path::Path) -> Run {
    let mut pending = make_test_pending(
        events_path,
        dir,
        std::sync::Arc::new(tokio::sync::Notify::new()),
    );
    pending.resolved = Some(test_resolved_runtime());
    pending.queued_tool_calls.push_back(QueuedToolCall {
        tool_call: ProviderToolCall {
            id: "tool_queued".to_string(),
            name: "run_command".to_string(),
            args: serde_json::json!({"command": "printf hi", "timeout": 60, "brief": "print hi"}),
            index: 0,
        },
        display_summary: "print hi".to_string(),
    });
    let (slot_event_tx, slot_event_rx) = tokio::sync::mpsc::channel(16);
    Run {
        execution_scope: test_execution_scope(),
        session_id: "test".to_string(),
        state: RunState::Pending(Box::new(pending)),
        slots: std::collections::HashMap::new(),
        slot_event_tx,
        slot_event_rx,
        cancel_token: std::sync::Arc::new(tokio::sync::Notify::new()),
        lock_path: std::path::PathBuf::new(),
        deferred_runtime_logs: std::collections::VecDeque::new(),
    }
}

fn make_skill_registry() -> crate::skill::registry::SkillRegistry {
    let mut definition = crate::skill::definition::SkillDefinition {
        name: "review".to_string(),
        description: "Review code".to_string(),
        instructions: "Review carefully.".to_string(),
        source: crate::skill::definition::SkillSource::Project,
        hash: String::new(),
        source_path: Some("/skills/review".to_string()),
        allowed_tools: None,
        disallowed_tools: None,
        max_turns: None,
        model: None,
        license: None,
        compatibility: None,
        metadata: serde_json::Value::Null,
    };
    definition.hash = definition.compute_hash();
    crate::skill::registry::SkillRegistry::builder()
        .with_definition(definition)
        .build()
}

fn make_skill_queued_run(
    events_path: std::path::PathBuf,
    dir: &std::path::Path,
    registry: Vec<crate::tool::ToolDefinition>,
    tool_name: &str,
) -> Run {
    let mut pending = make_test_pending(
        events_path,
        dir,
        std::sync::Arc::new(tokio::sync::Notify::new()),
    );
    pending.resolved = Some(ResolvedRuntime {
        config: test_resolved_runtime().config,
        registry,
    });
    pending.skill_registry = Some(make_skill_registry());
    pending.queued_tool_calls.push_back(QueuedToolCall {
        tool_call: ProviderToolCall {
            id: "tool_skill".to_string(),
            name: tool_name.to_string(),
            args: serde_json::json!({"skill_name": "review", "query": "review"}),
            index: 0,
        },
        display_summary: "review".to_string(),
    });
    let (slot_event_tx, slot_event_rx) = tokio::sync::mpsc::channel(16);
    Run {
        execution_scope: test_execution_scope(),
        session_id: "test".to_string(),
        state: RunState::Pending(Box::new(pending)),
        slots: std::collections::HashMap::new(),
        slot_event_tx,
        slot_event_rx,
        cancel_token: std::sync::Arc::new(tokio::sync::Notify::new()),
        lock_path: std::path::PathBuf::new(),
        deferred_runtime_logs: std::collections::VecDeque::new(),
    }
}

fn assert_blocked_tool_result(events_path: &std::path::Path, summary: &str) {
    let events = EventStore::replay(events_path).unwrap();
    let blocked = crate::tool::ToolResultEnvelope::blocked_marker();
    assert!(events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ToolResult {
            tool_call_id,
            status,
            summary: stored_summary,
            model_content,
            structured,
            ..
        } if tool_call_id == "tool_queued"
            && status == "blocked"
            && stored_summary == summary
            && model_content.is_empty()
            && structured.as_ref() == Some(&blocked)
    )));
}

fn write_blocking_pre_hook(pkg_dir: &std::path::Path, stderr_message: &str) {
    std::fs::create_dir_all(pkg_dir.join("hooks")).unwrap();

    #[cfg(windows)]
    let (command, hook_path, hook_body) = (
        "hooks/block.cmd",
        pkg_dir.join("hooks").join("block.cmd"),
        format!("@echo off\r\n<nul set /p ={stderr_message} 1>&2\r\nexit /b 2\r\n"),
    );

    #[cfg(not(windows))]
    let (command, hook_path, hook_body) = (
        "hooks/block.sh",
        pkg_dir.join("hooks").join("block.sh"),
        format!("#!/bin/sh\nprintf '{stderr_message}' >&2\nexit 2\n"),
    );

    std::fs::write(
        pkg_dir.join("kuku.toml"),
        format!(
            "[package]\nname = \"test-hook\"\nversion = \"1.0.0\"\n\n[[hooks]]\nevent = \"tool.pre_execute\"\ncommand = \"{command}\"\n",
        ),
    )
    .unwrap();
    std::fs::write(&hook_path, hook_body).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&hook_path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&hook_path, permissions).unwrap();
    }
}

mod lifecycle;
mod permissions;
mod streaming;
