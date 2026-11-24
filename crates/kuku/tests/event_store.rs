use std::io::ErrorKind;
use std::process::Command;
use std::sync::Arc;

use kuku::context::FileSource;
use kuku::context::provenance::{
    PromptCapabilityMetadata, PromptRendererIdentity, ToolRegistryProvenance,
};
use kuku::error::Error;
use kuku::event::{EventPayload, EventStore, StoredEvent};

fn current_event_lines() -> Vec<&'static str> {
    vec![
        r#"{"id":1,"ts":"2026-06-09T00:00:00Z","kind":"session.created","session_id":"s_001","created_at":"2026-06-09T00:00:00Z","kuku_version":"0.1.0","schema_version":2}"#,
        r#"{"id":4,"ts":"2026-06-09T00:00:03Z","kind":"prompt.snapshot","conversation":"main","snapshot_id":"snapshot:main:1","turn":1,"messages":[{"role":"system","content":"You are kuku."},{"role":"user","content":"Please inspect src/lib.rs and summarize changes."}],"project_instruction_sources":[{"path":"/workspace/AGENTS.md","hash":"sha256:agents"}],"memory_sources":[{"path":"/home/user/.kuku/memory.md","hash":"sha256:memory"}],"prompt_asset_sources":[],"skills":{"names":["using-superpowers","test-driven-development"],"hash":"sha256:skills"},"bootstrap_loaded":["using-superpowers","test-driven-development"],"provider":"anthropic","model":"claude-sonnet-4-6","renderer":{"provider":"anthropic","renderer":"anthropic"},"tool_registry":{"hash":"sha256:tools","names":[],"tool_count":0},"capabilities":{"context_budget_tier":"normal","max_context_tokens":200000,"remaining_input_tokens":180000}}"#,
        r#"{"id":5,"ts":"2026-06-09T00:00:04Z","kind":"message.user","conversation":"main","turn":1,"text":"Please inspect src/lib.rs and summarize changes."}"#,
        r#"{"id":6,"ts":"2026-06-09T00:00:05Z","kind":"message.assistant","conversation":"main","turn":1,"message_id":"msg_001","text":"I am checking the file now."}"#,
        r#"{"id":7,"ts":"2026-06-09T00:00:06Z","kind":"tool.call","conversation":"main","turn":1,"tool_call_id":"toolu_read_1","request_id":"req_1","index":0,"tool":"read_file","args":{"path":"src/lib.rs"}}"#,
        r#"{"id":8,"ts":"2026-06-09T00:00:07Z","kind":"tool.result","conversation":"main","turn":1,"tool_call_id":"toolu_read_1","status":"ok","summary":"Read src/lib.rs","model_content":"Read complete","truncated":false,"files_read":["src/lib.rs"],"files_changed":["src/lib.rs"],"commands_run":["cargo check -p kuku"],"memory_changed":{"scope":"project","action":"remember","count":1},"structured":{"kind":"file_content","path":"src/lib.rs"}}"#,
        r#"{"id":9,"ts":"2026-06-09T00:00:08Z","kind":"turn.started","conversation":"main","turn":1}"#,
        r#"{"id":10,"ts":"2026-06-09T00:00:09Z","kind":"turn.completed","conversation":"main","turn":1}"#,
        r#"{"id":11,"ts":"2026-06-09T00:00:10Z","kind":"turn.cancelled","conversation":"main","turn":2,"reason":"user_cancelled"}"#,
        r#"{"id":12,"ts":"2026-06-09T00:00:11Z","kind":"turn.interrupted","conversation":"main","turn":3,"reason":"approval_required"}"#,
    ]
}

fn session_created() -> EventPayload {
    EventPayload::SessionCreated {
        ts: "2026-05-13T00:00:00Z".to_string(),
        schema_version: 2,
        session_id: "s_001".to_string(),
        created_at: "2026-05-13T00:00:00Z".to_string(),
        kuku_version: "0.1.0".to_string(),
        default_model: None,
    }
}

