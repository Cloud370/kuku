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
fn nondefault_messages_model_can_require_an_agent_output_limit() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().canonicalize().unwrap().join("config.toml");
    let raw = format!(
        r#"{FIXTURE_CONFIG}
[provider.native]
format = "anthropic"
base_url = "https://messages.example/v1"
api_key = "fixture-key"

[model.native.fixture]
"#
    );
    fs::write(&path, raw).unwrap();
    let result = report(&config(home.path(), &["validate"]), 0, &path);
    assert_eq!("valid", result["status"]);

    let models = report(&config(home.path(), &["models"]), 0, &path);
    let native = models["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model["connection"] == "native")
        .unwrap();
    assert_eq!("invalid_model_settings", native["error"]["code"]);

    let edit = config(home.path(), &["set", "--patch", "logs.max_age_days = 30"]);
    assert_eq!("updated", report(&edit, 0, &path)["status"]);
    let edited = fs::read(&path).unwrap();
    let invalid_settings = r#"model.native.fixture.reasoning_effort = "unsupported""#;
    let default_without_limit = r#"default_model = { connection = "native", model = "fixture" }"#;
    for patch in [invalid_settings, default_without_limit] {
        let result = report(&config(home.path(), &["set", "--patch", patch]), 1, &path);
        assert_eq!("invalid_model_settings", result["diagnostics"][0]["code"]);
        assert_eq!("model.native.fixture", result["diagnostics"][0]["field"]);
        assert_eq!(edited, fs::read(&path).unwrap());
    }

    let with_limit =
        format!("{default_without_limit}\nmodel.native.fixture.max_output_tokens = 128");
    let result = report(
        &config(home.path(), &["set", "--patch", &with_limit]),
        0,
        &path,
    );
    assert_eq!(
        json!({"connection":"native","model":"fixture"}),
        result["default_model"]
    );
}

