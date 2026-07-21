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
async fn invalid_startup_uses_stable_invalid_revision_sentinel() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("config.toml");
    std::fs::write(&path, "not valid [[[\n").unwrap();
    let revisions = ServerRevisionCoordinator::open(home.path());
    let service = ConfigService::open(path.clone(), revisions).await.unwrap();
    let first = service.revision().await.unwrap();
    std::fs::write(&path, "still invalid [[\n").unwrap();
    service.reload_from_disk().await.unwrap();
    let second = service.revision().await.unwrap();
    assert_eq!(first, second);
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
async fn deleting_config_before_commit_is_a_disk_conflict() {
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
    let expected = service.revision().await.unwrap();
    std::fs::remove_file(path).unwrap();
    let error = service
        .commit_config(ConfigPatch::replace(valid_config()), expected)
        .await
        .unwrap_err();
    assert_eq!(error.code(), kuku_server::api::ApiErrorCode::Outdated);
}

#[tokio::test]
async fn creating_config_after_missing_startup_is_a_disk_conflict() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("config.toml");
    let revisions = ServerRevisionCoordinator::open(home.path());
    let service = ConfigService::open(path.clone(), revisions).await.unwrap();
    let expected = service.revision().await.unwrap();
    std::fs::write(&path, "external = true\n").unwrap();
    let error = service
        .commit_config(ConfigPatch::replace(valid_config()), expected)
        .await
        .unwrap_err();
    assert_eq!(error.code(), kuku_server::api::ApiErrorCode::Outdated);
}

#[tokio::test]
async fn valid_reload_replaces_snapshot_by_content_digest() {
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
    std::fs::write(
        &path,
        r#"default_model = "next"
[model.next]
provider = "local"
model = "model-y"
[provider.local]
format = "openai-responses"
base_url = "http://127.0.0.1:9000"
credential = { source = "direct_value", value = "key" }
"#,
    )
    .unwrap();
    service.reload_from_disk().await.unwrap();
    assert_eq!(
        service
            .snapshot()
            .await
            .unwrap()
            .resolved
            .unwrap()
            .default_tier(),
        "next"
    );
}

#[tokio::test]
async fn valid_reload_observes_external_deletion() {
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
    std::fs::remove_file(path).unwrap();
    assert_eq!(
        service.reload_from_disk().await.unwrap(),
        PlatformState::Missing
    );
    assert!(service.snapshot().await.unwrap().resolved.is_none());
}

#[tokio::test]
async fn watcher_detects_replacement_with_preserved_mtime() {
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
    let original_mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
    let revisions = ServerRevisionCoordinator::open(home.path());
    let service = ConfigService::open(path.clone(), revisions).await.unwrap();
    let watcher =
        kuku_server::config_watcher::ConfigWatcherHandle::start(path.clone(), service.clone());
    std::fs::write(
        &path,
        r#"default_model = "next"
[model.next]
provider = "local"
model = "model-y"
[provider.local]
format = "openai-responses"
base_url = "http://127.0.0.1:9000"
credential = { source = "direct_value", value = "key" }
"#,
    )
    .unwrap();
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(original_mtime))
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(3200)).await;
    assert_eq!(
        service
            .snapshot()
            .await
            .unwrap()
            .resolved
            .unwrap()
            .default_tier(),
        "next"
    );
    watcher.shutdown().await;
}

#[tokio::test]
async fn concurrent_commit_and_reload_never_install_the_stale_snapshot() {
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
    let expected = service.revision().await.unwrap();
    std::fs::write(
        &path,
        r#"default_model = "external"
[model.external]
provider = "local"
model = "model-z"
[provider.local]
format = "openai-responses"
base_url = "http://127.0.0.1:9000"
credential = { source = "direct_value", value = "key" }
"#,
    )
    .unwrap();
    let (reload, commit) = tokio::join!(
        service.reload_from_disk(),
        service.commit_config(ConfigPatch::replace(valid_config()), expected)
    );
    assert!(reload.is_ok());
    assert!(commit.is_err());
    assert_eq!(
        service
            .snapshot()
            .await
            .unwrap()
            .resolved
            .unwrap()
            .default_tier(),
        "external"
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

#[tokio::test]
async fn watcher_does_not_reload_after_shutdown() {
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
    let watcher =
        kuku_server::config_watcher::ConfigWatcherHandle::start(path.clone(), service.clone());
    watcher.shutdown().await;
    std::fs::write(path, "default_model = [").unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert_eq!(service.state().await, PlatformState::Ready);
}
