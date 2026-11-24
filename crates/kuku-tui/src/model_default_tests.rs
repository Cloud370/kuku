use std::fs;
use std::sync::{Arc, Barrier};
use std::thread;

use kuku::config::{ModelRef, parse_config_text, parse_config_text_with_env, publish_config};
use kuku_client::ConnectionOptions;
use sha2::{Digest, Sha256};

use super::{ModelDefaultPlan, SAVE_SCOPE, edited_config, prepare, test_plan};
use crate::local_settings::require_local;

const CONFIG: &str = r#"# retained comment
default_model = { connection = "work", model = "gateway/model-2" }

[provider.openai]
format = "openai-responses"
base_url = "https://public.example.test/v1"
api_key = "$MY_GATEWAY_KEY"

[provider.work]
format = "openai-responses"
base_url = "https://work.example.test/v1"
api_key = "$MY_WORK_KEY"

[model.openai."gateway/model-2"]
service_tier = "priority"
max_output_tokens = 222

[model.work."gateway/model-2"]
reasoning_effort = "low"
max_output_tokens = 444
"#;

fn route(model: &str) -> ModelRef {
    ModelRef::new("openai", model)
}

fn edit_with_server_plan(
    raw: &str,
    route: &ModelRef,
    already_default: bool,
    register_previous_default: Option<ModelRef>,
) -> String {
    let plan = ModelDefaultPlan {
        config_sha256: format!("{:x}", Sha256::digest(raw.as_bytes())),
        already_default,
        register_previous_default,
    };
    String::from_utf8(edited_config(raw.as_bytes(), route, &plan).unwrap()).unwrap()
}

#[test]
fn default_changes_between_exact_pairs_without_touching_settings() {
    let selected = route("gateway/model-2");
    let output = edit_with_server_plan(CONFIG, &selected, false, None);
    let before = parse_config_text(CONFIG).unwrap();
    let after = parse_config_text(&output).unwrap();
    assert_eq!(after.default_model, Some(selected));
    assert_eq!(after.model, before.model);
    assert_eq!(after.provider, before.provider);
    assert!(output.contains("# retained comment"));
}

#[test]
fn choosing_the_existing_default_keeps_its_environment_reference() {
    let raw = CONFIG.replacen(
        "model = \"gateway/model-2\"",
        "model = \"$MY_DEFAULT_ID\"",
        1,
    );
    let current = ModelRef::new("work", "gateway/model-2");
    assert_eq!(raw, edit_with_server_plan(&raw, &current, true, None));
}

#[test]
fn changing_an_implicit_default_keeps_its_raw_registration() {
    let raw = r#"# keep server-owned references
default_model = { connection = "$MY_CONNECTION", model = "$MY_DEFAULT_ID" }
provider = { work = { format = "openai-responses", api_key = "$MY_WORK_KEY", base_url = "$MY_WORK_URL" }, openai = { format = "openai-responses", api_key = "$MY_GATEWAY_KEY" } }
model = { openai = { "gateway/model-2" = { max_output_tokens = 200 } } }
"#;
    let selected = route("gateway/model-2");
    let output = edit_with_server_plan(
        raw,
        &selected,
        false,
        Some(ModelRef::new("work", "$MY_DEFAULT_ID")),
    );
    assert!(output.contains("model = {"));
    assert!(output.contains("base_url = \"$MY_WORK_URL\""));
    assert!(output.contains("\"$MY_DEFAULT_ID\""));
    assert!(output.contains("# keep server-owned references"));
    let service_env = |name: &str| match name {
        "MY_DEFAULT_ID" => Ok("upstream/first".into()),
        "MY_WORK_URL" => Ok("https://work.example.test/v1".into()),
        _ => Err(std::env::VarError::NotPresent),
    };
    let parsed = parse_config_text_with_env(&output, service_env).unwrap();
    assert_eq!(parsed.default_model, Some(selected));
    assert!(parsed.model["work"].contains_key("upstream/first"));
}

#[test]
fn preparation_rejects_the_wrong_config_fingerprint() {
    let plan = ModelDefaultPlan {
        config_sha256: "a".repeat(64),
        already_default: false,
        register_previous_default: None,
    };
    assert!(edited_config(CONFIG.as_bytes(), &route("gateway/model-2"), &plan).is_err());
}

#[test]
fn default_save_requires_a_managed_local_connection() {
    assert!(require_local(true, false, true).is_err());
    assert!(require_local(false, true, true).is_err());
    assert!(require_local(false, false, false).is_err());
    assert!(require_local(false, false, true).is_ok());
}

