use kuku::event::{EventPayload, EventStore};
use kuku::session::{list_sessions, SessionStatus};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tempfile::tempdir;

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
        "{\"id\":1,\"type\":\"session.meta\",\"ts\":\"2026-05-01T00:00:00Z\",\"schema_version\":1,\"session_id\":\"s_test1\",\"created_at\":\"2026-05-01T00:00:00Z\",\"kuku_version\":\"0.1.0\"}\n{\"id\":2,\"type\":\"turn.start\",\"turn\":1,\"ts\":\"2026-05-01T00:00:01Z\"}\n{\"id\":3,\"type\":\"user.input\",\"turn\":1,\"ts\":\"2026-05-01T00:00:01Z\",\"text\":\"hello world\"}\n{\"id\":4,\"type\":\"turn.end\",\"turn\":1,\"ts\":\"2026-05-01T00:00:02Z\"}\n",
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
        "{\"id\":1,\"type\":\"session.meta\",\"ts\":\"2026-05-01T00:00:00Z\",\"schema_version\":1,\"session_id\":\"s_aaa\",\"created_at\":\"2026-05-01T00:00:00Z\",\"kuku_version\":\"0.1.0\"}\n{\"id\":2,\"type\":\"turn.start\",\"turn\":1,\"ts\":\"2026-05-01T00:00:01Z\"}\n{\"id\":3,\"type\":\"user.input\",\"turn\":1,\"ts\":\"2026-05-01T00:00:01Z\",\"text\":\"proj a\"}\n{\"id\":4,\"type\":\"turn.end\",\"turn\":1,\"ts\":\"2026-05-01T00:00:02Z\"}\n",
    );
    std::thread::sleep(std::time::Duration::from_millis(50));
    write_session(
        &ws2,
        "s_bbb",
        "{\"id\":1,\"type\":\"session.meta\",\"ts\":\"2026-05-02T00:00:00Z\",\"schema_version\":1,\"session_id\":\"s_bbb\",\"created_at\":\"2026-05-02T00:00:00Z\",\"kuku_version\":\"0.1.0\"}\n{\"id\":2,\"type\":\"turn.start\",\"turn\":1,\"ts\":\"2026-05-02T00:00:01Z\"}\n{\"id\":3,\"type\":\"user.input\",\"turn\":1,\"ts\":\"2026-05-02T00:00:01Z\",\"text\":\"proj b\"}\n{\"id\":4,\"type\":\"turn.end\",\"turn\":1,\"ts\":\"2026-05-02T00:00:02Z\"}\n",
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
        "{\"id\":1,\"type\":\"session.meta\",\"ts\":\"2026-05-01T00:00:00Z\",\"schema_version\":1,\"session_id\":\"s_corrupt\",\"created_at\":\"2026-05-01T00:00:00Z\",\"kuku_version\":\"0.1.0\"}\nNOT JSON HERE\n{\"id\":2,\"type\":\"turn.start\",\"turn\":1,\"ts\":\"2026-05-01T00:00:01Z\"}\n{\"id\":3,\"type\":\"user.input\",\"turn\":1,\"ts\":\"2026-05-01T00:00:01Z\",\"text\":\"hello\"}\n{\"id\":4,\"type\":\"turn.end\",\"turn\":1,\"ts\":\"2026-05-01T00:00:02Z\"}\n",
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
        "{\"id\":1,\"type\":\"session.meta\",\"ts\":\"2026-05-01T00:00:00Z\",\"schema_version\":1,\"session_id\":\"s_meta_only\",\"created_at\":\"2026-05-01T00:00:00Z\",\"kuku_version\":\"0.1.0\"}\n",
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
        "{\"id\":1,\"type\":\"session.meta\",\"ts\":\"2026-05-01T00:00:00Z\",\"schema_version\":1,\"session_id\":\"s_keep\",\"created_at\":\"2026-05-01T00:00:00Z\",\"kuku_version\":\"0.1.0\"}\n{\"id\":2,\"type\":\"turn.start\",\"turn\":1,\"ts\":\"2026-05-01T00:00:01Z\"}\n{\"id\":3,\"type\":\"user.input\",\"turn\":1,\"ts\":\"2026-05-01T00:00:01Z\",\"text\":\"keep session\"}\n{\"id\":4,\"type\":\"turn.end\",\"turn\":1,\"ts\":\"2026-05-01T00:00:02Z\"}\n",
    );
    let events_path = sessions_dir.join("s_keep/events.jsonl");
    let before = EventStore::replay(&events_path).unwrap();
    assert_eq!(before.len(), 4);

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
    assert_eq!(replayed.len(), 4);
    assert!(matches!(
        &replayed[0].payload,
        EventPayload::SessionMeta { session_id, .. } if session_id == "s_keep"
    ));
}
