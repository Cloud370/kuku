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