fn prompt_snapshot(turn: u64, content: &str) -> EventPayload {
    EventPayload::PromptSnapshot {
        ts: "2026-05-13T00:00:00Z".to_string(),
        conversation: "main".to_string(),
        snapshot_id: format!("snapshot:main:{turn}"),
        turn,
        messages: vec![kuku::event::types::ContextMessage {
            role: "user".to_string(),
            content: content.to_string(),
        }],
        project_instruction_sources: vec![],
        memory_sources: vec![],
        prompt_asset_sources: vec![],
        skills: serde_json::json!({"names": [], "hash": "sha256:skills"}),
        bootstrap_loaded: vec![],
        provider: "anthropic".to_string(),
        model: "claude-sonnet-4-6".to_string(),
        renderer: PromptRendererIdentity {
            provider: "anthropic".to_string(),
            renderer: "anthropic".to_string(),
        },
        tool_registry: Box::new(ToolRegistryProvenance {
            hash: "sha256:tools".to_string(),
            names: vec![],
            tool_count: 0,
        }),
        agent_registry: None,
        skill_registry: Box::new(None),
        plugin_registry: Box::new(None),
        capabilities: PromptCapabilityMetadata {
            context_budget_tier: "normal".to_string(),
            max_context_tokens: Some(200000),
            remaining_input_tokens: Some(180000),
        },
    }
}

#[test]
fn appends_events_with_monotonic_ids() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let mut store = EventStore::open(&path).unwrap();

    let first = store.append(session_created()).unwrap();
    let second = store
        .append(EventPayload::TurnStarted {
            turn: 1,
            ts: "2026-05-13T00:00:01Z".to_string(),
            conversation: "main".to_string(),
        })
        .unwrap();

    assert_eq!(first.id, 1);
    assert_eq!(second.id, 2);

    let replayed = EventStore::replay(&path).unwrap();
    assert_eq!(replayed.len(), 2);
    assert_eq!(replayed[0].id, 1);
    assert_eq!(replayed[1].id, 2);
}

#[test]
fn reverse_pages_cross_checkpoints_and_ignore_an_uncommitted_tail() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let mut journal = Vec::new();
    for id in 1..=640 {
        let content = if id % 143 == 0 {
            "大".repeat(4000)
        } else {
            "work".repeat(10)
        };
        let event = serde_json::json!({"id":id,"kind":"fixture.progress","content":content});
        serde_json::to_writer(&mut journal, &event).unwrap();
        journal.extend(b"\r\n");
    }
    journal.extend(b"\n{\"id\":641");
    std::fs::write(&path, journal).unwrap();
    let page_ids = |before, limit| {
        EventStore::read_page_before(&path, before, limit)
            .unwrap()
            .into_iter()
            .map(|event| event.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(640, EventStore::read_head(&path).unwrap());
    assert_eq!(vec![638, 639, 640], page_ids(None, 3));
    assert_eq!(vec![141, 142, 143, 144, 145], page_ids(Some(146), 5));
    assert_eq!(vec![1, 2, 3], page_ids(Some(4), 100));
    assert!(page_ids(Some(1), 100).is_empty());
    let _writer = EventStore::open(&path).unwrap();
    assert_eq!(vec![126, 127, 128, 129], page_ids(Some(130), 4));
    assert_eq!(vec![639, 640], page_ids(Some(u64::MAX), 2));
}

#[test]
fn ignores_incomplete_trailing_line_on_replay() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    std::fs::write(
        &path,
        "{\"id\":1,\"ts\":\"2026-05-13T00:00:00Z\",\"kind\":\"session.created\",\"schema_version\":2,\"session_id\":\"s_001\",\"created_at\":\"2026-05-13T00:00:00Z\",\"kuku_version\":\"0.1.0\"}\n{\"id\":",
    )
    .unwrap();

    let replayed = EventStore::replay(&path).unwrap();
    assert_eq!(replayed.len(), 1);
    assert_eq!(replayed[0].id, 1);
}

#[test]
fn rejects_invalid_middle_line_even_when_later_events_are_valid() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    std::fs::write(
        &path,
        concat!(
            "{\"id\":1,\"ts\":\"2026-05-13T00:00:00Z\",\"kind\":\"session.created\",\"schema_version\":2,\"session_id\":\"s_001\",\"created_at\":\"2026-05-13T00:00:00Z\",\"kuku_version\":\"0.1.0\"}\n",
            "{\"id\":\n",
            "{\"id\":2,\"ts\":\"2026-05-13T00:00:01Z\",\"kind\":\"turn.started\",\"conversation\":\"main\",\"turn\":1}\n",
        ),
    )
    .unwrap();

    let error = EventStore::replay(&path).unwrap_err();
    assert!(matches!(error, Error::InvalidEventStream(_)));
}

