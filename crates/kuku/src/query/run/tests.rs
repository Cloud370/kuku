use super::*;
use crate::event::{EventPayload, EventStore};
use crate::provider::types::{ProviderKind, ProviderToolCall, ResolvedProvider, SecretString};
use crate::query::types::{CumulativeUsage, ExecSlot, ResolvedRuntime, ToolKind};

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

#[tokio::test]
async fn queued_deny_persists_blocked_tool_result() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    std::fs::write(
        dir.path().join("policy.md"),
        "# policy\n\n## allow\n\n## deny\n- run_command(printf hi)\n",
    )
    .unwrap();
    let mut run = make_queued_run(events_path.clone(), dir.path());

    let event = run.next().await.unwrap();

    assert!(
        matches!(event, Some(UiEvent::ToolEnd { id, status, result, .. }) if id == "tool_queued" && status == "blocked" && result == Some(crate::tool::ToolResultEnvelope::blocked_marker()))
    );
    assert_blocked_tool_result(&events_path, "permission denied");
}

#[tokio::test]
async fn queued_pre_hook_block_persists_blocked_tool_result() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    std::fs::write(
        dir.path().join("policy.md"),
        "# policy\n\n## allow\n- run_command(printf hi)\n\n## deny\n",
    )
    .unwrap();
    let pkg_dir = dir.path().join(".kuku").join("packages").join("test-hook");
    write_blocking_pre_hook(&pkg_dir, "blocked by hook");
    let mut run = make_queued_run(events_path.clone(), dir.path());
    if let RunState::Pending(pending) = &mut run.state {
        pending.plugin_registry = Some(std::sync::Arc::new(
            crate::plugin::PluginRegistry::builder()
                .load_packages(dir.path(), dir.path())
                .unwrap()
                .build()
                .unwrap(),
        ));
    }

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

        let event = run.next().await.unwrap();

        assert!(
            matches!(event, Some(UiEvent::Error { code, message }) if code == "unknown_tool" && message == format!("unknown tool: {tool_name}"))
        );
    }
}

#[tokio::test]
async fn decide_pre_hook_block_persists_blocked_tool_result() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    std::fs::write(dir.path().join("policy.md"), "# policy\n").unwrap();
    let pkg_dir = dir.path().join(".kuku").join("packages").join("test-hook");
    write_blocking_pre_hook(&pkg_dir, "blocked after allow");
    let mut run = make_queued_run(events_path.clone(), dir.path());
    let waiting = match std::mem::replace(&mut run.state, RunState::Done(None)) {
        RunState::Pending(mut pending) => {
            pending.plugin_registry = Some(std::sync::Arc::new(
                crate::plugin::PluginRegistry::builder()
                    .load_packages(dir.path(), dir.path())
                    .unwrap()
                    .build()
                    .unwrap(),
            ));
            PendingPermission {
                request: PermissionRequest {
                    id: "tool_queued".to_string(),
                    conversation: crate::conversation::address::ConversationAddress::MAIN,
                    turn: 1,
                    tool_call_id: "tool_queued".to_string(),
                    tool: "run_command".to_string(),
                    risk: "command".to_string(),
                    summary: "print hi".to_string(),
                    candidate: "printf hi".to_string(),
                    source: "default_ask".to_string(),
                },
                pending: *pending,
            }
        }
        other => panic!("expected pending run, got {other:?}"),
    };
    run.state = RunState::WaitingForPermission(Box::new(waiting));

    let event = run
        .decide("tool_queued", PermissionChoice::Once, None)
        .await
        .unwrap();

    assert!(
        matches!(event, Some(UiEvent::ToolEnd { id, status, result, .. }) if id == "tool_queued" && status == "blocked" && result == Some(crate::tool::ToolResultEnvelope::blocked_marker()))
    );
    assert_blocked_tool_result(&events_path, "blocked after allow");
}

