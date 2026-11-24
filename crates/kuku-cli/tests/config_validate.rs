use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use httpmock::{Method, MockServer};
use serde_json::{Value, json};

const FIXTURE_CONFIG: &str = r#"
default_model = { connection = "work", model = "vendor/fixture.v1" }

# User comment must survive CLI edits.
[provider.work]
format = "openai-responses"
base_url = "https://gateway.example/v1"
api_key = "$KUKU_CLI_FIXTURE_KEY"
profile = "compatible"
reasoning_efforts = ["none"]
"#;

fn config(home: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kuku-cli"))
        .env("KUKU_HOME", home)
        .env_remove("KUKU_CLI_FIXTURE_KEY")
        .arg("config")
        .args(arguments)
        .output()
        .unwrap()
}

#[track_caller]
fn report(output: &Output, code: i32, path: &Path) -> Value {
    assert_eq!(
        Some(code),
        output.status.code(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(path.to_string_lossy().as_ref(), result["path"]);
    result
}

#[test]
fn validates_configured_models_without_checking_credentials_or_discovery() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().canonicalize().unwrap().join("config.toml");
    fs::write(&path, FIXTURE_CONFIG).unwrap();
    let result = report(&config(home.path(), &["validate"]), 0, &path);
    assert_eq!("valid", result["status"]);
    assert_eq!("stopped", result["service"]["state"]);
    assert_eq!(
        json!({"connection":"work","model":"vendor/fixture.v1"}),
        result["default_model"]
    );

    let invalid = format!(
        "{FIXTURE_CONFIG}\n[model.work.\"vendor/fixture.v1\"]\nreasoning_effort = \"high\"\n"
    );
    fs::write(&path, &invalid).unwrap();
    let result = report(&config(home.path(), &["validate"]), 1, &path);
    assert_eq!("invalid", result["status"]);
    assert_eq!(
        "model.work.\"vendor/fixture.v1\"",
        result["diagnostics"][0]["field"]
    );
    assert!(
        result["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains("unsupported reasoning_effort 'high'"),
        "{result}"
    );

    fs::write(&path, "default_model = \"balanced\"\n").unwrap();
    let result = report(&config(home.path(), &["validate"]), 1, &path);
    assert_eq!("default_model", result["diagnostics"][0]["field"]);
}

#[test]
fn patch_creates_config_deletes_and_replaces_default_pair_and_preserves_file_on_failure() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().canonicalize().unwrap().join("config.toml");
    assert!(!path.exists());
    let output = config(home.path(), &["set", "--patch", FIXTURE_CONFIG]);
    assert_eq!("updated", report(&output, 0, &path)["status"]);

    let bytes = fs::read(&path).unwrap();
    let invalid = r#"default_model = { connection = "unknown", model = "fixture" }"#;
    let result = report(&config(home.path(), &["set", "--patch", invalid]), 1, &path);
    assert_eq!(
        "default_model.connection",
        result["diagnostics"][0]["field"]
    );
    assert_eq!(bytes, fs::read(&path).unwrap());

    let invalid = r#"delete = ['model.work."vendor/fixture.v1"']"#;
    let result = report(&config(home.path(), &["set", "--patch", invalid]), 1, &path);
    assert_eq!("default_model.model", result["diagnostics"][0]["field"]);
    assert_eq!(bytes, fs::read(&path).unwrap());

    let patch = r#"
delete = ['model.work."vendor/fixture.v1"']
default_model = { connection = "work", model = "vendor/fixture.v2" }

[model.work."vendor/fixture.v2"]
context_window = 8192
"#;
    let result = report(&config(home.path(), &["set", "--patch", patch]), 0, &path);
    assert_eq!("vendor/fixture.v2", result["default_model"]["model"]);
    let models = report(&config(home.path(), &["models"]), 0, &path);
    assert_eq!(1, models["models"].as_array().unwrap().len());
    assert_eq!(
        8192,
        models["models"][0]["selection"]["context_window_tokens"]
    );
    assert!(
        fs::read_to_string(&path)
            .unwrap()
            .contains("# User comment")
    );

    let changed = r#"default_model = { connection = "work", model = "vendor/fixture.v3" }"#;
    let result = report(&config(home.path(), &["set", "--patch", changed]), 0, &path);
    assert_eq!("vendor/fixture.v3", result["default_model"]["model"]);
    let models = report(&config(home.path(), &["models"]), 0, &path);
    assert_eq!(2, models["models"].as_array().unwrap().len());
    assert_eq!("vendor/fixture.v2", models["models"][0]["model"]);
    assert_eq!("vendor/fixture.v3", models["models"][1]["model"]);

    let move_to_other = r#"
delete = ["provider.work", "model.work"]
default_model = { connection = "other", model = "new.exact-id" }

[provider.other]
format = "openai-responses"
base_url = "https://second-gateway.example/v1"
api_key = "secret-fixture-value"
"#;
    let result = report(
        &config(home.path(), &["set", "--patch", move_to_other]),
        0,
        &path,
    );
    assert_eq!("other", result["default_model"]["connection"]);
    assert!(
        !fs::read_to_string(&path)
            .unwrap()
            .contains("[provider.work]")
    );
    let output = config(home.path(), &["show", "--json"]);
    let result = report(&output, 0, &path);
    assert!(result["config"].as_str().unwrap().contains("<redacted>"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("secret-fixture-value"));
}

#[test]
fn patch_reports_a_conflict_while_another_writer_holds_the_lock() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().canonicalize().unwrap().join("config.toml");
    fs::write(&path, FIXTURE_CONFIG).unwrap();
    let changed = FIXTURE_CONFIG.replace("fixture.v1", "fixture.before-conflict");
    kuku::config::publish_config(
        &path,
        Some(FIXTURE_CONFIG.as_bytes()),
        None,
        changed.as_bytes(),
        || {
            let output = config(
                home.path(),
                &[
                    "set",
                    "--patch",
                    r#"default_model = { connection = "work", model = "fixture.losing-writer" }"#,
                ],
            );
            let result = report(&output, 1, &path);
            assert_eq!("conflict", result["status"]);
            assert_eq!("config_conflict", result["diagnostics"][0]["code"]);
        },
    )
    .unwrap();
    assert_eq!(changed, fs::read_to_string(path).unwrap());
}

#[test]
fn discovers_exact_model_ids_from_only_the_requested_connection() {
    let mock = MockServer::start();
    let catalog = mock.mock(|when, then| {
        when.method(Method::GET)
            .path("/v1/models")
            .header("authorization", "Bearer fixture-key");
        then.status(200).json_body(
            json!({"data":[{"id":"vendor/fixture.v1"},{"id":"vendor/model.2:build-3"}]}),
        );
    });
    let home = tempfile::tempdir().unwrap();
    let path = home.path().canonicalize().unwrap().join("config.toml");
    fs::write(
        &path,
        FIXTURE_CONFIG.replace(
            "https://gateway.example/v1",
            &format!("{}/v1", mock.base_url()),
        ),
    )
    .unwrap();

    let configured = report(&config(home.path(), &["models"]), 0, &path);
    catalog.assert_hits(0);
    assert_eq!(1, configured["models"].as_array().unwrap().len());
    assert_eq!("vendor/fixture.v1", configured["models"][0]["model"]);

    let output = Command::new(env!("CARGO_BIN_EXE_kuku-cli"))
        .env("KUKU_HOME", home.path())
        .env("KUKU_CLI_FIXTURE_KEY", "fixture-key")
        .args(["config", "models", "--discover", "--connection", "work"])
        .output()
        .unwrap();
    let discovered = report(&output, 0, &path);
    assert_eq!("work", discovered["connection"]);
    let ids = discovered["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|model| model["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(ids.contains(&"vendor/model.2:build-3"), "{discovered}");
    assert!(ids.contains(&"vendor/fixture.v1"), "{discovered}");
    catalog.assert_hits(1);
}

#[test]
fn removing_a_model_with_an_environment_alias_removes_its_registration() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().canonicalize().unwrap().join("config.toml");
    let raw = FIXTURE_CONFIG.replace("vendor/fixture.v1", "$KUKU_CLI_DEFAULT_ID");
    let raw = format!("{raw}\n[model.work.\"$KUKU_CLI_REGISTERED_ID\"]\ncontext_window = 8192\n");
    fs::write(&path, &raw).unwrap();
    let patch_config = |patch: &str| {
        Command::new(env!("CARGO_BIN_EXE_kuku-cli"))
            .env("KUKU_HOME", home.path())
            .env("KUKU_CLI_DEFAULT_ID", "vendor/fixture.v1")
            .env("KUKU_CLI_REGISTERED_ID", "vendor/fixture.v1")
            .args(["config", "set", "--patch", patch])
            .output()
            .unwrap()
    };
    let edited = patch_config(
        r#"
[model.work."vendor/fixture.v1"]
context_window = 16384
"#,
    );
    assert_eq!("updated", report(&edited, 0, &path)["status"]);
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("context_window = 16384"));
    assert!(text.contains("[model.work.\"$KUKU_CLI_REGISTERED_ID\"]"));
    assert!(!text.contains("[model.work.\"vendor/fixture.v1\"]"));
    for current in ["$KUKU_CLI_REGISTERED_ID", "vendor/fixture.v1"] {
        fs::write(&path, &raw).unwrap();
        let patch = format!(
            r#"delete = ['model.work."{current}"']
default_model = {{ connection = "work", model = "next.model.v2" }}
"#
        );
        let output = patch_config(&patch);
        assert_eq!("updated", report(&output, 0, &path)["status"]);
        let result = report(&config(home.path(), &["models"]), 0, &path);
        assert_eq!(1, result["models"].as_array().unwrap().len());
        assert_eq!("next.model.v2", result["models"][0]["model"]);
    }
}

#[test]
fn config_uses_the_resolved_data_dir_for_kuku_home_references() {
    let parent = tempfile::tempdir().unwrap();
    let home = parent.path().canonicalize().unwrap().join("selected");
    let path = home.join("config.toml");
    let patch = format!("{FIXTURE_CONFIG}\n[discovery]\nextra_user_paths = [\"$KUKU_HOME\"]\n",);
    let output = Command::new(env!("CARGO_BIN_EXE_kuku-cli"))
        .env_remove("KUKU_HOME")
        .arg("--data-dir")
        .arg(&home)
        .args(["config", "set", "--patch", &patch])
        .output()
        .unwrap();
    assert_eq!("updated", report(&output, 0, &path)["status"]);
    assert!(fs::read_to_string(path).unwrap().contains("$KUKU_HOME"));
}
