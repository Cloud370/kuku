use kuku_server::api::ApiErrorCode;
use kuku_server::platform::{
    accepted_digest, write_private_atomic, RevisionDomain, ServerInstanceLock,
    ServerRevisionCoordinator,
};

#[test]
fn web_home_is_private_and_single_instance() {
    let home = tempfile::tempdir().unwrap();
    let state = home.path().join("state.json");
    write_private_atomic(&state, br#"{"ok":true}"#).unwrap();
    assert_eq!(std::fs::read(&state).unwrap(), br#"{"ok":true}"#);
    write_private_atomic(&state, br#"{"ok":false}"#).unwrap();
    assert_eq!(std::fs::read(&state).unwrap(), br#"{"ok":false}"#);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&state).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    let first = ServerInstanceLock::acquire(home.path()).unwrap();
    assert!(ServerInstanceLock::acquire(home.path()).is_err());
    drop(first);
    assert!(ServerInstanceLock::acquire(home.path()).is_ok());
}

#[test]
fn accepted_digest_is_sha256() {
    let digest = accepted_digest(b"abc");
    let encoded = digest
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(
        encoded,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[tokio::test]
async fn server_revision_is_one_opaque_digest_for_all_platform_files() {
    let home = tempfile::tempdir().unwrap();
    let revisions = ServerRevisionCoordinator::open(home.path());
    let before = revisions.current().await.unwrap();
    let guard = revisions.begin(&before).await.unwrap();
    let after = guard
        .finish(
            RevisionDomain::Init,
            accepted_digest(br#"{"complete":false}"#),
        )
        .await
        .unwrap();
    assert_ne!(before, after);
    assert_eq!(after.as_str().len(), 64);
    assert!(after
        .as_str()
        .bytes()
        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
    let error = revisions.begin(&before).await.unwrap_err();
    assert_eq!(error.code(), ApiErrorCode::StaleServerRevision);
}

#[tokio::test]
async fn stale_guard_does_not_change_accepted_revision() {
    let home = tempfile::tempdir().unwrap();
    let revisions = ServerRevisionCoordinator::open(home.path());
    let before = revisions.current().await.unwrap();
    let first = revisions.begin(&before).await.unwrap();
    let _ = first
        .finish(RevisionDomain::Config, accepted_digest(b"one"))
        .await
        .unwrap();
    let stale = revisions.begin(&before).await.unwrap_err();
    assert_eq!(stale.code(), ApiErrorCode::StaleServerRevision);
    let current = revisions.current().await.unwrap();
    assert_ne!(before, current);
}

#[tokio::test]
async fn probe_revision_excludes_init_and_settings_domains() {
    let home = tempfile::tempdir().unwrap();
    let revisions = ServerRevisionCoordinator::open(home.path());
    revisions
        .register_initial(RevisionDomain::Config, accepted_digest(b"config"))
        .await;
    revisions
        .register_initial(RevisionDomain::Workspace, accepted_digest(b"workspace"))
        .await;
    let before = revisions.probe_inputs().await.unwrap().token().clone();
    revisions
        .register_initial(RevisionDomain::Init, accepted_digest(b"init"))
        .await;
    revisions
        .register_initial(RevisionDomain::Settings, accepted_digest(b"settings"))
        .await;
    let after = revisions.probe_inputs().await.unwrap().token().clone();
    assert_eq!(before, after);
}
