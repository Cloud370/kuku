use kuku::session::{project_home, project_policy_path, session_events_path};

#[cfg(unix)]
#[test]
fn maps_workspace_path_under_kuku_home_without_encoding() {
    let kuku_home = std::env::temp_dir().join("kuku-home");
    let workspace = std::path::PathBuf::from("/code/kuku/example");

    let path = project_home(&kuku_home, &workspace).unwrap();

    assert_eq!(path, kuku_home.join("p").join("code/kuku/example"));
}

#[cfg(unix)]
#[test]
fn builds_session_events_path() {
    let kuku_home = std::env::temp_dir().join("kuku-home");
    let workspace = std::path::PathBuf::from("/code/kuku/example");

    let path = session_events_path(&kuku_home, &workspace, "s_001").unwrap();

    assert_eq!(
        path,
        kuku_home
            .join("p")
            .join("code/kuku/example/sessions/s_001/events.jsonl")
    );
}

#[cfg(unix)]
#[test]
fn builds_project_policy_path() {
    let kuku_home = std::env::temp_dir().join("kuku-home");
    let workspace = std::path::PathBuf::from("/code/kuku/example");

    let path = project_policy_path(&kuku_home, &workspace).unwrap();

    assert_eq!(
        path,
        kuku_home.join("p").join("code/kuku/example/policy.md")
    );
}
