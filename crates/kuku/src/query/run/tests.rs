mod authorization;
mod cancellation;
mod lifecycle;
mod streaming;

use super::{
    PendingPermission, PendingRun, PendingStep, PermissionChoice, QueuedToolCall, Run, RunState,
    StreamingChunkState, UiEvent,
};
use crate::event::{EventPayload, EventStore};
use crate::provider::types::{ProviderKind, ProviderToolCall, ResolvedProvider, SecretString};
use crate::query::types::{
    CumulativeUsage, ExecSlot, PermissionRequest, ResolvedRuntime, SlotEvent, ToolKind,
};

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

fn make_cancelled_run(events_path: std::path::PathBuf, turn: u64) -> Run {
    let (slot_event_tx, slot_event_rx) = tokio::sync::mpsc::channel(16);
    Run {
        agent: crate::conversation::address::ConversationAddress::MAIN,
        cleanup_context: None,
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

pub(super) fn make_test_pending(
    events_path: std::path::PathBuf,
    dir: &std::path::Path,
    cancel_token: std::sync::Arc<tokio::sync::Notify>,
) -> PendingRun {
    PendingRun {
        context_cursor: None,
        session_id: "test".to_string(),
        query: crate::query::types::Query::new("test"),
        conversation: crate::conversation::address::ConversationAddress::MAIN,
        events_path,
        kuku_home: dir.to_path_buf(),
        workspace: dir.to_path_buf(),
        policy_path: dir.join("policy.md"),
        turn: 1,
        request_num: 1,
        cumulative: CumulativeUsage::default(),
        resolved: None,
        admitted_tool_call_ids: Default::default(),
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
        waiting_for_job_input: false,
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
        request_base: None,
        recovery_count: 0,
    }
}

fn tool_stream_state(
    pending: PendingRun,
    chunks: Vec<crate::provider::chunk::ProviderChunk>,
) -> StreamingChunkState {
    StreamingChunkState {
        model_permit: None,
        pending,
        conversation: crate::conversation::address::ConversationAddress::MAIN,
        request_id: "req_tool_stream".to_string(),
        stream: Box::pin(tokio_stream::iter(chunks.into_iter().map(Ok))),
        provider_attempt: 1,
        stream_failure: None,
        accumulated_text: String::new(),
        accumulated_thinking: String::new(),
        model_phase: None,
        provider_replay: Vec::new(),
        stop_reason: None,
        tool_calls: Vec::new(),
        tool_arg_buffers: Vec::new(),
        tool_call_completions: Vec::new(),
        tool_stream_invalid: false,
        terminal_stream_invalid: false,
        stream_ended: false,
        provider_request_id: None,
        usage: None,
        request_timing: None,
        lead_events: Vec::new(),
        handoff_detector: None,
        thinking_start: None,
        thinking_duration_ms: 0,
    }
}

async fn finish_tool_stream(
    events_path: std::path::PathBuf,
    dir: &std::path::Path,
    chunks: Vec<crate::provider::chunk::ProviderChunk>,
) -> crate::error::Result<PendingStep> {
    let cancel_token = std::sync::Arc::new(tokio::sync::Notify::new());
    let pending = make_test_pending(events_path, dir, cancel_token.clone());
    let mut state = tool_stream_state(pending, chunks);
    loop {
        let poll = Run::poll_stream_chunk(&cancel_token, &mut state).await;
        assert!(poll.is_ok(), "stream polling must not fail: {poll:?}");
        if matches!(poll, Ok(None)) {
            break;
        }
    }
    crate::query::step::finish_streaming(state).await
}

fn assert_rejected_tool_response(
    events_path: &std::path::Path,
    expected_stop_reason: crate::event::ModelStopReason,
) {
    let events = EventStore::replay(events_path).unwrap();
    let response_index = events
        .iter()
        .position(|event| matches!(
            &event.payload,
            EventPayload::ModelResponse { stop_reason: Some(reason), .. } if reason == &expected_stop_reason
        ))
        .expect("model.response with the expected terminal reason");
    let interrupted_index = events
        .iter()
        .position(|event| {
            matches!(
                &event.payload,
                EventPayload::TurnInterrupted { reason, .. }
                    if reason == if expected_stop_reason == crate::event::ModelStopReason::Length {
                        "length"
                    } else {
                        "invalid_response"
                    }
            )
        })
        .expect("turn.interrupted after rejected model response");
    assert!(response_index < interrupted_index);
    assert!(!events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ToolCall { .. }
            | EventPayload::PermissionRequested { .. }
            | EventPayload::PermissionAllow { .. }
            | EventPayload::PermissionDeny { .. }
            | EventPayload::ToolResult { .. }
    )));
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
            args: serde_json::json!({"program": "printf", "args": ["hi"], "cwd": ".", "timeout": 60, "brief": "print hi"}),
            index: 0,
        },
        display_summary: "print hi".to_string(),
    });
    let (slot_event_tx, slot_event_rx) = tokio::sync::mpsc::channel(16);
    Run {
        agent: crate::conversation::address::ConversationAddress::MAIN,
        cleanup_context: None,
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
                candidate: crate::tool::builtin::run_command::command_permission_candidate(
                    &serde_json::json!({"program":"printf","args":["hi"],"cwd":"."}),
                ),
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
            max_output_tokens: Some(1000),
            think_level: crate::config::ThinkLevel::Off,
            thinking: crate::config::ResolvedThinking::default(),
            ..Default::default()
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

fn assert_prepared_handoff_after_rewind(target_before_handoff: bool, expect_summary: bool) {
    let directory = tempfile::tempdir().unwrap();
    let events_path = directory.path().join("events.jsonl");
    let mut writer = EventStore::open(&events_path).unwrap();
    writer
        .append(EventPayload::TurnStarted {
            turn: 1,
            ts: "t".into(),
            conversation: "main".into(),
        })
        .unwrap();
    let target = if target_before_handoff {
        Some(
            writer
                .append(EventPayload::MessageUser {
                    conversation: "main".into(),
                    turn: 1,
                    ts: "t".into(),
                    text: "rewound target input".into(),
                    from: None,
                    via_tool_call_id: None,
                })
                .unwrap(),
        )
    } else {
        None
    };
    let handoff = writer
        .append(EventPayload::Handoff {
            turn: 1,
            ts: "t".into(),
            request_id: "handoff".into(),
            summary: "retained handoff summary".into(),
            keep_turns: 1,
        })
        .unwrap();
    let target = match target {
        Some(target) => target,
        None => {
            writer
                .append(EventPayload::TurnStarted {
                    turn: 2,
                    ts: "t".into(),
                    conversation: "main".into(),
                })
                .unwrap();
            writer
                .append(EventPayload::MessageUser {
                    conversation: "main".into(),
                    turn: 2,
                    ts: "t".into(),
                    text: "rewound target input".into(),
                    from: None,
                    via_tool_call_id: None,
                })
                .unwrap()
        }
    };
    let catalog = crate::prompt::builtin_prompt_catalog();
    let old_prelude = crate::context::CanonicalMessage::user_text("retained handoff summary");
    let segment_value = serde_json::json!({
        "kind":"runtime.context_segment", "agent":"main", "signature":"old",
        "configuration":1, "handoff":handoff.id, "system_prompt":"old system",
        "prelude":[old_prelude], "tools":[], "catalog":catalog,
        "connection":"default", "protocol":"openai_compatible", "model":"test-model",
        "endpoint":"https://example.test", "behavior":"", "identity_slot":1,
        "handoff_prelude":0, "workspace":directory.path().display().to_string()
    });
    let segment_event = writer
        .append(EventPayload::Unknown(segment_value.clone()))
        .unwrap();
    writer
        .append(EventPayload::Unknown(serde_json::json!({
            "kind":"runtime.step_prepared", "agent":"main", "segment":segment_event.id,
            "context_head":0, "source_head":target.id
        })))
        .unwrap();
    let rewind = writer
        .append(EventPayload::Unknown(serde_json::json!({
            "kind":"runtime.operation",
            "record":{"operation":"message_rewind","address":"main","target_event_id":target.id}
        })))
        .unwrap();
    writer
        .append(EventPayload::TurnStarted {
            turn: 3,
            ts: "t".into(),
            conversation: "main".into(),
        })
        .unwrap();
    writer
        .append(EventPayload::MessageUser {
            conversation: "main".into(),
            turn: 3,
            ts: "t".into(),
            text: "new input after rewind".into(),
            from: None,
            via_tool_call_id: None,
        })
        .unwrap();
    drop(writer);

    let mut segment: crate::context::runtime::FrozenSegment =
        serde_json::from_value(segment_value).unwrap();
    segment.id = segment_event.id;
    let mut cursor = crate::context::runtime::ContextCursor::default();
    cursor.after = rewind.id;
    cursor.handoff = Some(handoff);
    cursor.segment = Some(segment);
    cursor.observe(rewind, "main", 1024 * 1024).unwrap();
    assert!(cursor.segment.is_none());
    assert_eq!(expect_summary, cursor.handoff.is_some());

    let mut pending = make_test_pending(
        events_path,
        directory.path(),
        std::sync::Arc::new(tokio::sync::Notify::new()),
    );
    pending.turn = 3;
    pending.resolved = Some(test_resolved_runtime());
    pending.context_cursor = Some(Box::new(cursor));
    let prepared = crate::query::frozen::prepare(&mut pending).unwrap();
    let request = prepared.request.request();
    let has_text = |messages: &[crate::context::CanonicalMessage], text: &str| {
        messages.iter().any(|message| {
            message.blocks.iter().any(|block| match block {
                crate::context::MessageBlock::Text(value) => value.contains(text),
                _ => false,
            })
        })
    };
    assert_eq!(
        expect_summary,
        has_text(
            &request.assembly.prelude_messages,
            "retained handoff summary"
        )
    );
    assert!(has_text(
        &request.assembly.history,
        "new input after rewind"
    ));
    assert!(!has_text(&request.assembly.history, "rewound target input"));
}

#[test]
fn prepare_rehydrates_only_handoffs_that_precede_rewind_target() {
    assert_prepared_handoff_after_rewind(false, true);
    assert_prepared_handoff_after_rewind(true, false);
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
            args: serde_json::json!({"program": "printf", "args": ["hi"], "cwd": ".", "timeout": 60, "brief": "print hi"}),
            index: 0,
        },
        display_summary: "print hi".to_string(),
    });
    let (slot_event_tx, slot_event_rx) = tokio::sync::mpsc::channel(16);
    Run {
        agent: crate::conversation::address::ConversationAddress::MAIN,
        cleanup_context: None,
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
        agent: crate::conversation::address::ConversationAddress::MAIN,
        cleanup_context: None,
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