#[tokio::test]
async fn cancel_when_idle_produces_turn_end() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    {
        let mut store = EventStore::open(&events_path).unwrap();
        store
            .append(EventPayload::SessionCreated {
                ts: "2026-05-20T00:00:00Z".to_string(),
                schema_version: 2,
                session_id: "test".to_string(),
                created_at: "2026-05-20T00:00:00Z".to_string(),
                kuku_version: "0.1.0".to_string(),
            })
            .unwrap();
        store
            .append(EventPayload::TurnStarted {
                turn: 1,
                ts: "2026-05-20T00:00:00Z".to_string(),
                conversation: "main".to_string(),
            })
            .unwrap();
    }

    let mut run = make_cancelled_run(events_path.clone(), 1);
    let result = run.next().await.unwrap();
    assert!(matches!(result, Some(UiEvent::Cancelled { turn: 1 })));
    let result = run.next().await.unwrap();
    assert!(result.is_none());

    let events = EventStore::replay(&events_path).unwrap();
    let last = events.last().unwrap();
    assert!(matches!(
        &last.payload,
        EventPayload::TurnCancelled { turn: 1, .. }
    ));
}

#[tokio::test]
async fn cancel_sets_token_and_transitions_state() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let cancel_token = std::sync::Arc::new(tokio::sync::Notify::new());
    let (slot_event_tx, slot_event_rx) = tokio::sync::mpsc::channel(16);
    let run = Run {
        session_id: "test".to_string(),
        state: RunState::Cancelled {
            events_path: events_path.clone(),
            turn: 1,
        },
        slots: std::collections::HashMap::new(),
        slot_event_tx,
        slot_event_rx,
        cancel_token: cancel_token.clone(),
        lock_path: std::path::PathBuf::new(),
        deferred_runtime_logs: std::collections::VecDeque::new(),
    };

    cancel_token.notify_waiters();
    assert!(matches!(&run.state, RunState::Cancelled { .. }));
}

#[test]
fn runtime_log_emit_fans_out_before_best_effort_persistence_failure() {
    let dir = tempfile::tempdir().unwrap();
    let mut pending = make_test_pending(
        dir.path().join("events.jsonl"),
        dir.path(),
        std::sync::Arc::new(tokio::sync::Notify::new()),
    );
    pending.runtime_log_writer =
        crate::log::BufferedLogWriter::with_flush_every(dir.path().join("runtime.jsonl"), 1);
    pending.runtime_log_writer.set_fail_after_bytes(Some(0));

    let result = crate::query::provider::emit_runtime_log(
        &mut pending,
        crate::log::LogLevel::Info,
        "runtime.test",
        "test log",
        None,
    );

    assert!(result.is_ok());
    let Some(UiEvent::Log { record }) = pending.pending_events.pop_front() else {
        panic!("expected host-visible log event before persistence failure");
    };
    assert_eq!(record.kind, "runtime.test");
}

#[tokio::test]
async fn runtime_log_persists_only_after_host_consumes_log_event() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let log_path = dir.path().join("runtime.jsonl");
    let mut pending = make_test_pending(
        events_path,
        dir.path(),
        std::sync::Arc::new(tokio::sync::Notify::new()),
    );
    pending.runtime_log_writer = crate::log::BufferedLogWriter::with_flush_every(&log_path, 1);
    crate::query::provider::emit_runtime_log(
        &mut pending,
        crate::log::LogLevel::Warn,
        "runtime.warn",
        "warn log",
        None,
    )
    .unwrap();

    assert!(
        !log_path.exists(),
        "disk write happened before host delivery"
    );

    let (slot_event_tx, slot_event_rx) = tokio::sync::mpsc::channel(16);
    let mut run = Run {
        session_id: "test".to_string(),
        state: RunState::Pending(Box::new(pending)),
        slots: std::collections::HashMap::new(),
        slot_event_tx,
        slot_event_rx,
        cancel_token: std::sync::Arc::new(tokio::sync::Notify::new()),
        lock_path: std::path::PathBuf::new(),
        deferred_runtime_logs: std::collections::VecDeque::new(),
    };

    let event = run.next().await.unwrap().expect("log event");
    assert!(matches!(event, UiEvent::Log { .. }));
    assert!(
        !log_path.exists(),
        "disk write happened before host consumed log"
    );

    let _ = run.next().await;
    assert!(
        log_path.exists(),
        "disk write should happen after host consumption"
    );
}

