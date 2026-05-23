use kuku::session::{delete_session, list_sessions};
use std::fs;
use tempfile::tempdir;

fn write_session(dir: &std::path::Path, id: &str) {
    let session_dir = dir.join(id);
    fs::create_dir_all(&session_dir).unwrap();
    fs::write(
        session_dir.join("events.jsonl"),
        "{\"id\":1,\"type\":\"session.meta\",\"ts\":\"2026-05-01T00:00:00Z\",\"schema_version\":1,\"session_id\":\"s_test\",\"created_at\":\"2026-05-01T00:00:00Z\",\"kuku_version\":\"0.1.0\"}\n{\"id\":2,\"type\":\"turn.start\",\"turn\":1,\"ts\":\"2026-05-01T00:00:01Z\"}\n{\"id\":3,\"type\":\"user.input\",\"turn\":1,\"ts\":\"2026-05-01T00:00:01Z\",\"text\":\"hello\"}\n{\"id\":4,\"type\":\"turn.end\",\"turn\":1,\"ts\":\"2026-05-01T00:00:02Z\"}\n",
    ).unwrap();
}

fn write_lock(dir: &std::path::Path, id: &str, pid: i32) {
    let lock_path = dir.join(id).join("lock");
    fs::write(&lock_path, format!("{pid}\n2026-05-01T00:00:00Z\n")).unwrap();
}

#[test]
fn delete_session_removes_directory() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let workspace = std::fs::canonicalize(dir.path()).unwrap();
    let sessions_dir = kuku::session::project_home(home, &workspace)
        .unwrap()
        .join("sessions");
    std::fs::create_dir_all(&sessions_dir).unwrap();

    write_session(&sessions_dir, "s_delete_me");

    let before = list_sessions(home, Some(&workspace)).unwrap();
    assert_eq!(before.len(), 1);

    delete_session(home, Some(&workspace), "s_delete_me").unwrap();

    let after = list_sessions(home, Some(&workspace)).unwrap();
    assert!(after.is_empty());
}

#[test]
fn delete_nonexistent_session_returns_error() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let workspace = std::fs::canonicalize(dir.path()).unwrap();

    let result = delete_session(home, Some(&workspace), "s_nonexistent");
    assert!(result.is_err());
}

#[test]
fn delete_active_session_returns_locked_error() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let workspace = std::fs::canonicalize(dir.path()).unwrap();
    let sessions_dir = kuku::session::project_home(home, &workspace)
        .unwrap()
        .join("sessions");
    std::fs::create_dir_all(&sessions_dir).unwrap();

    write_session(&sessions_dir, "s_active");
    write_lock(&sessions_dir, "s_active", std::process::id() as i32);

    let result = delete_session(home, Some(&workspace), "s_active");
    assert!(result.is_err());
    assert!(sessions_dir.join("s_active").exists());
}

#[test]
fn delete_with_dead_lock_succeeds() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let workspace = std::fs::canonicalize(dir.path()).unwrap();
    let sessions_dir = kuku::session::project_home(home, &workspace)
        .unwrap()
        .join("sessions");
    std::fs::create_dir_all(&sessions_dir).unwrap();

    write_session(&sessions_dir, "s_dead");
    write_lock(&sessions_dir, "s_dead", 99999);

    delete_session(home, Some(&workspace), "s_dead").unwrap();
    assert!(!sessions_dir.join("s_dead").exists());
}
