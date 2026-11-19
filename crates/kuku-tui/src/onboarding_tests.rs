use std::fs;

use kuku_client::ConnectionOptions;

use super::should_offer_setup;

#[test]
fn live_service_with_custom_config_skips_missing_default_config() {
    let directory = tempfile::tempdir().unwrap();
    let data_dir = directory.path().join("data");
    let active_config = directory.path().join("custom.toml");
    fs::write(&active_config, "# active config").unwrap();
    let options = ConnectionOptions {
        data_dir: Some(data_dir.clone()),
        ..ConnectionOptions::default()
    };
    let candidate = options.local_service().unwrap().config_path().to_owned();
    assert!(!candidate.exists());
    assert!(active_config.exists());

    assert!(!should_offer_setup(Ok(true), true));
}

#[tokio::test]
async fn stale_discovery_without_a_live_service_still_allows_setup() {
    let directory = tempfile::tempdir().unwrap();
    let options = ConnectionOptions {
        data_dir: Some(directory.path().join("data")),
        ..ConnectionOptions::default()
    };
    let service = options.local_service().unwrap();
    let service_dir = service.data_dir().join("service");
    kuku::util::private_directory(&service_dir).unwrap();
    fs::File::create(service_dir.join("instance.lock")).unwrap();
    fs::write(service_dir.join("connection.json"), b"stale discovery").unwrap();
    assert!(!service.config_path().exists());

    let live_service = service.status().await.unwrap().is_some();
    assert!(!live_service);
    assert!(should_offer_setup(Ok(live_service), true));
}

#[tokio::test]
async fn existing_config_and_remote_or_token_options_skip_first_run_setup() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("existing.toml");
    fs::write(&config, kuku::config::generate_default()).unwrap();
    let options = ConnectionOptions {
        data_dir: Some(directory.path().join("data")),
        config: Some(config.clone()),
        ..ConnectionOptions::default()
    };
    assert!(super::run_if_needed(&options).await.unwrap());
    assert!(!options
        .data_dir
        .as_ref()
        .unwrap()
        .join("credentials")
        .exists());

    let remote = ConnectionOptions {
        remote: Some("https://service.invalid".into()),
        data_dir: Some(directory.path().join("remote-data")),
        ..ConnectionOptions::default()
    };
    assert!(super::run_if_needed(&remote).await.unwrap());
    assert!(!remote.data_dir.as_ref().unwrap().exists());

    let token = ConnectionOptions {
        token: Some("not-a-real-token".into()),
        data_dir: Some(directory.path().join("token-data")),
        ..ConnectionOptions::default()
    };
    assert!(super::run_if_needed(&token).await.unwrap());
    assert!(!token.data_dir.as_ref().unwrap().exists());
}

#[test]
fn status_errors_do_not_offer_setup() {
    assert!(!should_offer_setup(Err(()), true));
}