#[tokio::test]
async fn completion_flush_failure_does_not_block_done() {
    let dir = tempfile::tempdir().unwrap();
    let mut pending = make_test_pending(
        dir.path().join("events.jsonl"),
        dir.path(),
        std::sync::Arc::new(tokio::sync::Notify::new()),
    );
    pending.runtime_log_writer =
        crate::log::BufferedLogWriter::with_flush_every(dir.path().join("runtime.jsonl"), 64);
    pending.runtime_log_writer.set_fail_after_bytes(Some(0));
    crate::query::provider::emit_runtime_log(
        &mut pending,
        crate::log::LogLevel::Info,
        "runtime.test",
        "test log",
        None,
    )
    .unwrap();

    let state = StreamingChunkState {
        pending,
        conversation: crate::conversation::address::ConversationAddress::MAIN,
        request_id: "req_1".to_string(),
        stream: Box::pin(tokio_stream::empty()),
        accumulated_text: "complete".to_string(),
        accumulated_thinking: String::new(),
        stop_reason: Some("end_turn".to_string()),
        tool_calls: Vec::new(),
        tool_arg_buffers: Vec::new(),
        provider_request_id: None,
        usage: None,
        lead_events: Vec::new(),
        handoff_detector: None,
        thinking_start: None,
        thinking_duration_ms: 0,
    };

    let step = crate::query::step::finish_streaming(state).await;

    assert!(matches!(step, Ok(PendingStep::Done(output, _, 1)) if output.text == "complete"));
}

#[tokio::test]
async fn completion_persists_runtime_model_usage_log() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let log_path = dir.path().join("runtime.jsonl");
    let mut pending = make_test_pending(
        events_path,
        dir.path(),
        std::sync::Arc::new(tokio::sync::Notify::new()),
    );
    pending.runtime_log_writer = crate::log::BufferedLogWriter::with_flush_every(&log_path, 1);

    let state = StreamingChunkState {
        pending,
        conversation: crate::conversation::address::ConversationAddress::MAIN,
        request_id: "req_7".to_string(),
        stream: Box::pin(tokio_stream::empty()),
        accumulated_text: "complete".to_string(),
        accumulated_thinking: String::new(),
        stop_reason: Some("end_turn".to_string()),
        tool_calls: Vec::new(),
        tool_arg_buffers: Vec::new(),
        provider_request_id: None,
        usage: Some(crate::provider::types::ProviderUsage {
            input_tokens: Some(120),
            output_tokens: Some(30),
            cache_read_input_tokens: Some(900),
            cache_creation_input_tokens: Some(0),
        }),
        lead_events: Vec::new(),
        handoff_detector: None,
        thinking_start: None,
        thinking_duration_ms: 0,
    };

    let step = crate::query::step::finish_streaming(state).await;

    assert!(matches!(step, Ok(PendingStep::Done(output, _, 1)) if output.text == "complete"));
    let log = std::fs::read_to_string(&log_path).expect("runtime log should be written");
    assert!(log.contains("\"kind\":\"runtime.model_usage\""));
    assert!(log.contains("\"request_id\":\"req_7\""));
    assert!(log.contains("\"cache_read_input_tokens\":900"));
    assert!(log.contains("\"cache_hit_rate\":"));
}

#[tokio::test]
async fn incomplete_handoff_marker_does_not_leak_to_final_output() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let mut pending = make_test_pending(
        events_path.clone(),
        dir.path(),
        std::sync::Arc::new(tokio::sync::Notify::new()),
    );
    pending.handoff_triggered = true;

    let stream: std::pin::Pin<
        Box<
            dyn futures_core::Stream<
                    Item = std::result::Result<
                        crate::provider::chunk::ProviderChunk,
                        crate::provider::types::ProviderFailure,
                    >,
                > + Send,
        >,
    > = Box::pin(tokio_stream::iter(vec![
        Ok(crate::provider::chunk::ProviderChunk::TextDelta {
            text: "visible".to_string(),
        }),
        Ok(crate::provider::chunk::ProviderChunk::TextDelta {
            text: "\n\n<kuku_handoff".to_string(),
        }),
        Ok(crate::provider::chunk::ProviderChunk::StopReason {
            reason: "end_turn".to_string(),
        }),
        Ok(crate::provider::chunk::ProviderChunk::StreamEnd),
    ]));

    let mut streaming = StreamingChunkState {
        pending,
        conversation: crate::conversation::address::ConversationAddress::MAIN,
        request_id: "req_1".to_string(),
        stream,
        accumulated_text: String::new(),
        accumulated_thinking: String::new(),
        stop_reason: None,
        tool_calls: Vec::new(),
        tool_arg_buffers: Vec::new(),
        provider_request_id: None,
        usage: None,
        lead_events: Vec::new(),
        handoff_detector: Some(crate::query::handoff::HandoffDetector::new()),
        thinking_start: None,
        thinking_duration_ms: 0,
    };
    let cancel_token = std::sync::Arc::new(tokio::sync::Notify::new());

    loop {
        match Run::poll_stream_chunk(&cancel_token, &mut streaming)
            .await
            .unwrap()
        {
            Some(UiEvent::TextDelta { text }) => assert_eq!(text, "visible"),
            Some(_) => continue,
            None => break,
        }
    }
    let step = crate::query::step::finish_streaming(streaming)
        .await
        .unwrap();

    let PendingStep::Done(output, _, _) = step else {
        panic!("expected done step");
    };
    assert_eq!(output.text, "visible");

    let events = EventStore::replay(&events_path).unwrap();
    let response = events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::ModelResponse { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .expect("model.response event");
    assert_eq!(response, "visible");
}

