use std::fs;
use std::sync::{Arc, Barrier};
use std::thread;

use kuku_client::ConnectionOptions;
use sha2::Digest;
use toml_edit::{DocumentMut, Item};

use super::{
    edited_config, prepare, test_plan, update_user_default, ModelDefaultPlan, ResolvedModel,
    SAVE_SCOPE,
};

use crate::local_settings::{
    active_config_path, ensure_same_service, require_local, same_service, ServiceIdentity,
    LOCAL_SERVICE_CHANGED,
};

const CONFIG: &str = r#"# retained comment
default_model = "light"

[provider.openai]
format = "openai-responses"
base_url = "https://api.openai.com/v1"
api_key = "$OPENAI_API_KEY"

[provider.openai.options]
profile = "openai"
reasoning_efforts = ["low", "medium"]

[model.strong]
provider = "openai"
model = "strong-model"
purpose = "keep strong"

[model.balanced]
provider = "openai"
model = "balanced-model"

[model.light]
provider = "openai"
model = "light-model"
"#;

fn route(model: &str) -> ResolvedModel {
    ResolvedModel {
        connection: "openai".into(),
        model: model.into(),
    }
}

fn edit_with_server_plan(
    raw: &str,
    route: &ResolvedModel,
    tier: &str,
    overwrite_user_default: bool,
) -> Result<(String, Vec<u8>), String> {
    let plan = ModelDefaultPlan {
        tier: tier.into(),
        overwrite_user_default,
        config_sha256: format!("{:x}", sha2::Sha256::digest(raw.as_bytes())),
    };
    edited_config(raw.as_bytes(), route, &plan)
}

#[test]
fn matching_tier_is_preferred_and_repeated_route_saves_are_idempotent() {
    let (tier, output) =
        edit_with_server_plan(CONFIG, &route("light-model"), "light", false).unwrap();
    assert_eq!(tier, "light");
    assert_eq!(output, CONFIG.as_bytes());
    let (tier, repeated) =
        edit_with_server_plan(CONFIG, &route("light-model"), "light", false).unwrap();
    assert_eq!(tier, "light");
    assert_eq!(repeated, output);

    let (tier, output) =
        edit_with_server_plan(CONFIG, &route("balanced-model"), "balanced", false).unwrap();
    let output = String::from_utf8(output).unwrap();
    assert_eq!(tier, "balanced");
    assert!(output.contains("default_model = \"balanced\""));
    assert!(output.contains("purpose = \"keep strong\""));
    assert!(!output.contains("user-default"));
}

#[test]
fn fallback_creates_or_overwrites_only_user_default() {
    let selected = route("custom-model");
    let (tier, output) = edit_with_server_plan(CONFIG, &selected, "user-default", true).unwrap();
    let output = String::from_utf8(output).unwrap();
    assert_eq!(tier, "user-default");
    assert!(output.contains("default_model = \"user-default\""));
    assert!(output.contains("purpose = \"Saved by /model\""));
    assert!(output.contains("model = \"strong-model\""));
    assert!(output.contains("model = \"light-model\""));

    let (tier, repeated) =
        edit_with_server_plan(&output, &selected, "user-default", false).unwrap();
    let repeated = String::from_utf8(repeated).unwrap();
    assert_eq!(tier, "user-default");
    assert_eq!(repeated.matches("[model.user-default]").count(), 1);
    assert!(repeated.contains("model = \"custom-model\""));

    let old = format!(
        "{}\n[model.user-default]\nprovider = \"openai\"\nmodel = \"old-user-model\"\nreasoning_effort = \"high\"\nservice_tier = \"priority\"\nmax_output_tokens = 4096\n",
        CONFIG.replace("default_model = \"light\"", "default_model = \"user-default\"")
    );
    let (_, overwritten) = edit_with_server_plan(&old, &selected, "user-default", true).unwrap();
    let overwritten = String::from_utf8(overwritten).unwrap();
    assert!(overwritten.contains("model = \"custom-model\""));
    assert!(overwritten.contains("model = \"strong-model\""));
    assert!(overwritten.contains("model = \"light-model\""));
    let fallback = overwritten.split("[model.user-default]").last().unwrap();
    assert!(!fallback.contains("reasoning_effort"));
    assert!(!fallback.contains("service_tier"));
    assert!(!fallback.contains("max_output_tokens = 4096"));
}

