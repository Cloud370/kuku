use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use super::{EventPayload, EventStore, LockedJournal};

mod allocation;

fn payload() -> EventPayload {
    EventPayload::Unknown(serde_json::json!({"kind": "runtime.test"}))
}

fn corrupt_committed_prefix(store: &EventStore, path: &Path) {
    let writer = store.inner.writer.lock().unwrap();
    let file = &writer.journal.as_ref().unwrap().file;
    // NOTE: Windows locks also prevent the deliberate external corruption used by these probes.
    file.unlock().unwrap();
    let corrupted = OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|mut file| file.write_all(b"!"));
    super::fs_sync::lock_writer(file, path).unwrap();
    corrupted.unwrap();
}

fn replace_writer_file(store: &EventStore, file: File) {
    let mut writer = store.inner.writer.lock().unwrap();
    drop(writer.journal.take());
    writer.journal = Some(LockedJournal::writer(file, &store.inner.path).unwrap());
}

#[test]
fn closing_the_writer_releases_the_journal_lock_with_a_duplicate_handle_open() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let mut store = EventStore::open(&path).unwrap();
    store.append(payload()).unwrap();
    let duplicate = {
        let writer = store.inner.writer.lock().unwrap();
        writer.journal.as_ref().unwrap().file.try_clone().unwrap()
    };

    drop(store);
    let mut reopened = EventStore::open(&path).unwrap();
    assert_eq!(2, reopened.append(payload()).unwrap().id);
    drop(duplicate);
}

#[test]
fn releasing_a_journal_reader_allows_writes_with_a_duplicate_handle_open() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    drop(EventStore::open(&path).unwrap());
    let reader = LockedJournal::reader(&path).unwrap();
    let duplicate = reader.file.try_clone().unwrap();
    assert!(EventStore::open(&path).is_err());

    drop(reader);
    let mut writer = EventStore::open(&path).unwrap();
    assert_eq!(1, writer.append(payload()).unwrap().id);
    drop(duplicate);
}

#[test]
fn shared_open_and_append_do_not_rescan_committed_history() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let mut first = EventStore::open(&path).unwrap();
    first.append(payload()).unwrap();

    corrupt_committed_prefix(&first, &path);
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
    corrupt_committed_prefix(&store, &path);

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
    replace_writer_file(&store, File::open(&path).unwrap());

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
    replace_writer_file(
        &store,
        OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/null")
            .unwrap(),
    );

    let error = store.append(payload()).unwrap_err();
    assert!(error.to_string().contains("cannot commit event 1"));
    assert_eq!(0, store.last_id());
    assert!(shared.read_after(0, 1).is_err());
    assert!(
        tokio::time::timeout(Duration::from_millis(20), notification)
            .await
            .is_err()
    );
    replace_writer_file(
        &store,
        OpenOptions::new()
            .read(true)
            .append(true)
            .open(&path)
            .unwrap(),
    );
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

#[test]
fn static_pages_do_not_open_a_writer_or_repair_an_incomplete_tail() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let contents =
        "{\"id\":1,\"kind\":\"runtime.test\"}\n{\"id\":2,\"kind\":\"runtime.test\"}\n{\"id\":";
    std::fs::write(&path, contents).unwrap();

    assert_eq!(1, EventStore::read_page(&path, 0, 1).unwrap()[0].id);
    assert_eq!(2, EventStore::read_page(&path, 1, 256).unwrap()[0].id);
    assert!(EventStore::read_page(&path, 2, 256).unwrap().is_empty());
    assert_eq!(contents.as_bytes(), std::fs::read(&path).unwrap());
    assert!(super::find_active_store(
        &super::registry().lock().unwrap(),
        &std::fs::canonicalize(&path).unwrap(),
    )
    .is_none());
    let missing = temp.path().join("missing").join("events.jsonl");
    assert!(EventStore::read_page(&missing, 0, 256).unwrap().is_empty());
    assert!(!missing.parent().unwrap().exists());
}

#[test]
fn static_pages_share_the_active_writer_and_keep_count_and_byte_limits() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let contents: String = (1..=600)
        .map(|id| format!("{{\"id\":{id},\"kind\":\"runtime.test\"}}\n"))
        .collect();
    std::fs::write(&path, contents).unwrap();
    let store = EventStore::open(&path).unwrap();
    assert_eq!(
        store.read_after(257, 256).unwrap(),
        EventStore::read_page(&path, 257, usize::MAX).unwrap()
    );
    drop(store);
    assert_eq!(
        256,
        EventStore::read_page(&path, 0, usize::MAX).unwrap().len()
    );
    assert_eq!(600, EventStore::read_page(&path, 599, 256).unwrap()[0].id);
    assert!(EventStore::read_page(&path, 0, 0).unwrap().is_empty());

    let mut store = EventStore::open(&path).unwrap();
    for _ in 0..3 {
        store
            .append(EventPayload::Unknown(serde_json::json!({
                "kind": "runtime.test", "data": "x".repeat(600 * 1024),
            })))
            .unwrap();
    }
    drop(store);
    let page = EventStore::read_page(&path, 600, 256).unwrap();
    assert_eq!(
        vec![601, 602],
        page.into_iter().map(|event| event.id).collect::<Vec<_>>()
    );
    assert_eq!(603, EventStore::read_page(&path, 602, 256).unwrap()[0].id);
}

#[cfg(unix)]
#[test]
fn stores_open_below_unmodified_unlistable_ancestors() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let ancestor = temp.path().join("ancestor");
    let durable_root = ancestor.join("data");
    std::fs::create_dir_all(&durable_root).unwrap();
    let direct = durable_root.join("direct.jsonl");
    let managed = durable_root
        .join("sessions")
        .join("main")
        .join("events.jsonl");
    std::fs::set_permissions(&ancestor, std::fs::Permissions::from_mode(0o111)).unwrap();
    let direct_result = EventStore::open(&direct);
    let managed_result = EventStore::open(&managed);
    std::fs::set_permissions(&ancestor, std::fs::Permissions::from_mode(0o700)).unwrap();

    let mut direct_store = direct_result.unwrap();
    let mut managed_store = managed_result.unwrap();
    assert_eq!(1, direct_store.append(payload()).unwrap().id);
    assert_eq!(1, managed_store.append(payload()).unwrap().id);
    drop(direct_store);
    drop(managed_store);
    assert_eq!(1, EventStore::replay(&direct).unwrap().len());
    assert_eq!(1, EventStore::replay(&managed).unwrap().len());
}
