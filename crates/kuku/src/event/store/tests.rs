use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::time::Duration;

use super::{EventPayload, EventStore};

mod allocation;

fn payload() -> EventPayload {
    EventPayload::Unknown(serde_json::json!({"kind": "runtime.test"}))
}

#[test]
fn shared_open_and_append_do_not_rescan_committed_history() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let mut first = EventStore::open(&path).unwrap();
    first.append(payload()).unwrap();

    OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .write_all(b"!")
        .unwrap();
    let mut shared = EventStore::open(&path).unwrap();
    assert_eq!(2, shared.append(payload()).unwrap().id);
    assert_eq!(2, first.last_id());

    drop(first);
    drop(shared);
    assert!(EventStore::open(&path).is_err());
}

#[test]
fn paged_reads_seek_past_earlier_history() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let contents: String = (1..=300)
        .map(|id| format!("{{\"id\":{id},\"kind\":\"runtime.test\"}}\n"))
        .collect();
    std::fs::write(&path, contents).unwrap();
    let mut store = EventStore::open(&path).unwrap();
    let mut file = OpenOptions::new().write(true).open(&path).unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    file.write_all(b"!").unwrap();

    let events = store.read_after(290, 20).unwrap();
    assert_eq!(
        (291..=300).collect::<Vec<u64>>(),
        events.iter().map(|e| e.id).collect::<Vec<_>>()
    );
    assert!(store.read_after(0, 1).is_err());
    assert_eq!(301, store.append(payload()).unwrap().id);
}

#[test]
fn write_failure_stops_all_shared_handles() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let mut store = EventStore::open(&path).unwrap();
    let mut shared = store.clone();
    store.inner.writer.lock().unwrap().file = std::fs::File::open(&path).unwrap();

    assert!(store.append(payload()).is_err());
    assert_eq!(0, shared.last_id());
    let error = shared.append(payload()).unwrap_err();
    assert!(error
        .to_string()
        .contains("stopped after a durability failure"));
    assert!(std::fs::read(&path).unwrap().is_empty());
}

#[tokio::test]
async fn shared_handles_notify_after_a_durable_commit() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let mut store = EventStore::open(&path).unwrap();
    let shared = store.clone();
    let changed = shared.changed();
    let notification = changed.notified();
    tokio::pin!(notification);
    notification.as_mut().enable();

    let committed = store.append(payload()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), notification)
        .await
        .unwrap();
    assert_eq!(1, shared.last_id());
    assert_eq!(vec![committed], shared.read_after(0, 1).unwrap());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn sync_failure_does_not_publish_or_allow_later_commits() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let mut store = EventStore::open(&path).unwrap();
    let mut shared = store.clone();
    let changed = shared.changed();
    let notification = changed.notified();
    tokio::pin!(notification);
    notification.as_mut().enable();
    store.inner.writer.lock().unwrap().file = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/null")
        .unwrap();

    let error = store.append(payload()).unwrap_err();
    assert!(error.to_string().contains("cannot commit event 1"));
    assert_eq!(0, store.last_id());
    assert!(shared.read_after(0, 1).is_err());
    assert!(
        tokio::time::timeout(Duration::from_millis(20), notification)
            .await
            .is_err()
    );
    store.inner.writer.lock().unwrap().file = OpenOptions::new()
        .read(true)
        .append(true)
        .open(&path)
        .unwrap();
    assert!(shared.append(payload()).is_err());
    assert!(std::fs::read(&path).unwrap().is_empty());

    drop(store);
    drop(shared);
    let mut reopened = EventStore::open(&path).unwrap();
    assert_eq!(1, reopened.append(payload()).unwrap().id);
}

#[test]
fn nesting_limits_never_acknowledge_an_unreadable_record() {
    for known in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.jsonl");
        let mut store = EventStore::open(&path).unwrap();
        let mut accepted = 0;
        for depth in [0, 1, 32, 120, 125, 126, 127, 128, 129, 150, 256] {
            let mut nested = serde_json::Value::Null;
            for _ in 0..depth {
                nested = serde_json::Value::Array(vec![nested]);
            }
            let event = if known {
                EventPayload::ToolCall {
                    turn: 1,
                    ts: String::new(),
                    conversation: None,
                    tool_call_id: "call".to_string(),
                    request_id: "request".to_string(),
                    index: 0,
                    tool: "echo".to_string(),
                    args: nested,
                }
            } else {
                EventPayload::Unknown(serde_json::json!({
                    "kind": "runtime.test", "data": nested,
                }))
            };
            let before_len = std::fs::metadata(&path).unwrap().len();
            match store.append(event) {
                Ok(event) => {
                    assert!(depth < 150, "deep record was acknowledged at depth {depth}");
                    accepted += 1;
                    assert_eq!(accepted, event.id);
                    assert_eq!(1, store.read_after(accepted - 1, 1).unwrap().len());
                }
                Err(error) => {
                    assert!(depth > 32, "shallow record was rejected: {error}");
                    assert!(matches!(error, crate::error::Error::InvalidArgument(_)));
                    assert_eq!(before_len, std::fs::metadata(&path).unwrap().len());
                    assert_eq!(accepted, store.last_id());
                }
            }
        }
        assert_eq!(accepted + 1, store.append(payload()).unwrap().id);
        drop(store);
        let reopened = EventStore::open(&path).unwrap();
        assert_eq!(accepted + 1, reopened.last_id());
        assert_eq!(
            accepted as usize + 1,
            EventStore::replay(&path).unwrap().len()
        );
    }
}

#[test]
fn nesting_probe_ignores_escaped_string_contents() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let mut store = EventStore::open(&path).unwrap();
    let text = "\\\"[{\\".repeat(1000);
    let event = EventPayload::MessageUser {
        ts: String::new(),
        conversation: "main".to_string(),
        turn: 1,
        text: text.clone(),
        from: None,
        via_tool_call_id: None,
    };
    store.append(event).unwrap();
    store
        .append(EventPayload::Unknown(serde_json::json!({
            "kind": "runtime.test", "text": text,
        })))
        .unwrap();
    assert_eq!(2, store.read_after(0, 256).unwrap().len());
    drop(store);
    assert_eq!(2, EventStore::replay(&path).unwrap().len());
}