#[test]
fn replacing_a_different_provider_default_discards_old_route_options() {
    let mut document = r#"
default_model = "user-default"

[model.user-default]
provider = "anthropic"
model = "old-model"
purpose = "old purpose"
reasoning_effort = "high"
service_tier = "priority"
max_output_tokens = 4096

[model.strong]
provider = "openai"
model = "strong-model"
max_output_tokens = 2048
"#
    .parse::<DocumentMut>()
    .unwrap();
    update_user_default(&mut document, &route("new-model")).unwrap();
    let output = document.to_string();
    let fallback = document["model"]["user-default"].as_table_like().unwrap();
    assert_eq!(
        Some("openai"),
        fallback.get("provider").and_then(Item::as_str)
    );
    assert_eq!(
        Some("new-model"),
        fallback.get("model").and_then(Item::as_str)
    );
    assert_eq!(
        Some("Saved by /model"),
        fallback.get("purpose").and_then(Item::as_str)
    );
    assert!(fallback.get("reasoning_effort").is_none());
    assert!(fallback.get("service_tier").is_none());
    assert!(fallback.get("max_output_tokens").is_none());
    assert!(output.contains("max_output_tokens = 2048"));
    assert!(output.contains("model = \"strong-model\""));
}

#[test]
fn matched_tier_retains_its_existing_route_settings() {
    let input = CONFIG.replace(
        "model = \"balanced-model\"",
        "model = \"balanced-model\"\nmax_output_tokens = 888",
    );
    let (tier, output) =
        edit_with_server_plan(&input, &route("balanced-model"), "balanced", false).unwrap();
    assert_eq!("balanced", tier);
    assert!(String::from_utf8(output)
        .unwrap()
        .contains("max_output_tokens = 888"));
}

#[test]
fn inline_tables_environment_references_comments_and_provider_options_survive() {
    const TIER: &str = "KUKU_ISSUE100_DEFAULT_TIER";
    const FORMAT: &str = "KUKU_ISSUE100_PROVIDER_FORMAT";
    let old_tier = std::env::var_os(TIER);
    let old_format = std::env::var_os(FORMAT);
    std::env::set_var(TIER, "balanced");
    std::env::set_var(FORMAT, "openai-responses");
    let input = r#"# keep this note
default_model = "$KUKU_ISSUE100_DEFAULT_TIER"
provider = { openai = { format = "$KUKU_ISSUE100_PROVIDER_FORMAT", base_url = "https://api.openai.com/v1", api_key = "$OPENAI_API_KEY", options = { profile = "openai", reasoning_efforts = ["low", "medium"] } } }
model = { strong = { provider = "openai", model = "strong-model" }, balanced = { provider = " openai ", model = "balanced-model" }, light = { provider = "openai", model = "light-model" } }
"#;
    let (tier, output) =
        edit_with_server_plan(input, &route("balanced-model"), "balanced", false).unwrap();
    if let Some(value) = old_tier {
        std::env::set_var(TIER, value);
    } else {
        std::env::remove_var(TIER);
    }
    if let Some(value) = old_format {
        std::env::set_var(FORMAT, value);
    } else {
        std::env::remove_var(FORMAT);
    }
    let output = String::from_utf8(output).unwrap();
    assert_eq!(tier, "balanced");
    assert!(output.contains("# keep this note"));
    assert!(output.contains("$KUKU_ISSUE100_PROVIDER_FORMAT"));
    assert!(output.contains("reasoning_efforts = [\"low\", \"medium\"]"));
    assert!(output.contains("provider = \" openai \""));
    assert!(output.contains("default_model = \"balanced\""));
    assert!(!output.contains("user-default"));
}

#[test]
fn client_environment_does_not_override_the_service_validated_tier_or_expand_raw_refs() {
    const TIER: &str = "KUKU_ISSUE100_CLIENT_DEFAULT_TIER";
    const PROVIDER: &str = "KUKU_ISSUE100_CLIENT_PROVIDER";
    const MODEL: &str = "KUKU_ISSUE100_CLIENT_MODEL";
    let old = [TIER, PROVIDER, MODEL].map(std::env::var_os);
    std::env::set_var(TIER, "client-tier");
    std::env::set_var(PROVIDER, "client-provider");
    std::env::set_var(MODEL, "client-model");
    let input = format!(
        "# preserve refs\ndefault_model = \"${TIER}\"\n[model.balanced]\nprovider = \"${PROVIDER}\"\nmodel = \"${MODEL}\"\n"
    );
    let edited = edit_with_server_plan(&input, &route("service-model"), "balanced", false);
    for (name, value) in [TIER, PROVIDER, MODEL].into_iter().zip(old) {
        if let Some(value) = value {
            std::env::set_var(name, value);
        } else {
            std::env::remove_var(name);
        }
    }
    let output = String::from_utf8(edited.unwrap().1).unwrap();
    assert!(output.contains("default_model = \"balanced\""));
    assert!(output.contains("provider = \"$KUKU_ISSUE100_CLIENT_PROVIDER\""));
    assert!(output.contains("model = \"$KUKU_ISSUE100_CLIENT_MODEL\""));
    assert!(output.contains("# preserve refs"));
}