#[test]
fn truncates_partial_tail_before_appending_after_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    std::fs::write(
        &path,
        "{\"id\":1,\"ts\":\"2026-05-13T00:00:00Z\",\"kind\":\"session.created\",\"schema_version\":2,\"session_id\":\"s_001\",\"created_at\":\"2026-05-13T00:00:00Z\",\"kuku_version\":\"0.1.0\"}\n{\"id\":",
    )
    .unwrap();

    let mut store = EventStore::open(&path).unwrap();
    let appended = store
        .append(EventPayload::TurnStarted {
            turn: 1,
            ts: "2026-05-13T00:00:01Z".to_string(),
            conversation: "main".to_string(),
        })
        .unwrap();

    assert_eq!(appended.id, 2);
    drop(store);

    let contents = std::fs::read_to_string(&path).unwrap();
    assert!(!contents.ends_with("{\"id\":"));

    let replayed = EventStore::replay(&path).unwrap();
    assert_eq!(replayed.len(), 2);
    assert_eq!(replayed[0].id, 1);
    assert_eq!(replayed[1].id, 2);
}

#[test]
fn replay_returns_empty_when_file_is_missing() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");

    let replayed = EventStore::replay(&path).unwrap();
    assert!(replayed.is_empty());
}

#[test]
fn open_creates_parent_directories() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("nested").join("events.jsonl");

    let mut store = EventStore::open(&path).unwrap();

    assert_eq!(store.append(session_created()).unwrap().id, 1);
}

#[test]
fn rejects_non_monotonic_ids() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    std::fs::write(
        &path,
        concat!(
            "{\"id\":2,\"ts\":\"2026-05-13T00:00:00Z\",\"kind\":\"session.created\",\"schema_version\":2,\"session_id\":\"s_001\",\"created_at\":\"2026-05-13T00:00:00Z\",\"kuku_version\":\"0.1.0\"}\n",
            "{\"id\":2,\"ts\":\"2026-05-13T00:00:01Z\",\"kind\":\"turn.started\",\"conversation\":\"main\",\"turn\":1}\n",
        ),
    )
    .unwrap();

    let error = EventStore::replay(&path).unwrap_err();
    assert!(matches!(error, Error::InvalidEventStream(_)));
}

#[test]
fn skips_blank_lines() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    std::fs::write(
        &path,
        concat!(
            "\n",
            "{\"id\":1,\"ts\":\"2026-05-13T00:00:00Z\",\"kind\":\"session.created\",\"schema_version\":2,\"session_id\":\"s_001\",\"created_at\":\"2026-05-13T00:00:00Z\",\"kuku_version\":\"0.1.0\"}\n",
            "  \n",
            "{\"id\":2,\"ts\":\"2026-05-13T00:00:01Z\",\"kind\":\"turn.started\",\"conversation\":\"main\",\"turn\":1}\n",
        ),
    )
    .unwrap();

    let replayed = EventStore::replay(&path).unwrap();
    assert_eq!(replayed.len(), 2);
    assert_eq!(replayed[0].id, 1);
    assert_eq!(replayed[1].id, 2);
}

#[test]
fn append_writes_newline_terminated_jsonl() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let mut store = EventStore::open(&path).unwrap();

    store.append(session_created()).unwrap();
    drop(store);

    let contents = std::fs::read_to_string(&path).unwrap();
    assert!(contents.ends_with('\n'));
}

#[test]
fn concurrent_handles_do_not_reuse_the_same_event_id() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let mut left = EventStore::open(&path).unwrap();
    let mut right = EventStore::open(&path).unwrap();

    let first = left.append(session_created()).unwrap();
    let second = right
        .append(EventPayload::TurnStarted {
            turn: 1,
            ts: "2026-05-13T00:00:01Z".to_string(),
            conversation: "main".to_string(),
        })
        .unwrap();

    assert_eq!(1, first.id);
    assert_eq!(2, second.id);

    let replayed = EventStore::replay(&path).unwrap();
    assert_eq!(2, replayed.len());
    assert_eq!(1, replayed[0].id);
    assert_eq!(2, replayed[1].id);
}

