use kuku::session::{list_sessions, SessionStatus};
use std::path::{Path, PathBuf};
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
