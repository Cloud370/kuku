use std::io::ErrorKind;

use kuku::error::Error;
use kuku::event::{EventPayload, EventStore};

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