#[test]
fn patch_fills_missing_update_defaults_and_preserves_an_existing_source() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().canonicalize().unwrap().join("config.toml");
    fs::write(&path, FIXTURE_CONFIG).unwrap();
    let patch = |text| {
        let output = config(home.path(), &["set", "--patch", text]);
        assert_eq!("updated", report(&output, 0, &path)["status"]);
        kuku::config::load_config(&path)
            .unwrap()
            .resolve()
            .unwrap()
            .update
    };

    let alpha = patch(r#"update.channel = "alpha""#);
    assert_eq!("alpha", alpha.channel);
    assert_eq!("github", alpha.source);

    let mirror = patch(
        r#"
[update]
source = "mirror"
[update.sources]
custom = "https://mirror.example/latest.json"
"#,
    );
    assert_eq!("alpha", mirror.channel);
    let stable = patch(r#"update.channel = "stable""#);
    assert_eq!("stable", stable.channel);
    assert_eq!("mirror", stable.source);
    assert_eq!(
        "https://mirror.example/latest.json",
        stable.sources["custom"]
    );
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

const PROBE_MODEL: &str = "claude-sonnet-4-6";
const PROBE_SECRET: &str = "mock-secret-from-draft";

fn draft_probe(home: &Path, draft: &Path, build: &str, model: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kuku-cli"))
        .env("KUKU_HOME", home)
        .args(["--require-build", build, "--config"])
        .arg(draft)
        .args(["config", "test", "--connection", "native", "--model", model])
        .args(args)
        .output()
        .unwrap()
}

fn probe_config(endpoint: &str) -> String {
    format!(
        r#"{FIXTURE_CONFIG}
[provider.native]
format = "anthropic"
profile = "anthropic"
base_url = "{endpoint}/v1"
api_key = "{PROBE_SECRET}"

[model.native."{PROBE_MODEL}"]
reasoning_effort = "unsupported-session-effort"
service_tier = "standard_only"
"#
    )
}

#[test]
fn draft_text_probe_checks_build_pair_and_final_trial_settings_without_writing() {
    let mock = MockServer::start();
    let request = mock.mock(|when, then| {
        when.method(Method::POST)
            .path("/v1/messages")
            .header("x-api-key", PROBE_SECRET)
            .json_body_partial(
                json!({
                    "model": PROBE_MODEL,
                    "max_tokens": kuku::adapter::TRIAL_OUTPUT_TOKENS,
                    "output_config": {"effort": "medium"},
                    "service_tier": "standard_only"
                })
                .to_string(),
            );
        then.status(200).json_body(json!({
            "content": [{"type":"text","text":"377"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 15, "output_tokens": 1}
        }));
    });
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().canonicalize().unwrap();
    let draft = dir.join("draft config.toml");
    let default_path = dir.join("config.toml");
    let text = probe_config(&mock.base_url());
    fs::write(&default_path, FIXTURE_CONFIG).unwrap();
    fs::write(&draft, &text).unwrap();
    let opts = ["--reasoning-effort", "medium"];
    let build = kuku::BUILD_VERSION;

    let mismatch = draft_probe(&dir, &draft, "fixture-different-build", PROBE_MODEL, &opts);
    assert_eq!(mismatch.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&mismatch.stderr).contains("does not match the running service")
    );
    assert!(mismatch.stdout.is_empty());
    let unknown = draft_probe(&dir, &draft, build, "claude-sonnet-4-6-typo", &opts);
    assert_eq!(
        "unknown_model",
        report(&unknown, 1, &draft)["diagnostics"][0]["code"]
    );
    let bad_effort = draft_probe(&dir, &draft, build, PROBE_MODEL, &[]);
    assert_eq!(
        "invalid_model_settings",
        report(&bad_effort, 1, &draft)["diagnostics"][0]["code"]
    );
    request.assert_hits(0);

    let outcome = draft_probe(&dir, &draft, build, PROBE_MODEL, &opts);
    let result = report(&outcome, 0, &draft);
    assert_eq!("tested", result["status"]);
    assert_eq!("text", result["check"]);
    assert_eq!("native", result["connection"]);
    assert_eq!(PROBE_MODEL, result["model"]);
    assert_eq!(true, result["evidence"]["text_returned"]);
    assert_eq!("stopped", result["service"]["state"]);
    request.assert_hits(1);
    assert!(!String::from_utf8_lossy(&outcome.stdout).contains(PROBE_SECRET));
    assert_eq!(text, fs::read_to_string(&draft).unwrap());
    assert_eq!(FIXTURE_CONFIG, fs::read_to_string(&default_path).unwrap());
    assert!(!dir.join("service").exists());
}

#[test]
fn model_probe_failure_redacts_echoed_credentials_in_stdout_and_stderr() {
    let mock = MockServer::start();
    let request = mock.mock(|when, then| {
        when.method(Method::POST)
            .path("/v1/messages")
            .header("x-api-key", PROBE_SECRET);
        then.status(401).json_body(json!({
            "error": {"type": "authentication_error", "message": format!("bad key {PROBE_SECRET}")}
        }));
    });
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().canonicalize().unwrap();
    let draft = dir.join("draft.toml");
    fs::write(&draft, probe_config(&mock.base_url())).unwrap();
    let outcome = draft_probe(
        &dir,
        &draft,
        kuku::BUILD_VERSION,
        PROBE_MODEL,
        &["--reasoning-effort", "medium"],
    );
    let result = report(&outcome, 1, &draft);
    assert_eq!("model_test_failed", result["diagnostics"][0]["code"]);
    assert_eq!("stopped", result["service"]["state"]);
    request.assert_hits(1);
    for stream in [&outcome.stdout, &outcome.stderr] {
        let stream = String::from_utf8_lossy(stream);
        assert!(stream.contains("<redacted>"), "{stream}");
        assert!(!stream.contains(PROBE_SECRET));
    }
}

#[test]
fn image_and_tool_probes_do_not_inherit_session_reasoning() {
    let mock = MockServer::start();
    let image = mock.mock(|when, then| {
        when.method(Method::POST)
            .path("/v1/messages")
            .body_contains("image/png")
            .json_body_partial(
                json!({
                    "model": PROBE_MODEL,
                    "max_tokens": kuku::adapter::TRIAL_OUTPUT_TOKENS,
                    "thinking": {"type": "disabled"},
                })
                .to_string(),
            );
        then.status(200).json_body(json!({
            "content":[{"type":"text","text":"not a color grid"}],
            "stop_reason": "end_turn"
        }));
    });
    let tool_reply = mock.mock(|when, then| {
        when.method(Method::POST)
            .path("/v1/messages")
            .body_contains("\"tool_result\"")
            .body_contains("\"content\":\"kuku-");
        then.status(200).json_body(json!({
            "content":[{"type":"text","text":"not the supplied result"}],
            "stop_reason": "end_turn"
        }));
    });
    let tool_call = mock.mock(|when, then| {
        when.method(Method::POST)
            .path("/v1/messages")
            .body_contains("\"tools\"")
            .body_contains("kuku_setup_check")
            .matches(|request| {
                let body: Value = serde_json::from_slice(request.body.as_deref().unwrap()).unwrap();
                body["messages"]
                    .as_array()
                    .is_some_and(|messages| messages.len() == 1)
                    && body.get("thinking").is_none()
            });
        then.status(200).json_body(json!({
            "content":[{
                "type":"tool_use","id":"probe-call","name":"kuku_setup_check","input":{}
            }],
            "stop_reason":"tool_use"
        }));
    });
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().canonicalize().unwrap();
    let draft = dir.join("draft.toml");
    let text = probe_config(&mock.base_url());
    fs::write(&draft, &text).unwrap();
    let run = |args: &[&str]| draft_probe(&dir, &draft, kuku::BUILD_VERSION, PROBE_MODEL, args);
    let rejected = run(&["--check", "image", "--reasoning-effort", "medium"]);
    assert_eq!(
        "config_arguments",
        report(&rejected, 1, &draft)["diagnostics"][0]["code"]
    );
    assert_eq!(
        false,
        report(&run(&["--check", "image"]), 0, &draft)["evidence"]["image_match"]
    );
    assert_eq!(
        false,
        report(&run(&["--check", "tools"]), 0, &draft)["evidence"]["tool_round_trip"]
    );
    image.assert_hits(1);
    tool_call.assert_hits(1);
    tool_reply.assert_hits(1);
    assert_eq!(text, fs::read_to_string(&draft).unwrap());
    assert!(!dir.join("service").exists());
}

#[test]
fn removing_a_model_with_an_environment_alias_removes_its_registration() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().canonicalize().unwrap().join("config.toml");
    let raw = FIXTURE_CONFIG.replace("vendor/fixture.v1", "$KUKU_CLI_DEFAULT_ID");
    let raw = format!("{raw}\n[model.work.\"$KUKU_CLI_REGISTERED_ID\"]\ncontext_window = 8192\n");
    fs::write(&path, &raw).unwrap();
    let set = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_kuku-cli"))
            .env("KUKU_HOME", home.path())
            .env("KUKU_CLI_DEFAULT_ID", "vendor/fixture.v1")
            .env("KUKU_CLI_REGISTERED_ID", "vendor/fixture.v1")
            .args(["config", "set"])
            .args(args)
            .output()
            .unwrap()
    };
    let edited = set(&[
        "--patch",
        r#"
[model.work."vendor/fixture.v1"]
context_window = 16384
"#,
    ]);
    assert_eq!("updated", report(&edited, 0, &path)["status"]);
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("context_window = 16384"));
    assert!(text.contains("[model.work.\"$KUKU_CLI_REGISTERED_ID\"]"));
    assert!(!text.contains("[model.work.\"vendor/fixture.v1\"]"));
    let scalar = set(&[r#"model.work."vendor/fixture.v1".context_window"#, "32768"]);
    report(&scalar, 0, &path);
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("context_window = 32768"));
    assert!(text.contains("[model.work.\"$KUKU_CLI_REGISTERED_ID\"]"));
    assert!(!text.contains("[model.work.\"vendor/fixture.v1\"]"));
    for current in ["$KUKU_CLI_REGISTERED_ID", "vendor/fixture.v1"] {
        fs::write(&path, &raw).unwrap();
        let patch = format!(
            r#"delete = ['model.work."{current}"']
default_model = {{ connection = "work", model = "next.model.v2" }}
"#
        );
        let output = set(&["--patch", &patch]);
        assert_eq!("updated", report(&output, 0, &path)["status"]);
        let result = report(&config(home.path(), &["models"]), 0, &path);
        assert_eq!(1, result["models"].as_array().unwrap().len());
        assert_eq!("next.model.v2", result["models"][0]["model"]);
    }
}