#[test]
fn active_config_path_local_gate_and_service_change_are_checked() {
    let temp = tempfile::tempdir().unwrap();
    let active_path = temp
        .path()
        .join("service")
        .join("custom")
        .join("config.toml");
    let requested_path = temp
        .path()
        .join("service")
        .join("default")
        .join("config.toml");
    let active = active_path.to_str().unwrap();
    assert_eq!(
        active_config_path(false, &requested_path, Some(active)).unwrap(),
        active_path
    );
    assert!(active_config_path(true, &requested_path, Some(active)).is_err());
    assert!(active_config_path(false, &requested_path, Some("relative.toml")).is_err());
    assert!(require_local(true, false, true).is_err());
    assert!(require_local(false, true, true).is_err());
    assert!(require_local(false, false, false).is_err());
    assert!(require_local(false, false, true).is_ok());

    let expected = ServiceIdentity {
        endpoint: "local".into(),
        instance_id: "one".into(),
        pid: 2,
        data_dir: temp.path().join("data").display().to_string(),
        config_path: temp.path().join("config.toml"),
    };
    let mut changed = expected.clone();
    changed.instance_id = "two".into();
    assert!(!same_service(&expected, &changed));
    changed = expected.clone();
    changed.config_path = temp.path().join("other.toml");
    assert!(!same_service(&expected, &changed));
    assert_eq!(
        Err(LOCAL_SERVICE_CHANGED.to_owned()),
        ensure_same_service(&expected, &changed)
    );
    assert!(same_service(&expected, &expected));
    assert!(ensure_same_service(&expected, &expected).is_ok());
}

#[tokio::test]
async fn prepare_uses_normalized_managed_endpoint_and_rejects_other_origin_or_path() {
    use std::time::Duration;

    use kuku_client::Client;
    use kuku_server::server_args::ServerArgs;

    use crate::local_settings::same_service_endpoint;

    const CONFIG: &str = r#"
default_model = "balanced"

[provider.openai]
format = "openai-responses"
base_url = "https://api.openai.com/v1"
api_key = "test-key"
profile = "openai"

[model.strong]
provider = "openai"
model = "strong-model"
think = "off"

[model.balanced]
provider = "openai"
model = "balanced-model"
think = "off"

[model.light]
provider = "openai"
model = "light-model"
think = "off"
"#;

    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let config_path = home.path().join("custom.toml");
    fs::write(&config_path, CONFIG).unwrap();
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
    let prepared = prepare(&options, Some(&connection), route("balanced-model"))
        .await
        .unwrap();
    assert_eq!("balanced", prepared.tier);
    assert_eq!(config_before_prepare, fs::read(&config_path).unwrap());
    server.abort();
    let _ = server.await;
}

#[test]
fn confirmation_shows_route_path_tier_and_scope_sentence() {
    let plan = test_plan(route("balanced-model"));
    let header = plan.confirmation_header();
    assert!(header.contains("Connection/model: openai/balanced-model"));
    assert!(header.contains(&format!("Config: {}", plan.config_path.display())));
    assert!(header.contains("Default tier: balanced"));
    assert!(!header.contains(SAVE_SCOPE));
}

#[test]
fn external_edit_is_not_overwritten() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    fs::write(&path, CONFIG).unwrap();
    let permissions = fs::metadata(&path).unwrap().permissions();
    let changed = CONFIG.replace("light-model", "external-change");
    fs::write(&path, &changed).unwrap();
    let result = crate::config_file::publish_config(
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
    let original = b"default_model = \"balanced\"\n";
    fs::write(&path, original).unwrap();
    let permissions = fs::metadata(&path).unwrap().permissions();
    let start = Arc::new(Barrier::new(3));
    let writers = [
        b"default_model = \"strong\"\n".to_vec(),
        b"default_model = \"light\"\n".to_vec(),
    ]
    .into_iter()
    .map(|updated| {
        let path = path.clone();
        let permissions = permissions.clone();
        let start = Arc::clone(&start);
        thread::spawn(move || {
            start.wait();
            crate::config_file::publish_config(
                &path,
                Some(original),
                Some(&permissions),
                &updated,
                || {},
            )
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