#[test]
fn concurrent_processes_do_not_reuse_the_same_event_id() {
    if std::env::var("KUKU_EVENT_STORE_CHILD").ok().as_deref() == Some("1") {
        let path = std::env::var("KUKU_EVENT_STORE_PATH").unwrap();
        let mut store = EventStore::open(&path).unwrap();
        store.append(session_created()).unwrap();
        return;
    }

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let current_exe = std::env::current_exe().unwrap();

    let spawn_child = || {
        Command::new(&current_exe)
            .env("KUKU_EVENT_STORE_CHILD", "1")
            .env("KUKU_EVENT_STORE_PATH", &path)
            .arg("--exact")
            .arg("concurrent_processes_do_not_reuse_the_same_event_id")
            .status()
            .unwrap()
    };

    let left = spawn_child();
    let right = spawn_child();

    assert!(left.success());
    assert!(right.success());

    let replayed = EventStore::replay(&path).unwrap();
    assert_eq!(2, replayed.len());
    assert_eq!(1, replayed[0].id);
    assert_eq!(2, replayed[1].id);
}

#[test]
fn fact_only_events_roundtrip_without_observability_fields() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let mut store = EventStore::open(&path).unwrap();

    store.append(session_created()).unwrap();
    store
        .append(prompt_snapshot(
            1,
            "<kuku_execution_context>workspace: /tmp</kuku_execution_context>",
        ))
        .unwrap();
    store
        .append(EventPayload::ContextSources {
            turn: 1,
            ts: "2026-05-13T00:00:00Z".to_string(),
            request_id: "req_1".to_string(),
            project_instruction_sources: vec![FileSource {
                path: "/workspace/AGENTS.md".to_string(),
                hash: "sha256:agents".to_string(),
            }],
            memory_sources: vec![FileSource {
                path: "/home/user/.kuku/memory.md".to_string(),
                hash: "sha256:memory".to_string(),
            }],
        })
        .unwrap();
    store
        .append(EventPayload::ModelResponse {
            conversation: None,
            turn: 1,
            ts: "2026-05-13T00:00:00Z".to_string(),
            request_id: "req_1".to_string(),
            text: "answer".to_string(),
            thinking: Some("reasoning".to_string()),
            stop_reason: None,
            input_tokens_total: Some(123),
            output_tokens_total: None,
        })
        .unwrap();
    store
        .append(EventPayload::ModelError {
            conversation: None,
            turn: 1,
            ts: "2026-05-13T00:00:01Z".to_string(),
            request_id: "req_1".to_string(),
            kind: "RateLimited".to_string(),
            message: "HTTP 429: rate limited".to_string(),
        })
        .unwrap();

    let replayed = EventStore::replay(&path).unwrap();
    assert_eq!(replayed.len(), 5);

    match &replayed[1].payload {
        EventPayload::PromptSnapshot { messages, .. } => {
            assert_eq!(messages.len(), 1);
            assert!(messages[0].content.contains("<kuku_execution_context>"));
        }
        other => panic!("expected prompt.snapshot, got {other:?}"),
    }

    match &replayed[2].payload {
        EventPayload::ContextSources {
            request_id,
            project_instruction_sources,
            memory_sources,
            ..
        } => {
            assert_eq!(request_id, "req_1");
            assert_eq!(project_instruction_sources[0].path, "/workspace/AGENTS.md");
            assert_eq!(memory_sources[0].hash, "sha256:memory");
        }
        other => panic!("expected context.sources, got {other:?}"),
    }

    match &replayed[3].payload {
        EventPayload::ModelError { kind, message, .. } => {
            panic!(
                "expected model.response before model.error, got model.error kind={kind} message={message}"
            );
        }
        EventPayload::ModelResponse {
            text,
            thinking,
            input_tokens_total,
            ..
        } => {
            assert_eq!(text, "answer");
            assert_eq!(thinking.as_deref(), Some("reasoning"));
            assert_eq!(*input_tokens_total, Some(123));
        }
        other => panic!("expected model.response, got {other:?}"),
    }

    match &replayed[4].payload {
        EventPayload::ModelError { kind, message, .. } => {
            assert_eq!(kind, "RateLimited");
            assert_eq!(message, "HTTP 429: rate limited");
        }
        other => panic!("expected model.error, got {other:?}"),
    }
}