#[tokio::test]
async fn cancel_during_streaming_aborts_stream() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    std::fs::write(&events_path, "").unwrap();
    let cancel_token = std::sync::Arc::new(tokio::sync::Notify::new());

    let token_clone = cancel_token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        token_clone.notify_waiters();
    });

    let pending = make_test_pending(events_path.clone(), dir.path(), cancel_token.clone());

    let stream: std::pin::Pin<
        Box<
            dyn futures_core::Stream<
                    Item = std::result::Result<
                        crate::provider::chunk::ProviderChunk,
                        crate::provider::types::ProviderFailure,
                    >,
                > + Send
                + Sync,
        >,
    > = Box::pin(tokio_stream::pending());

    let mut streaming = StreamingChunkState {
        pending,
        conversation: crate::conversation::address::ConversationAddress::MAIN,
        request_id: "req_1".to_string(),
        stream,
        accumulated_text: "partial".to_string(),
        accumulated_thinking: String::new(),
        stop_reason: None,
        tool_calls: Vec::new(),
        tool_arg_buffers: Vec::new(),
        provider_request_id: None,
        usage: None,
        lead_events: Vec::new(),
        handoff_detector: None,
        thinking_start: None,
        thinking_duration_ms: 0,
    };

    let (slot_event_tx, slot_event_rx) = tokio::sync::mpsc::channel(16);
    let _run = Run {
        session_id: "test".to_string(),
        state: RunState::Pending(Box::new(make_test_pending(
            events_path.clone(),
            dir.path(),
            cancel_token.clone(),
        ))),
        slots: std::collections::HashMap::new(),
        slot_event_tx,
        slot_event_rx,
        cancel_token: cancel_token.clone(),
        lock_path: std::path::PathBuf::new(),
        deferred_runtime_logs: std::collections::VecDeque::new(),
    };

    let result = Run::poll_stream_chunk(&cancel_token, &mut streaming)
        .await
        .unwrap();
    assert!(result.is_none());
    assert_eq!(streaming.stop_reason.as_deref(), Some("cancelled"));
}

#[tokio::test]
async fn malformed_tool_call_arguments_fail_instead_of_staying_empty_object() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    std::fs::write(&events_path, "").unwrap();
    let cancel_token = std::sync::Arc::new(tokio::sync::Notify::new());

    let pending = make_test_pending(events_path, dir.path(), cancel_token.clone());
    let stream: std::pin::Pin<
        Box<
            dyn futures_core::Stream<
                    Item = std::result::Result<
                        crate::provider::chunk::ProviderChunk,
                        crate::provider::types::ProviderFailure,
                    >,
                > + Send,
        >,
    > = Box::pin(tokio_stream::iter(vec![
        Ok(crate::provider::chunk::ProviderChunk::ToolCallStart {
            index: 0,
            id: "tool_bad_args".to_string(),
            name: "run_command".to_string(),
        }),
        Ok(crate::provider::chunk::ProviderChunk::ToolCallArgDelta {
            index: 0,
            fragment: "{\"command\":".to_string(),
        }),
        Ok(crate::provider::chunk::ProviderChunk::ContentBlockStop { index: 0 }),
        Ok(crate::provider::chunk::ProviderChunk::StopReason {
            reason: "tool_use".to_string(),
        }),
        Ok(crate::provider::chunk::ProviderChunk::StreamEnd),
    ]));

    let mut streaming = StreamingChunkState {
        pending,
        conversation: crate::conversation::address::ConversationAddress::MAIN,
        request_id: "req_bad_args".to_string(),
        stream,
        accumulated_text: String::new(),
        accumulated_thinking: String::new(),
        stop_reason: None,
        tool_calls: Vec::new(),
        tool_arg_buffers: Vec::new(),
        provider_request_id: None,
        usage: None,
        lead_events: Vec::new(),
        handoff_detector: None,
        thinking_start: None,
        thinking_duration_ms: 0,
    };

    let error = loop {
        match Run::poll_stream_chunk(&cancel_token, &mut streaming).await {
            Ok(Some(_)) => continue,
            Ok(None) => panic!("expected malformed tool args to fail"),
            Err(error) => break error,
        }
    };

    assert!(matches!(
        error,
        crate::error::Error::Provider { kind: crate::provider::types::ProviderFailureKind::InvalidRequest, message, .. }
            if message.contains("tool_bad_args")
    ));
}

