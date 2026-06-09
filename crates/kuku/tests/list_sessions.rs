use kuku::event::{EventPayload, EventStore};
use kuku::session::{list_sessions, SessionStatus};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tempfile::tempdir;

fn session_events(session_id: &str, created_at: &str, prompt: &str) -> String {
    format!(
        concat!(
            "{{\"id\":1,\"ts\":\"{created_at}\",\"kind\":\"session.created\",\"schema_version\":2,\"session_id\":\"{session_id}\",\"created_at\":\"{created_at}\",\"kuku_version\":\"0.1.0\"}}\n",
            "{{\"id\":2,\"ts\":\"{created_at}\",\"kind\":\"conversation.opened\",\"conversation\":\"main\"}}\n",
            "{{\"id\":3,\"ts\":\"{created_at}\",\"kind\":\"message.user\",\"conversation\":\"main\",\"turn\":1,\"text\":\"{prompt}\"}}\n",
            "{{\"id\":4,\"ts\":\"{created_at}\",\"kind\":\"turn.started\",\"conversation\":\"main\",\"turn\":1}}\n",
            "{{\"id\":5,\"ts\":\"{created_at}\",\"kind\":\"turn.completed\",\"conversation\":\"main\",\"turn\":1}}\n"
        ),
        session_id = session_id,
        created_at = created_at,
        prompt = prompt,
    )
}

fn write_session(dir: &Path, id: &str, events: &str) {
    let session_dir = dir.join(id);
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(session_dir.join("events.jsonl"), events).unwrap();
}

fn expected_sessions_dir(kuku_home: &Path, workspace: &Path) -> PathBuf {
    kuku::session::project_home(kuku_home, workspace)
        .unwrap()
        .join("sessions")
}

#[test]
fn empty_workspace_returns_empty_list() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let workspace = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::create_dir_all(expected_sessions_dir(home, &workspace)).unwrap();

    let sessions = list_sessions(home, Some(&workspace)).unwrap();
    assert!(sessions.is_empty());
}

#[test]
fn finds_sessions_in_workspace() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let workspace = std::fs::canonicalize(dir.path()).unwrap();
    let sessions_dir = expected_sessions_dir(home, &workspace);
    std::fs::create_dir_all(&sessions_dir).unwrap();

    write_session(
        &sessions_dir,
        "s_test1",
        &session_events("s_test1", "2026-05-01T00:00:00Z", "hello world"),
    );

    let sessions = list_sessions(home, Some(&workspace)).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, "s_test1");
    assert_eq!(sessions[0].title, "hello world");
    assert_eq!(sessions[0].created_at, "2026-05-01T00:00:00Z");
    assert_eq!(sessions[0].turn_count, 1);
    assert_eq!(sessions[0].status, SessionStatus::Done);
    assert_eq!(sessions[0].workspace, workspace);
}

#[test]
fn finds_legacy_type_tagged_sessions_in_workspace() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let workspace = std::fs::canonicalize(dir.path()).unwrap();
    let sessions_dir = expected_sessions_dir(home, &workspace);
    std::fs::create_dir_all(&sessions_dir).unwrap();

    write_session(
        &sessions_dir,
        "s_legacy",
        concat!(
            "{\"id\":1,\"ts\":\"2026-05-01T00:00:00Z\",\"type\":\"session.meta\",\"schema_version\":1,\"session_id\":\"s_legacy\",\"created_at\":\"2026-05-01T00:00:00Z\",\"kuku_version\":\"0.1.0\"}\n",
            "{\"id\":2,\"ts\":\"2026-05-01T00:00:01Z\",\"type\":\"user.input\",\"turn\":1,\"text\":\"legacy prompt\"}\n",
            "{\"id\":3,\"ts\":\"2026-05-01T00:00:02Z\",\"type\":\"turn.start\",\"turn\":1}\n",
            "{\"id\":4,\"ts\":\"2026-05-01T00:00:03Z\",\"type\":\"turn.end\",\"turn\":1}\n",
        ),
    );

    let sessions = list_sessions(home, Some(&workspace)).unwrap();

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].title, "legacy prompt");
    assert_eq!(sessions[0].created_at, "2026-05-01T00:00:00Z");
    assert_eq!(sessions[0].turn_count, 1);
    assert_eq!(sessions[0].status, SessionStatus::Done);
}

#[test]
fn global_lists_all_workspaces() {
    let dir = tempdir().unwrap();
    let home = dir.path();

    let ws1 = home
        .join("p")
        .join("code")
        .join("project-a")
        .join("sessions");
    let ws2 = home
        .join("p")
        .join("code")
        .join("project-b")
        .join("sessions");
    std::fs::create_dir_all(&ws1).unwrap();
    std::fs::create_dir_all(&ws2).unwrap();

    write_session(
        &ws1,
        "s_aaa",
        &session_events("s_aaa", "2026-05-01T00:00:00Z", "proj a"),
    );
    std::thread::sleep(std::time::Duration::from_millis(50));
    write_session(
        &ws2,
        "s_bbb",
        &session_events("s_bbb", "2026-05-02T00:00:00Z", "proj b"),
    );

    let sessions = list_sessions(home, None).unwrap();
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].session_id, "s_bbb");
    assert_eq!(sessions[1].session_id, "s_aaa");
}