#[test]
fn open_returns_io_error_for_missing_parent_file_path() {
    let temp = tempfile::tempdir().unwrap();
    let parent_file = temp.path().join("not_a_directory");
    std::fs::write(&parent_file, "x").unwrap();
    let path = parent_file.join("events.jsonl");

    let error = match EventStore::open(&path) {
        Ok(_) => panic!("expected io error"),
        Err(error) => error,
    };
    assert!(
        matches!(error, Error::Io(ref io_error) if io_error.kind() == ErrorKind::AlreadyExists || io_error.kind() == ErrorKind::NotADirectory)
    );
}

#[test]
fn fact_event_json_omits_removed_observability_fields() {
    use kuku::event::types::ContextMessage;

    let event = StoredEvent {
        id: 10,
        payload: EventPayload::PromptSnapshot {
            ts: "2026-05-18T00:01:00Z".to_string(),
            conversation: "main".to_string(),
            snapshot_id: "snapshot:main:2".to_string(),
            turn: 2,
            messages: vec![ContextMessage {
                role: "user".to_string(),
                content: "<kuku_tool_guidance>use tools</kuku_tool_guidance>".to_string(),
            }],
            project_instruction_sources: vec![],
            memory_sources: vec![],
            prompt_asset_sources: vec![],
            skills: serde_json::json!({"names": [], "hash": "sha256:skills"}),
            bootstrap_loaded: vec![],
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            renderer: PromptRendererIdentity {
                provider: "anthropic".to_string(),
                renderer: "anthropic".to_string(),
            },
            tool_registry: Box::new(ToolRegistryProvenance {
                hash: "sha256:tools".to_string(),
                names: vec![],
                tool_count: 0,
            }),
            agent_registry: None,
            skill_registry: Box::new(None),
            plugin_registry: Box::new(None),
            capabilities: PromptCapabilityMetadata {
                context_budget_tier: "normal".to_string(),
                max_context_tokens: Some(200000),
                remaining_input_tokens: Some(180000),
            },
        },
    };
    let response = StoredEvent {
        id: 11,
        payload: EventPayload::ModelResponse {
            conversation: None,
            turn: 2,
            ts: "2026-05-18T00:01:00Z".to_string(),
            request_id: "req_2".to_string(),
            text: "hi".to_string(),
            thinking: None,
            stop_reason: None,
            input_tokens_total: Some(7),
            output_tokens_total: None,
        },
    };

    let prelude_json = serde_json::to_value(&event).unwrap();
    let response_json = serde_json::to_value(&response).unwrap();

    assert_eq!(prelude_json["kind"], "prompt.snapshot");
    assert!(prelude_json.get("provenance").is_none());
    assert!(response_json.get("usage").is_none());
    assert!(response_json.get("stop_reason").is_none());
    assert!(response_json.get("tool_call_count").is_none());
    assert_eq!(response_json["input_tokens_total"], 7);
}

#[test]
fn permission_requested_roundtrips_as_fact_event() {
    let event = StoredEvent {
        id: 12,
        payload: EventPayload::PermissionRequested {
            turn: 2,
            ts: "2026-05-18T00:02:00Z".to_string(),
            tool_call_id: "toolu_cmd".to_string(),
            tool: "run_command".to_string(),
            risk: "write".to_string(),
            summary: "run tests".to_string(),
            candidate: "cargo test".to_string(),
            source: "default_ask".to_string(),
            workspace: None,
        },
    };

    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["kind"], "permission.requested");
    assert!(json.get("type").is_none());
    assert_eq!(json["candidate"], "cargo test");
    assert_eq!(event.payload.kind_name(), "permission.requested");

    let back: StoredEvent = serde_json::from_value(json).unwrap();
    assert_eq!(back, event);
}

