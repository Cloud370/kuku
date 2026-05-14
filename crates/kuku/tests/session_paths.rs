use kuku::session::{project_home, project_policy_path, session_events_path};

#[test]
fn maps_workspace_path_under_kuku_home_without_encoding() {
    let kuku_home = std::path::Path::new("/tmp/kuku-home");
    let workspace = std::path::Path::new("/code/kuku/example");

    let path = project_home(kuku_home, workspace).unwrap();

    assert_eq!(
        path,
        std::path::Path::new("/tmp/kuku-home/p/code/kuku/example")
    );
}

#[test]
fn builds_session_events_path() {
    let kuku_home = std::path::Path::new("/tmp/kuku-home");
    let workspace = std::path::Path::new("/code/kuku/example");

    let path = session_events_path(kuku_home, workspace, "s_001").unwrap();

    assert_eq!(
        path,
        std::path::Path::new("/tmp/kuku-home/p/code/kuku/example/sessions/s_001/events.jsonl")
    );
}

#[test]
fn builds_project_policy_path() {
    let kuku_home = std::path::Path::new("/tmp/kuku-home");
    let workspace = std::path::Path::new("/code/kuku/example");

    let path = project_policy_path(kuku_home, workspace).unwrap();

    assert_eq!(
        path,
        std::path::Path::new("/tmp/kuku-home/p/code/kuku/example/policy.md")
    );
}