#[test]
fn zero_byte_session_returns_with_defaults() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let workspace = std::fs::canonicalize(dir.path()).unwrap();
    let sessions_dir = expected_sessions_dir(home, &workspace);
    std::fs::create_dir_all(&sessions_dir).unwrap();

    write_session(&sessions_dir, "s_empty", "");

    let sessions = list_sessions(home, Some(&workspace)).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].status, SessionStatus::Interrupted);
    assert!(sessions[0].title.is_empty());
    assert_eq!(sessions[0].turn_count, 0);
}

#[test]
fn corrupted_json_lines_are_skipped() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let workspace = std::fs::canonicalize(dir.path()).unwrap();
    let sessions_dir = expected_sessions_dir(home, &workspace);
    std::fs::create_dir_all(&sessions_dir).unwrap();

    write_session(
        &sessions_dir,
        "s_corrupt",
        &(session_events("s_corrupt", "2026-05-01T00:00:00Z", "hello").replacen(
            '\n',
            "\nNOT JSON HERE\n",
            1,
        )),
    );

    let sessions = list_sessions(home, Some(&workspace)).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].title, "hello");
    assert_eq!(sessions[0].turn_count, 1);
    assert_eq!(sessions[0].status, SessionStatus::Done);
}

#[test]
fn session_with_only_meta_has_zero_turns() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let workspace = std::fs::canonicalize(dir.path()).unwrap();
    let sessions_dir = expected_sessions_dir(home, &workspace);
    std::fs::create_dir_all(&sessions_dir).unwrap();

    write_session(
        &sessions_dir,
        "s_meta_only",
        "{\"id\":1,\"ts\":\"2026-05-01T00:00:00Z\",\"kind\":\"session.created\",\"schema_version\":2,\"session_id\":\"s_meta_only\",\"created_at\":\"2026-05-01T00:00:00Z\",\"kuku_version\":\"0.1.0\"}\n",
    );

    let sessions = list_sessions(home, Some(&workspace)).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].turn_count, 0);
    assert!(sessions[0].title.is_empty());
    assert_eq!(sessions[0].status, SessionStatus::Interrupted);
    assert_eq!(sessions[0].created_at, "2026-05-01T00:00:00Z");
}

#[test]
fn pruning_logs_does_not_break_session_listing() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let workspace = std::fs::canonicalize(dir.path()).unwrap();
    let sessions_dir = expected_sessions_dir(home, &workspace);
    std::fs::create_dir_all(&sessions_dir).unwrap();
    write_session(
        &sessions_dir,
        "s_keep",
        &session_events("s_keep", "2026-05-01T00:00:00Z", "keep session"),
    );
    let events_path = sessions_dir.join("s_keep/events.jsonl");
    let before = EventStore::replay(&events_path).unwrap();
    assert_eq!(before.len(), 5);

    let logs = home.join("logs/runtime");
    std::fs::create_dir_all(&logs).unwrap();
    std::fs::write(logs.join("old.jsonl"), vec![b'x'; 2 * 1024 * 1024]).unwrap();
    std::fs::write(logs.join("events.jsonl"), vec![b'e'; 2 * 1024 * 1024]).unwrap();

    let limits = kuku::config::LogsConfig {
        max_age_days: 14,
        max_total_size_mb: 1,
    };
    kuku::log::prune_logs(
        home,
        &limits,
        SystemTime::now(),
        kuku::log::PruneOptions::default(),
    )
    .unwrap();

    let sessions = list_sessions(home, Some(&workspace)).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, "s_keep");
    assert_eq!(sessions[0].title, "keep session");
    assert_eq!(sessions[0].status, SessionStatus::Done);
    assert!(events_path.exists());
    let replayed = EventStore::replay(&events_path).unwrap();
    assert_eq!(replayed.len(), 5);
    assert!(matches!(
        &replayed[0].payload,
        EventPayload::SessionCreated { session_id, .. } if session_id == "s_keep"
    ));
}

#[test]
fn oversized_final_event_line_still_marks_session_done() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let workspace = std::fs::canonicalize(dir.path()).unwrap();
    let sessions_dir = expected_sessions_dir(home, &workspace);
    std::fs::create_dir_all(&sessions_dir).unwrap();

    let large_text = "x".repeat(5000);
    write_session(
        &sessions_dir,
        "s_large_tail",
        &format!(
            "{{\"id\":1,\"ts\":\"2026-05-01T00:00:00Z\",\"kind\":\"session.created\",\"schema_version\":2,\"session_id\":\"s_large_tail\",\"created_at\":\"2026-05-01T00:00:00Z\",\"kuku_version\":\"0.1.0\"}}\n{{\"id\":2,\"ts\":\"2026-05-01T00:00:00Z\",\"kind\":\"conversation.opened\",\"conversation\":\"main\"}}\n{{\"id\":3,\"ts\":\"2026-05-01T00:00:01Z\",\"kind\":\"message.user\",\"conversation\":\"main\",\"turn\":1,\"text\":\"hello\"}}\n{{\"id\":4,\"ts\":\"2026-05-01T00:00:01Z\",\"kind\":\"turn.started\",\"conversation\":\"main\",\"turn\":1}}\n{{\"id\":5,\"ts\":\"2026-05-01T00:00:02Z\",\"kind\":\"turn.completed\",\"conversation\":\"main\",\"turn\":1,\"summary\":\"{}\"}}\n",
            large_text
        ),
    );

    let sessions = list_sessions(home, Some(&workspace)).unwrap();

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].status, SessionStatus::Done);
}
