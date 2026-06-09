use std::io::ErrorKind;
use std::process::Command;
use std::sync::Arc;

use kuku::context::FileSource;
use kuku::error::Error;
use kuku::event::{EventPayload, EventStore, StoredEvent};

fn new_event_lines() -> Vec<&'static str> {
    vec![
        r#"{"id":1,"ts":"2026-06-09T00:00:00Z","kind":"session.created","session_id":"s_001","created_at":"2026-06-09T00:00:00Z","kuku_version":"0.1.0","schema_version":2}"#,
        r#"{"id":2,"ts":"2026-06-09T00:00:01Z","kind":"conversation.opened","conversation":"session://s_001/conversations/c_main"}"#,
        r#"{"id":3,"ts":"2026-06-09T00:00:02Z","kind":"conversation.bound","conversation":"session://s_001/conversations/c_main","binding_id":"binding:main"}"#,
        r#"{"id":4,"ts":"2026-06-09T00:00:03Z","kind":"prompt.snapshot","conversation":"session://s_001/conversations/c_main","binding_id":"binding:main","snapshot_id":"snapshot:main:1","turn":1,"messages":[{"role":"system","content":"You are kuku."},{"role":"user","content":"Please inspect src/lib.rs and summarize changes."}],"project_instruction_sources":[{"path":"/workspace/AGENTS.md","hash":"sha256:agents"}],"memory_sources":[{"path":"/home/user/.kuku/memory.md","hash":"sha256:memory"}],"prompt_asset_sources":[],"skills":{"names":["using-superpowers","test-driven-development"],"hash":"sha256:skills"},"bootstrap_loaded":["using-superpowers","test-driven-development"],"provider":"anthropic","model":"claude-sonnet-4-6","renderer":{"provider":"anthropic","renderer":"anthropic"},"tool_registry":{"hash":"sha256:tools","names":[],"tool_count":0},"capabilities":{"context_budget_tier":"normal","max_context_tokens":200000,"remaining_input_tokens":180000}}"#,
        r#"{"id":5,"ts":"2026-06-09T00:00:04Z","kind":"message.user","conversation":"session://s_001/conversations/c_main","turn":1,"text":"Please inspect src/lib.rs and summarize changes."}"#,
        r#"{"id":6,"ts":"2026-06-09T00:00:05Z","kind":"message.assistant","conversation":"session://s_001/conversations/c_main","turn":1,"message_id":"msg_001","text":"I am checking the file now."}"#,
        r#"{"id":7,"ts":"2026-06-09T00:00:06Z","kind":"tool.call","conversation":"session://s_001/conversations/c_main","turn":1,"tool_call_id":"toolu_read_1","request_id":"req_1","index":0,"tool":"read_file","args":{"path":"src/lib.rs"}}"#,
        r#"{"id":8,"ts":"2026-06-09T00:00:07Z","kind":"tool.result","conversation":"session://s_001/conversations/c_main","turn":1,"tool_call_id":"toolu_read_1","status":"ok","summary":"Read src/lib.rs","model_content":"Read complete","truncated":false,"files_read":["src/lib.rs"],"files_changed":["src/lib.rs"],"commands_run":["cargo check -p kuku"],"memory_changed":{"scope":"project","action":"remember","count":1},"structured":{"kind":"file_content","path":"src/lib.rs"}}"#,
        r#"{"id":9,"ts":"2026-06-09T00:00:08Z","kind":"turn.started","conversation":"session://s_001/conversations/c_main","turn":1}"#,
        r#"{"id":10,"ts":"2026-06-09T00:00:09Z","kind":"turn.completed","conversation":"session://s_001/conversations/c_main","turn":1}"#,
        r#"{"id":11,"ts":"2026-06-09T00:00:10Z","kind":"turn.cancelled","conversation":"session://s_001/conversations/c_main","turn":2,"reason":"user_cancelled"}"#,
        r#"{"id":12,"ts":"2026-06-09T00:00:11Z","kind":"turn.interrupted","conversation":"session://s_001/conversations/c_main","turn":3,"reason":"approval_required"}"#,
        r#"{"id":13,"ts":"2026-06-09T00:00:12Z","kind":"conversation.rollback","conversation":"session://s_001/conversations/c_main","to_turn":2,"to_event_id":12,"scope":"messages"}"#,
        r#"{"id":14,"ts":"2026-06-09T00:00:13Z","kind":"conversation.rollback.undone","conversation":"session://s_001/conversations/c_main","rollback_event_id":13}"#,
    ]
}

