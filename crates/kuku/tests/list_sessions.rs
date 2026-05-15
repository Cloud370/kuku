use kuku::session::list_sessions;
use std::path::{Component, Path, PathBuf};
use tempfile::tempdir;

/// Mirror project_home logic: kuku_home/p/<workspace normal components>
fn expected_sessions_dir(kuku_home: &Path, workspace: &Path) -> PathBuf {
    let mut path = kuku_home.to_path_buf();
    path.push("p");
    for component in workspace.components() {
        match component {
            Component::RootDir | Component::Prefix(_) | Component::CurDir => {}
            Component::Normal(part) => path.push(part),
            Component::ParentDir => {}
        }
    }
    path.push("sessions");
    path
}

#[test]
fn empty_workspace_returns_empty_list() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let workspace = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::create_dir_all(expected_sessions_dir(home, &workspace)).unwrap();

    let sessions = list_sessions(home, &workspace).unwrap();
    assert!(sessions.is_empty());
}

#[test]
fn finds_sessions_in_workspace() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let workspace = std::fs::canonicalize(dir.path()).unwrap();
    let sessions_dir = expected_sessions_dir(home, &workspace);
    std::fs::create_dir_all(&sessions_dir).unwrap();

    let s1 = sessions_dir.join("s_x4f2a");
    std::fs::create_dir(&s1).unwrap();
    let events = s1.join("events.jsonl");
    std::fs::write(
        &events,
        "{\"id\":1,\"type\":\"session.meta\",\"ts\":\"2026-05-01T00:00:00Z\",\"schema_version\":1,\"session_id\":\"s_x4f2a\",\"created_at\":\"2026-05-01T00:00:00Z\",\"kuku_version\":\"0.1.0\"}\n{\"id\":2,\"type\":\"turn.start\",\"turn\":1,\"ts\":\"2026-05-01T00:00:01Z\"}\n",
    )
    .unwrap();

    let sessions = list_sessions(home, &workspace).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, "s_x4f2a");
    assert_eq!(sessions[0].turn_count, 1);
}