#[tokio::test]
async fn prepare_uses_normalized_managed_endpoint_and_rejects_other_origin_or_path() {
    use std::time::Duration;

    use kuku_client::Client;
    use kuku_server::server_args::ServerArgs;

    use crate::local_settings::same_service_endpoint;

    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let config_path = home.path().join("custom.toml");
    let config = CONFIG
        .replace("service_tier = \"priority\"", "")
        .replace("reasoning_effort = \"low\"", "")
        .replace("$MY_GATEWAY_KEY", "fixture-only-key")
        .replace("$MY_WORK_KEY", "fixture-only-key");
    fs::write(&config_path, &config).unwrap();
    let args = ServerArgs {
        listen: "127.0.0.1:0".into(),
        config: Some(config_path.clone()),
        data_dir: Some(home.path().to_path_buf()),
        password: None,
        max_concurrent_runs: 1,
        managed: true,
    };
    let server = tokio::spawn(async move {
        kuku_server::service::serve(args, None)
            .await
            .map_err(|error| error.to_string())
    });
    let discovery = home.path().join("service").join("connection.json");
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if discovery.exists() {
                break;
            }
            assert!(
                !server.is_finished(),
                "managed service exited before discovery"
            );
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    })
    .await
    .expect("managed service did not publish discovery");

    let options = ConnectionOptions {
        workspace: Some(workspace.path().to_path_buf()),
        data_dir: Some(home.path().to_path_buf()),
        config: Some(config_path.clone()),
        ..ConnectionOptions::default()
    };
    let connection = options.connect().await.unwrap();
    let active = options
        .local_service()
        .unwrap()
        .status()
        .await
        .unwrap()
        .unwrap();
    assert_ne!(connection.client.endpoint(), active.endpoint());
    assert!(same_service_endpoint(
        connection.client.endpoint(),
        active.endpoint()
    ));

    let mut different_origin = url::Url::parse(active.endpoint()).unwrap();
    different_origin.set_host(Some("localhost")).unwrap();
    let different_origin = Client::remote(different_origin.as_str(), active.token()).unwrap();
    assert!(!same_service_endpoint(
        different_origin.endpoint(),
        active.endpoint()
    ));

    let mut different_path = url::Url::parse(active.endpoint()).unwrap();
    different_path.set_path("/other");
    let different_path = Client::remote(different_path.as_str(), active.token()).unwrap();
    assert!(!same_service_endpoint(
        different_path.endpoint(),
        active.endpoint()
    ));

    let config_before_prepare = fs::read(&config_path).unwrap();
    let prepared = prepare(&options, Some(&connection), route("gateway/model-2"))
        .await
        .unwrap();
    assert_eq!(&route("gateway/model-2"), prepared.route());
    let before = parse_config_text(&config).unwrap();
    let after = parse_config_text(std::str::from_utf8(&prepared.updated_bytes).unwrap()).unwrap();
    assert_eq!(before.model, after.model);
    assert_eq!(after.default_model, Some(route("gateway/model-2")));
    assert_eq!(config_before_prepare, fs::read(&config_path).unwrap());
    server.abort();
    let _ = server.await;
}

#[test]
fn confirmation_shows_exact_pair_and_path() {
    let plan = test_plan(route("gateway/model-2"));
    let header = plan.confirmation_header();
    assert!(header.contains("Connection: openai"));
    assert!(header.contains("Model: gateway/model-2"));
    assert!(header.contains(&format!("Config: {}", plan.config_path.display())));
    assert!(!header.contains(SAVE_SCOPE));
}

#[test]
fn external_edit_is_not_overwritten() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    fs::write(&path, CONFIG).unwrap();
    let permissions = fs::metadata(&path).unwrap().permissions();
    let changed = CONFIG.replace("gateway/model-2", "external-change");
    fs::write(&path, &changed).unwrap();
    let result = publish_config(
        &path,
        Some(CONFIG.as_bytes()),
        Some(&permissions),
        b"must-not-publish",
        || {},
    );
    assert!(result.is_err());
    assert_eq!(fs::read(&path).unwrap(), changed.as_bytes());
}

#[test]
fn concurrent_saves_loaded_from_same_bytes_have_one_winner() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    let original = b"default_model = { connection = \"work\", model = \"old\" }\n";
    fs::write(&path, original).unwrap();
    let permissions = fs::metadata(&path).unwrap().permissions();
    let start = Arc::new(Barrier::new(3));
    let writers = [
        b"default_model = { connection = \"work\", model = \"first\" }\n".to_vec(),
        b"default_model = { connection = \"work\", model = \"second\" }\n".to_vec(),
    ]
    .into_iter()
    .map(|updated| {
        let path = path.clone();
        let permissions = permissions.clone();
        let start = Arc::clone(&start);
        thread::spawn(move || {
            start.wait();
            publish_config(&path, Some(original), Some(&permissions), &updated, || {})
        })
    })
    .collect::<Vec<_>>();
    start.wait();
    let results = writers
        .into_iter()
        .map(|writer| writer.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert!(results.iter().any(|result| result.is_err()));
    assert_ne!(fs::read(&path).unwrap(), original);
}