#[test]
fn new_writes_use_kind_not_type_at_top_level() {
    let event = StoredEvent {
        id: 8,
        payload: EventPayload::Unknown(serde_json::json!({
            "id": 8,
            "ts": "2026-06-09T00:00:07Z",
            "kind": "tool.result",
            "conversation": "main",
            "turn": 1,
            "tool_call_id": "toolu_read_1",
            "status": "ok",
            "summary": "Read src/lib.rs",
            "model_content": "Read complete",
            "truncated": false,
            "files_read": ["src/lib.rs"],
            "files_changed": ["src/lib.rs"],
            "commands_run": ["cargo check -p kuku"],
            "memory_changed": {"scope": "project", "action": "remember", "count": 1}
        })),
    };

    let json = serde_json::to_value(&event).unwrap();

    assert_eq!("tool.result", json["kind"]);
    assert!(
        json.get("type").is_none(),
        "new writes should omit top-level type: {json}"
    );
}

#[test]
fn replay_recognizes_current_event_kinds() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let contents = format!("{}\n", current_event_lines().join("\n"));
    std::fs::write(&path, contents).unwrap();

    let replayed = EventStore::replay(&path).unwrap();

    assert_eq!(10, replayed.len());
    for event in &replayed {
        assert!(
            !matches!(event.payload, EventPayload::Unknown(_)),
            "expected recognized kind for event {}: {:?}",
            event.id,
            event.payload
        );
    }
}

#[test]
fn unknown_kind_stays_readable() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    std::fs::write(
        &path,
        "{\"id\":1,\"ts\":\"2026-06-09T00:00:00Z\",\"kind\":\"future.event\",\"conversation\":\"main\",\"turn\":1,\"custom\":\"x\"}\n",
    )
    .unwrap();

    let replayed = EventStore::replay(&path).unwrap();

    match &replayed[0].payload {
        EventPayload::Unknown(value) => {
            assert_eq!("future.event", value["kind"]);
            assert_eq!("x", value["custom"]);
        }
        other => panic!("expected unknown event, got {other:?}"),
    }
}

#[test]
fn concurrent_async_appends_keep_contiguous_ids_and_valid_jsonl() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let temp = tempfile::tempdir().unwrap();
        let path = Arc::new(temp.path().join("events.jsonl"));
        let mut tasks = Vec::new();

        for index in 0..16_u64 {
            let path = Arc::clone(&path);
            tasks.push(tokio::spawn(async move {
                tokio::task::spawn_blocking(move || {
                    let mut store = EventStore::open(&*path).unwrap();
                    store
                        .append(EventPayload::TurnStarted {
                            turn: index + 1,
                            ts: format!("2026-06-09T00:00:{index:02}Z"),
                            conversation: "main".to_string(),
                        })
                        .unwrap()
                        .id
                })
                .await
                .unwrap()
            }));
        }

        let mut ids = Vec::new();
        for task in tasks {
            ids.push(task.await.unwrap());
        }
        ids.sort_unstable();

        let expected: Vec<u64> = (1..=16).collect();
        assert_eq!(expected, ids);

        let contents = std::fs::read_to_string(&*path).unwrap();
        for line in contents.lines() {
            serde_json::from_str::<serde_json::Value>(line).unwrap();
        }
    });
}

#[test]
fn unknown_appends_receive_the_assigned_id() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let mut store = EventStore::open(&path).unwrap();
    for supplied_id in [None, Some(500)] {
        let mut value = serde_json::json!({"kind": "runtime.input", "text": "work"});
        if let Some(id) = supplied_id {
            value["id"] = id.into();
        }
        let stored = store.append(EventPayload::Unknown(value)).unwrap();
        let EventPayload::Unknown(value) = &stored.payload else {
            panic!("unknown payload was replaced");
        };
        assert_eq!(stored.id, value["id"]);
    }
    let replayed = EventStore::replay(&path).unwrap();
    assert_eq!(
        vec![1, 2],
        replayed.iter().map(|e| e.id).collect::<Vec<_>>()
    );
    assert_eq!(2, store.last_id());
}