#[test]
fn offline_default_change_retains_resolvable_references_and_repairs_missing_ones() {
    const VARIABLE: &str = "KUKU_FIXTURE_OFFLINE_ID";
    let home = tempfile::tempdir().unwrap();
    let path = home.path().canonicalize().unwrap().join("config.toml");
    let patch = r#"default_model = { connection = "work", model = "next" }"#;
    let run = |model: Option<&str>, args: &[&str]| {
        let mut cli = Command::new(env!("CARGO_BIN_EXE_kuku-cli"));
        cli.env("KUKU_HOME", home.path()).env_remove(VARIABLE);
        if let Some(id) = model {
            cli.env(VARIABLE, id);
        }
        cli.arg("config").args(args).output().unwrap()
    };
    let raw = FIXTURE_CONFIG.replace("vendor/fixture.v1", &format!("${VARIABLE}"));
    fs::write(&path, &raw).unwrap();
    report(&run(Some("next"), &["set", "--patch", patch]), 0, &path);
    let models = report(&run(Some("old"), &["models"]), 0, &path);
    assert_eq!(2, models["models"].as_array().unwrap().len());
    assert_eq!("old", models["models"][1]["model"]);

    for invalid in [
        raw,
        FIXTURE_CONFIG.replace(
            "connection = \"work\"",
            &format!("connection = \"${VARIABLE}\""),
        ),
    ] {
        fs::write(&path, invalid).unwrap();
        report(&run(None, &["set", "--patch", patch]), 0, &path);
        assert!(!fs::read_to_string(&path).unwrap().contains(VARIABLE));
        let models = report(&run(None, &["models"]), 0, &path);
        assert_eq!(1, models["models"].as_array().unwrap().len());
        assert_eq!("next", models["default_model"]["model"]);
    }
}

