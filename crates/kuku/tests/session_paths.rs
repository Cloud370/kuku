use kuku::session::{project_home, project_policy_path, session_events_path};

#[test]
fn maps_unix_workspace_path_under_kuku_home() {
    let kuku_home = std::env::temp_dir().join("kuku-home");
    let workspace = std::path::PathBuf::from("/code/kuku/example");

    let path = project_home(&kuku_home, &workspace).unwrap();

    assert_eq!(path, kuku_home.join("p").join("code/kuku/example"));
}

#[cfg(windows)]
#[test]
fn maps_windows_drive_path_with_drive_letter() {
    let kuku_home = std::env::temp_dir().join("kuku-home");
    let workspace = std::path::PathBuf::from("C:\\code\\kuku\\example");

    let path = project_home(&kuku_home, &workspace).unwrap();

    assert_eq!(
        path,
        kuku_home.join("p").join("C").join("code/kuku/example")
    );
}

#[cfg(windows)]
#[test]
fn maps_verbatim_drive_path_same_as_standard() {
    let kuku_home = std::env::temp_dir().join("kuku-home");

    let standard = project_home(&kuku_home, &std::path::PathBuf::from("C:\\foo")).unwrap();
    let verbatim = project_home(&kuku_home, &std::path::PathBuf::from("\\\\?\\C:\\foo")).unwrap();

    assert_eq!(standard, verbatim);
}

#[cfg(windows)]
#[test]
fn distinct_drives_map_to_distinct_paths() {
    let kuku_home = std::env::temp_dir().join("kuku-home");

    let c_path = project_home(&kuku_home, &std::path::PathBuf::from("C:\\foo")).unwrap();
    let d_path = project_home(&kuku_home, &std::path::PathBuf::from("D:\\foo")).unwrap();

    assert_ne!(c_path, d_path);
    assert!(c_path.ends_with("C/foo"));
    assert!(d_path.ends_with("D/foo"));
}

#[cfg(windows)]
#[test]
fn drive_root_gets_namespace() {
    let kuku_home = std::env::temp_dir().join("kuku-home");
    let workspace = std::path::PathBuf::from("W:\\");

    let path = project_home(&kuku_home, &workspace).unwrap();

    assert!(path.ends_with("W"));
}

#[cfg(windows)]
#[test]
fn unc_path_splits_server_share() {
    let kuku_home = std::env::temp_dir().join("kuku-home");
    let workspace = std::path::PathBuf::from("\\\\server\\share\\foo");

    let path = project_home(&kuku_home, &workspace).unwrap();

    assert!(path.ends_with("server/share/foo"));
}

#[cfg(windows)]
#[test]
fn different_unc_servers_do_not_collide() {
    let kuku_home = std::env::temp_dir().join("kuku-home");

    let a = project_home(
        &kuku_home,
        &std::path::PathBuf::from("\\\\srv1\\share\\foo"),
    )
    .unwrap();
    let b = project_home(
        &kuku_home,
        &std::path::PathBuf::from("\\\\srv2\\share\\foo"),
    )
    .unwrap();

    assert_ne!(a, b);
}

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
