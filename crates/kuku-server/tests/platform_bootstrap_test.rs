use std::sync::Arc;

use kuku_server::platform::{ConfigPatch, ConfigService, PlatformState, ServerRevisionCoordinator};

fn valid_config() -> kuku::config::ConfigFile {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"default_model = "custom"

[model.custom]
provider = "local"
model = "model-x"

[provider.local]
format = "openai-responses"
base_url = "http://127.0.0.1:9000"
credential = { source = "direct_value", value = "key" }
"#,
    )
    .unwrap();
    kuku::config::load_config(&path).unwrap()
}

#[tokio::test]
async fn missing_config_is_state_not_startup_failure() {
    let home = tempfile::tempdir().unwrap();
    let revisions = ServerRevisionCoordinator::open(home.path());
    let service = ConfigService::open(home.path().join("config.toml"), revisions)
        .await
        .unwrap();
    assert_eq!(service.state().await, PlatformState::Missing);
    let revision = service.revision().await.unwrap();
    assert_eq!(revision.as_str().len(), 64);
    assert!(revision
        .as_str()
        .bytes()
        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
}

#[tokio::test]
async fn stale_revision_returns_server_revision_conflict() {
    let home = tempfile::tempdir().unwrap();
    let revisions = ServerRevisionCoordinator::open(home.path());
    let service = ConfigService::open(home.path().join("config.toml"), Arc::clone(&revisions))
        .await
        .unwrap();
    let stale = service.revision().await.unwrap();
    let _ = service
        .commit_config(ConfigPatch::replace(valid_config()), stale.clone())
        .await
        .unwrap();
    let error = service
        .commit_config(ConfigPatch::replace(valid_config()), stale)
        .await
        .unwrap_err();
    assert_eq!(
        error.code(),
        kuku_server::api::ApiErrorCode::StaleServerRevision
    );
}

#[tokio::test]
async fn invalid_reload_keeps_last_good_snapshot() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("config.toml");
    std::fs::write(
        &path,
        r#"default_model = "custom"
[model.custom]
provider = "local"
model = "model-x"
[provider.local]
format = "openai-responses"
base_url = "http://127.0.0.1:9000"
credential = { source = "direct_value", value = "key" }
"#,
    )
    .unwrap();
    let revisions = ServerRevisionCoordinator::open(home.path());
    let service = ConfigService::open(path.clone(), revisions).await.unwrap();
    assert_eq!(service.state().await, PlatformState::Ready);
    std::fs::write(&path, "default_model = [").unwrap();
    assert!(matches!(
        service.reload_from_disk().await.unwrap(),
        PlatformState::Invalid { .. }
    ));
    assert!(service.snapshot().await.unwrap().resolved.is_some());
}

#[tokio::test]
async fn watcher_shutdown_is_sticky_and_joined() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("config.toml");
    let revisions = ServerRevisionCoordinator::open(home.path());
    let service = ConfigService::open(path.clone(), revisions).await.unwrap();
    let watcher = kuku_server::config_watcher::ConfigWatcherHandle::start(path, service);
    watcher.shutdown().await;
}