fn session_meta() -> EventPayload {
    EventPayload::SessionMeta {
        ts: "2026-05-13T00:00:00Z".to_string(),
        schema_version: 1,
        session_id: "s_001".to_string(),
        created_at: "2026-05-13T00:00:00Z".to_string(),
        kuku_version: "0.1.0".to_string(),
    }
}

#[test]
fn appends_events_with_monotonic_ids() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let mut store = EventStore::open(&path).unwrap();

    let first = store.append(session_meta()).unwrap();
    let second = store
        .append(EventPayload::TurnStart {
            turn: 1,
            ts: "2026-05-13T00:00:01Z".to_string(),
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
fn ignores_incomplete_trailing_line_on_replay() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    std::fs::write(
        &path,
        "{\"id\":1,\"type\":\"session.meta\",\"ts\":\"2026-05-13T00:00:00Z\",\"schema_version\":1,\"session_id\":\"s_001\",\"created_at\":\"2026-05-13T00:00:00Z\",\"kuku_version\":\"0.1.0\"}\n{\"id\":",
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
            "{\"id\":1,\"type\":\"session.meta\",\"ts\":\"2026-05-13T00:00:00Z\",\"schema_version\":1,\"session_id\":\"s_001\",\"created_at\":\"2026-05-13T00:00:00Z\",\"kuku_version\":\"0.1.0\"}\n",
            "{\"id\":\n",
            "{\"id\":2,\"type\":\"turn.start\",\"turn\":1,\"ts\":\"2026-05-13T00:00:01Z\"}\n",
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
        "{\"id\":1,\"type\":\"session.meta\",\"ts\":\"2026-05-13T00:00:00Z\",\"schema_version\":1,\"session_id\":\"s_001\",\"created_at\":\"2026-05-13T00:00:00Z\",\"kuku_version\":\"0.1.0\"}\n{\"id\":",
    )
    .unwrap();

    let mut store = EventStore::open(&path).unwrap();
    let appended = store
        .append(EventPayload::TurnStart {
            turn: 1,
            ts: "2026-05-13T00:00:01Z".to_string(),
        })
        .unwrap();

    assert_eq!(appended.id, 2);

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

    assert_eq!(store.append(session_meta()).unwrap().id, 1);
}

#[test]
fn rejects_non_monotonic_ids() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    std::fs::write(
        &path,
        concat!(
            "{\"id\":2,\"type\":\"session.meta\",\"ts\":\"2026-05-13T00:00:00Z\",\"schema_version\":1,\"session_id\":\"s_001\",\"created_at\":\"2026-05-13T00:00:00Z\",\"kuku_version\":\"0.1.0\"}\n",
            "{\"id\":2,\"type\":\"turn.start\",\"turn\":1,\"ts\":\"2026-05-13T00:00:01Z\"}\n",
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
            "{\"id\":1,\"type\":\"session.meta\",\"ts\":\"2026-05-13T00:00:00Z\",\"schema_version\":1,\"session_id\":\"s_001\",\"created_at\":\"2026-05-13T00:00:00Z\",\"kuku_version\":\"0.1.0\"}\n",
            "  \n",
            "{\"id\":2,\"type\":\"turn.start\",\"turn\":1,\"ts\":\"2026-05-13T00:00:01Z\"}\n",
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

    store.append(session_meta()).unwrap();

    let contents = std::fs::read_to_string(&path).unwrap();
    assert!(contents.ends_with('\n'));
}

#[test]
fn concurrent_handles_do_not_reuse_the_same_event_id() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let mut left = EventStore::open(&path).unwrap();
    let mut right = EventStore::open(&path).unwrap();

    let first = left.append(session_meta()).unwrap();
    let second = right
        .append(EventPayload::TurnStart {
            turn: 1,
            ts: "2026-05-13T00:00:01Z".to_string(),
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
        store.append(session_meta()).unwrap();
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

    store
        .append(EventPayload::ContextPrelude {
            ts: "2026-05-13T00:00:00Z".to_string(),
            messages: vec![kuku::event::types::ContextMessage {
                role: "user".to_string(),
                content: "<kuku_execution_context>workspace: /tmp</kuku_execution_context>"
                    .to_string(),
            }],
        })
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
            turn: 1,
            ts: "2026-05-13T00:00:00Z".to_string(),
            request_id: "req_1".to_string(),
            text: "answer".to_string(),
            thinking: Some("reasoning".to_string()),
            input_tokens_total: Some(123),
        })
        .unwrap();
    store
        .append(EventPayload::ModelError {
            turn: 1,
            ts: "2026-05-13T00:00:01Z".to_string(),
            request_id: "req_1".to_string(),
            kind: "RateLimited".to_string(),
            message: "HTTP 429: rate limited".to_string(),
        })
        .unwrap();

    let replayed = EventStore::replay(&path).unwrap();
    assert_eq!(replayed.len(), 4);

    match &replayed[0].payload {
        EventPayload::ContextPrelude { messages, .. } => {
            assert_eq!(messages.len(), 1);
            assert!(messages[0].content.contains("<kuku_execution_context>"));
        }
        other => panic!("expected context.prelude, got {other:?}"),
    }

    match &replayed[1].payload {
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

    match &replayed[2].payload {
        EventPayload::ModelError { kind, message, .. } => {
            panic!("expected model.response before model.error, got model.error kind={kind} message={message}");
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

    match &replayed[3].payload {
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
        payload: EventPayload::ContextPrelude {
            ts: "2026-05-18T00:01:00Z".to_string(),
            messages: vec![ContextMessage {
                role: "user".to_string(),
                content: "<kuku_tool_guidance>use tools</kuku_tool_guidance>".to_string(),
            }],
        },
    };
    let response = StoredEvent {
        id: 11,
        payload: EventPayload::ModelResponse {
            turn: 2,
            ts: "2026-05-18T00:01:00Z".to_string(),
            request_id: "req_2".to_string(),
            text: "hi".to_string(),
            thinking: None,
            input_tokens_total: Some(7),
        },
    };

    let prelude_json = serde_json::to_value(&event).unwrap();
    let response_json = serde_json::to_value(&response).unwrap();

    assert!(prelude_json.get("context").is_none());
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
fn replay_reads_legacy_type_events_in_read_only_mode() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    std::fs::write(
        &path,
        concat!(
            "{\"id\":1,\"type\":\"session.meta\",\"ts\":\"2026-05-13T00:00:00Z\",\"schema_version\":1,\"session_id\":\"s_001\",\"created_at\":\"2026-05-13T00:00:00Z\",\"kuku_version\":\"0.1.0\"}\n",
            "{\"id\":2,\"type\":\"user.input\",\"turn\":1,\"ts\":\"2026-05-13T00:00:01Z\",\"text\":\"hello\"}\n",
            "{\"id\":3,\"type\":\"turn.end\",\"turn\":1,\"ts\":\"2026-05-13T00:00:02Z\"}\n",
        ),
    )
    .unwrap();

    let replayed = EventStore::replay(&path).unwrap();

    assert_eq!(3, replayed.len());
    assert!(matches!(
        replayed[0].payload,
        EventPayload::SessionMeta { .. }
    ));
    assert!(matches!(
        replayed[1].payload,
        EventPayload::UserInput { .. }
    ));
    assert!(matches!(replayed[2].payload, EventPayload::TurnEnd { .. }));
}

#[test]
fn new_writes_use_kind_not_type_at_top_level() {
    let event = StoredEvent {
        id: 8,
        payload: EventPayload::Unknown(serde_json::json!({
            "id": 8,
            "ts": "2026-06-09T00:00:07Z",
            "kind": "tool.result",
            "conversation": "session://s_001/conversations/c_main",
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
fn replay_recognizes_every_new_event_kind() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let contents = format!("{}\n", new_event_lines().join("\n"));
    std::fs::write(&path, contents).unwrap();

    let replayed = EventStore::replay(&path).unwrap();

    assert_eq!(14, replayed.len());
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
        "{\"id\":1,\"ts\":\"2026-06-09T00:00:00Z\",\"kind\":\"future.event\",\"conversation\":\"session://s_001/conversations/c_main\",\"turn\":1,\"custom\":\"x\"}\n",
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
                        .append(EventPayload::TurnStart {
                            turn: index + 1,
                            ts: format!("2026-06-09T00:00:{index:02}Z"),
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