#[test]
fn patch_adopts_an_explicit_model_reference_and_retains_existing_settings() {
    const NEW_KEY: &str = "$KUKU_CLI_DYNAMIC_ID";
    const OLD_KEY: &str = "$KUKU_CLI_OLD_ID";
    const ORIGINAL_MODEL: &str = "vendor/fixture.v1";
    const NEXT_MODEL: &str = "vendor/fixture.v2";

    let home = tempfile::tempdir().unwrap();
    let path = home.path().canonicalize().unwrap().join("config.toml");
    let run = |model: &str, args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_kuku-cli"))
            .env("KUKU_HOME", home.path())
            .env("KUKU_CLI_DYNAMIC_ID", model)
            .env("KUKU_CLI_OLD_ID", ORIGINAL_MODEL)
            .arg("config")
            .args(args)
            .output()
            .unwrap()
    };
    let select_reference = format!(
        r#"
default_model = {{ connection = "work", model = "{NEW_KEY}" }}
[model.work."{NEW_KEY}"]
max_output_tokens = 128
"#
    );
    for old_key in [ORIGINAL_MODEL, OLD_KEY] {
        let raw = format!(
            "{FIXTURE_CONFIG}\n# Model comment\n[model.work.\"{old_key}\"]\ncontext_window = 8192\n"
        );
        fs::write(&path, &raw).unwrap();
        let duplicate =
            format!("{select_reference}\n[model.work.\"{old_key}\"]\ncontext_window = 2048\n");
        let result = run(ORIGINAL_MODEL, &["set", "--patch", &duplicate]);
        assert_eq!("invalid", report(&result, 1, &path)["status"]);
        assert_eq!(raw, fs::read_to_string(&path).unwrap());

        let output = run(ORIGINAL_MODEL, &["set", "--patch", &select_reference]);
        assert_eq!("updated", report(&output, 0, &path)["status"]);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("# Model comment"));
        assert!(text.contains(&format!("[model.work.\"{NEW_KEY}\"]")));
        assert!(!text.contains(&format!("[model.work.\"{old_key}\"]")));
        let result = report(&run(NEXT_MODEL, &["models"]), 0, &path);
        assert_eq!(NEXT_MODEL, result["default_model"]["model"]);
        assert_eq!(1, result["models"].as_array().unwrap().len());
        let selection = &result["models"][0]["selection"];
        assert_eq!(8192, selection["context_window_tokens"]);
        assert_eq!(128, selection["max_output_tokens"]);
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