#[tokio::test]
async fn cancelled_tool_result_envelope_has_correct_fields() {
    let result = crate::tool::ToolResultEnvelope::cancelled("test cancel");
    assert_eq!(result.status, "cancelled");
    assert_eq!(result.summary, "test cancel");
    assert!(result.model_content.is_empty());
    assert!(!result.truncated);
    assert_eq!(
        result.structured,
        Some(serde_json::json!({"kind": "cancelled"}))
    );
}

#[test]
fn cancel_pending_permission_rejects_mismatched_queued_tool() {
    let dir = tempfile::tempdir().unwrap();
    let mut run = make_waiting_run(
        dir.path().join("events.jsonl"),
        dir.path(),
        "req_cancel",
        "tool_request",
        "tool_queued",
    );

    let error = run.cancel_pending_permission("req_cancel").unwrap_err();

    assert!(
        matches!(error, crate::error::Error::InvalidEventStream(message) if message.contains("tool_request") && message.contains("tool_queued"))
    );
    assert!(matches!(
        &run.state,
        RunState::WaitingForPermission(waiting)
            if waiting.request.tool_call_id == "tool_request"
                && waiting.pending.queued_tool_calls.front().unwrap().tool_call.id == "tool_queued"
    ));
}

#[test]
fn cancel_pending_permission_restores_state_when_persistence_fails() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events_dir");
    std::fs::create_dir(&events_path).unwrap();
    let mut run = make_waiting_run(
        events_path,
        dir.path(),
        "req_cancel",
        "tool_cancel",
        "tool_cancel",
    );

    let error = run.cancel_pending_permission("req_cancel").unwrap_err();

    assert!(matches!(error, crate::error::Error::Io(_)));
    assert!(matches!(
        &run.state,
        RunState::WaitingForPermission(waiting)
            if waiting.request.id == "req_cancel"
                && waiting.pending.queued_tool_calls.front().unwrap().tool_call.id == "tool_cancel"
    ));
}

#[tokio::test]
async fn cancel_waiting_permission_writes_cancelled_result_without_deny() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let mut run = make_waiting_run(
        events_path.clone(),
        dir.path(),
        "req_cancel",
        "tool_cancel",
        "tool_cancel",
    );

    run.cancel();
    let event = run.next().await.unwrap();

    assert!(matches!(event, Some(UiEvent::Cancelled { turn: 1 })));
    let events = EventStore::replay(&events_path).unwrap();
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ToolResult { ref tool_call_id, ref status, ref structured, .. }
            if tool_call_id == "tool_cancel"
                && status == "cancelled"
                && structured == &Some(serde_json::json!({"kind": "cancelled"}))
    )));
    assert!(!events.iter().any(|event| matches!(
        event.payload,
        EventPayload::PermissionDeny { ref tool_call_id, .. } if tool_call_id == "tool_cancel"
    )));
}

