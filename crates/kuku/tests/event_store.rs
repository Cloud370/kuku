use std::io::ErrorKind;

use kuku::context::FileSource;
use kuku::error::Error;
use kuku::event::{EventPayload, EventStore, StoredEvent};

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
    assert_eq!(json["type"], "permission.requested");
    assert_eq!(json["candidate"], "cargo test");
    assert_eq!(event.payload.type_name(), "permission.requested");

    let back: StoredEvent = serde_json::from_value(json).unwrap();
    assert_eq!(back, event);
}