#[test]
fn paged_reads_resume_by_id_across_sparse_checkpoints_and_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let contents: String = (1..=600)
        .map(|index| format!("{{\"id\":{},\"kind\":\"runtime.test\"}}\n", index * 2))
        .collect();
    std::fs::write(&path, &contents).unwrap();
    let mut store = EventStore::open(&path).unwrap();
    assert_eq!(1200, store.last_id());
    assert!(store.read_after(0, 0).unwrap().is_empty());
    assert!(store.read_after(1200, 1).unwrap().is_empty());
    let first = store.read_after(511, usize::MAX).unwrap();
    assert_eq!(256, first.len());
    assert_eq!(512, first[0].id);
    assert_eq!(1022, first.last().unwrap().id);

    let mut all_ids = Vec::new();
    let mut after = 0;
    loop {
        let page = store.read_after(after, 73).unwrap();
        let Some(last) = page.last() else {
            break;
        };
        after = last.id;
        all_ids.extend(page.into_iter().map(|event| event.id));
    }
    assert_eq!(
        (1..=600).map(|index| index * 2).collect::<Vec<_>>(),
        all_ids
    );
    assert_eq!(2, store.read_after(0, 1).unwrap()[0].id);
    assert_eq!(1201, store.append(session_created()).unwrap().id);
    drop(store);
    assert!(
        std::fs::read(&path)
            .unwrap()
            .starts_with(contents.as_bytes())
    );
    let reopened = EventStore::open(&path).unwrap();
    assert_eq!(
        vec![1198, 1200, 1201],
        reopened
            .read_after(1196, 10)
            .unwrap()
            .into_iter()
            .map(|e| e.id)
            .collect::<Vec<_>>()
    );
}

#[test]
fn record_limit_rejects_oversized_writes_without_poisoning_the_writer() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let mut store = EventStore::open(&path).unwrap();
    let maximum = 1024 * 1024;
    let mut value = serde_json::json!({"id": 1, "kind": "runtime.test", "data": ""});
    let overhead = serde_json::to_vec(&value).unwrap().len() + 1;
    value["data"] = "x".repeat(maximum - overhead).into();
    assert_eq!(
        1,
        store
            .append(EventPayload::Unknown(value.clone()))
            .unwrap()
            .id
    );
    assert_eq!(maximum as u64, std::fs::metadata(&path).unwrap().len());

    value["data"] = "x".repeat(maximum - overhead + 1).into();
    assert!(matches!(
        store.append(EventPayload::Unknown(value)),
        Err(Error::InvalidArgument(_))
    ));
    assert_eq!(1, store.last_id());
    assert_eq!(maximum as u64, std::fs::metadata(&path).unwrap().len());
    assert_eq!(2, store.append(session_created()).unwrap().id);
    assert_eq!(1, store.read_after(0, 256).unwrap().len());
    assert_eq!(2, store.read_after(1, 256).unwrap()[0].id);
    drop(store);
    assert_eq!(2, EventStore::replay(&path).unwrap().len());
}

#[test]
fn page_byte_target_keeps_large_record_reads_bounded() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let mut store = EventStore::open(&path).unwrap();
    for _ in 0..5 {
        store
            .append(EventPayload::Unknown(serde_json::json!({
                "kind": "runtime.test", "data": "x".repeat(600 * 1024),
            })))
            .unwrap();
    }
    let page = store.read_after(0, 256).unwrap();
    assert_eq!(vec![1, 2], page.iter().map(|e| e.id).collect::<Vec<_>>());
    let next = store.read_after(2, 256).unwrap();
    assert_eq!(vec![3, 4], next.iter().map(|e| e.id).collect::<Vec<_>>());
    assert_eq!(5, store.read_after(4, 256).unwrap()[0].id);
}

#[test]
fn repairs_large_incomplete_tail_but_rejects_complete_corruption() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let first = "{\"id\":1,\"kind\":\"runtime.test\"}\n";
    std::fs::write(&path, format!("{first}{}", "x".repeat(3 * 1024 * 1024))).unwrap();
    assert_eq!(1, EventStore::replay(&path).unwrap().len());
    assert!(std::fs::metadata(&path).unwrap().len() > first.len() as u64);
    let store = EventStore::open(&path).unwrap();
    drop(store);
    assert_eq!(first.as_bytes(), std::fs::read(&path).unwrap());

    for tail in [
        "{\"id\":\n".to_string(),
        format!("{}\n", "x".repeat(1024 * 1024)),
    ] {
        let damaged = format!("{first}{tail}");
        std::fs::write(&path, &damaged).unwrap();
        assert!(matches!(
            EventStore::replay(&path),
            Err(Error::InvalidEventStream(_))
        ));
        assert!(matches!(
            EventStore::open(&path),
            Err(Error::InvalidEventStream(_))
        ));
        assert_eq!(damaged.as_bytes(), std::fs::read(&path).unwrap());
    }
}