#[tokio::test]
async fn cancelled_run_persists_tool_result_for_finished_active_slot() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    let mut store = EventStore::open(&events_path).unwrap();
    store
        .append(EventPayload::TurnStarted {
            turn: 1,
            ts: "2026-05-20T00:00:00Z".to_string(),
            conversation: "main".to_string(),
        })
        .unwrap();
    store
        .append(EventPayload::ToolCall {
            turn: 1,
            ts: "2026-05-20T00:00:01Z".to_string(),
            conversation: None,
            tool_call_id: "tool_cancelled".to_string(),
            request_id: "req_1".to_string(),
            index: 0,
            tool: "run_command".to_string(),
            args: serde_json::json!({"command": "printf hi", "timeout": 60, "brief": "print hi"}),
        })
        .unwrap();

    let (slot_event_tx, slot_event_rx) = tokio::sync::mpsc::channel(16);
    let mut slots = std::collections::HashMap::new();
    slots.insert(
        "tool_cancelled".to_string(),
        ExecSlot {
            tool_call_id: "tool_cancelled".to_string(),
            conversation: None,
            kind: ToolKind::Command { pid: None },
            ordered_with_simple_tools: false,
            label: "print hi".to_string(),
            cancel: std::sync::Arc::new(tokio::sync::Notify::new()),
            nested_permissions: std::sync::Arc::new(std::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
        },
    );
    let mut run = Run {
        session_id: "test".to_string(),
        state: RunState::Cancelled {
            events_path: events_path.clone(),
            turn: 1,
        },
        slots,
        slot_event_tx: slot_event_tx.clone(),
        slot_event_rx,
        cancel_token: std::sync::Arc::new(tokio::sync::Notify::new()),
        lock_path: std::path::PathBuf::new(),
        deferred_runtime_logs: std::collections::VecDeque::new(),
    };

    slot_event_tx
        .send((
            "tool_cancelled".to_string(),
            SlotEvent::Done {
                status: "ok".to_string(),
                summary: "finished after cancellation".to_string(),
                model_content: String::new(),
                result: Some(serde_json::json!({"kind": "command_result"})),
            },
        ))
        .await
        .unwrap();

    let event = run.next().await.unwrap();

    assert!(matches!(
        event,
        Some(UiEvent::ToolEnd { ref id, ref status, .. })
            if id == "tool_cancelled" && status == "ok"
    ));
    let events = EventStore::replay(&events_path).unwrap();
    assert!(events.iter().any(|event| matches!(
        &event.payload,
        EventPayload::ToolResult { tool_call_id, status, summary, .. }
            if tool_call_id == "tool_cancelled"
                && status == "ok"
                && summary == "finished after cancellation"
    )));
}

#[tokio::test]
async fn resume_after_cancel_includes_turn_end_in_history() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join("events.jsonl");
    {
        let mut store = EventStore::open(&events_path).unwrap();
        store
            .append(EventPayload::SessionCreated {
                ts: "2026-05-20T00:00:00Z".to_string(),
                schema_version: 2,
                session_id: "test".to_string(),
                created_at: "2026-05-20T00:00:00Z".to_string(),
                kuku_version: "0.1.0".to_string(),
            })
            .unwrap();
        store
            .append(EventPayload::TurnStarted {
                turn: 1,
                ts: "2026-05-20T00:00:00Z".to_string(),
                conversation: "main".to_string(),
            })
            .unwrap();
        store
            .append(EventPayload::MessageUser {
                turn: 1,
                ts: "2026-05-20T00:00:01Z".to_string(),
                conversation: "main".to_string(),
                text: "hello".to_string(),
                from: None,
                via_tool_call_id: None,
            })
            .unwrap();
        store
            .append(EventPayload::ModelResponse {
                turn: 1,
                ts: "2026-05-20T00:00:02Z".to_string(),
                request_id: "req_1".to_string(),
                text: "partial".to_string(),
                thinking: None,
                input_tokens_total: None,
            })
            .unwrap();
        store
            .append(EventPayload::TurnCompleted {
                turn: 1,
                ts: "2026-05-20T00:00:03Z".to_string(),
                conversation: "main".to_string(),
            })
            .unwrap();
    }

    let events = EventStore::replay(&events_path).unwrap();
    let (summary, history) = crate::context::rebuild_history(
        &events,
        &crate::conversation::address::ConversationAddress::MAIN,
    );
    assert!(summary.is_none());
    assert_eq!(history.len(), 2);
    let messages: Vec<_> = history.iter().map(|m| format!("{:?}", m.role)).collect();
    assert!(messages.contains(&"User".to_string()));
    assert!(messages.contains(&"Assistant".to_string()));
}