#[test]
fn active_writer_rejects_another_process_until_the_last_clone_is_dropped() {
    if let Ok(expected) = std::env::var("KUKU_EVENT_STORE_LOCK_CHILD") {
        let path = std::env::var("KUKU_EVENT_STORE_PATH").unwrap();
        if expected == "locked" {
            let error = match EventStore::open(&path) {
                Ok(_) => panic!("another process acquired the active writer"),
                Err(error) => error,
            };
            assert!(matches!(error, Error::Io(ref error) if error.kind() == ErrorKind::WouldBlock));
            assert!(error.to_string().contains("already has an active writer"));
            assert!(
                matches!(EventStore::replay(&path), Err(Error::Io(error)) if error.kind() == ErrorKind::WouldBlock)
            );
            assert!(
                matches!(EventStore::read_page(&path, 0, 256), Err(Error::Io(error)) if error.kind() == ErrorKind::WouldBlock)
            );
        } else {
            let mut store = EventStore::open(&path).unwrap();
            assert_eq!(2, store.append(session_created()).unwrap().id);
        }
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let mut store = EventStore::open(&path).unwrap();
    store.append(session_created()).unwrap();
    let clone = store.clone();
    drop(store);
    let child = |expected| {
        let output = Command::new(std::env::current_exe().unwrap())
            .env("KUKU_EVENT_STORE_LOCK_CHILD", expected)
            .env("KUKU_EVENT_STORE_PATH", &path)
            .arg("--exact")
            .arg("active_writer_rejects_another_process_until_the_last_clone_is_dropped")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "child failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    child("locked");
    assert_eq!(1, EventStore::replay(&path).unwrap().len());
    drop(clone);
    child("unlocked");
    assert_eq!(2, EventStore::replay(&path).unwrap().len());
}

#[test]
fn normalized_paths_share_the_writer() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let mut original = EventStore::open(&path).unwrap();
    let alias = temp.path().join(".").join("events.jsonl");
    let mut aliased = EventStore::open(alias).unwrap();
    assert_eq!(1, original.append(session_created()).unwrap().id);
    assert_eq!(2, aliased.append(session_created()).unwrap().id);
    assert_eq!(2, original.last_id());
}

#[test]
fn hardlink_aliases_cannot_bypass_writer_or_reader_exclusion() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let alias = temp.path().join("alias.jsonl");
    let mut original = EventStore::open(&path).unwrap();
    original.append(session_created()).unwrap();
    std::fs::hard_link(&path, &alias).unwrap();

    assert!(
        matches!(EventStore::open(&alias), Err(Error::Io(error)) if error.kind() == ErrorKind::WouldBlock)
    );
    assert!(
        matches!(EventStore::replay(&alias), Err(Error::Io(error)) if error.kind() == ErrorKind::WouldBlock)
    );
    assert!(
        matches!(EventStore::read_page(&alias, 0, 256), Err(Error::Io(error)) if error.kind() == ErrorKind::WouldBlock)
    );
    let output = Command::new(std::env::current_exe().unwrap())
        .env("KUKU_EVENT_STORE_LOCK_CHILD", "locked")
        .env("KUKU_EVENT_STORE_PATH", &alias)
        .arg("--exact")
        .arg("active_writer_rejects_another_process_until_the_last_clone_is_dropped")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(2, original.append(session_created()).unwrap().id);
    drop(original);
    let mut aliased = EventStore::open(&alias).unwrap();
    assert_eq!(2, aliased.last_id());
    assert_eq!(3, aliased.append(session_created()).unwrap().id);
    drop(aliased);
    assert_eq!(
        vec![1, 2, 3],
        EventStore::replay(&path)
            .unwrap()
            .into_iter()
            .map(|event| event.id)
            .collect::<Vec<_>>()
    );
}
