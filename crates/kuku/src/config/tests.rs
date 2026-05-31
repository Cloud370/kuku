use super::*;
use std::fs;

fn temp_config(content: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("config.toml"), content).unwrap();
    dir
}

const FULL_CONFIG: &str = r#"# kuku config
default_model = "balanced"

[model.strong]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "high"
context_window = 200000
max_output_tokens = 64000
purpose = "deep reasoning"

[model.balanced]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "medium"
context_window = 200000
max_output_tokens = 48000
purpose = "general purpose"

[model.light]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"
think = "off"
context_window = 200000
max_output_tokens = 32000
purpose = "quick tasks"

[provider.anthropic]
format = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "test-key-anthropic"

[handoff]
enabled = true
threshold = 0.7
keep_turns = 2

[plugin]
enabled = true

[update]
source = "github"
channel = "stable"
"#;

// ── set_value tests ──

#[test]
fn set_value_updates_string_field() {
    let dir = temp_config(FULL_CONFIG);
    let path = dir.path().join("config.toml");

    set_value(&path, "default_model", "strong").unwrap();

    let content = fs::read_to_string(&path).unwrap();
    assert!(content.contains(r#"default_model = "strong""#));
    assert!(!content.contains(r#"default_model = "balanced""#));
}

#[test]
fn set_value_updates_nested_string_field() {
    let dir = temp_config(FULL_CONFIG);
    let path = dir.path().join("config.toml");

    set_value(&path, "model.balanced.think", "high").unwrap();

    let content = fs::read_to_string(&path).unwrap();
    assert!(content.contains("think = \"high\""));
}

#[test]
fn set_value_updates_integer_field() {
    let dir = temp_config(FULL_CONFIG);
    let path = dir.path().join("config.toml");

    set_value(&path, "model.balanced.context_window", "128000").unwrap();

    let content = fs::read_to_string(&path).unwrap();
    assert!(content.contains("context_window = 128000"));
}

#[test]
fn set_value_preserves_comments() {
    let dir = temp_config(FULL_CONFIG);
    let path = dir.path().join("config.toml");

    set_value(&path, "default_model", "light").unwrap();

    let content = fs::read_to_string(&path).unwrap();
    assert!(content.contains("# kuku config"));
}

#[test]
fn set_value_rejects_invalid_think_level() {
    let dir = temp_config(FULL_CONFIG);
    let path = dir.path().join("config.toml");

    let error = set_value(&path, "model.balanced.think", "invalid").unwrap_err();
    assert!(error.to_string().contains("ThinkLevel"));
    assert!(error.to_string().contains("off/low/medium/high"));
}

#[test]
fn set_value_rejects_invalid_format() {
    let dir = temp_config(FULL_CONFIG);
    let path = dir.path().join("config.toml");

    let error = set_value(&path, "provider.anthropic.format", "invalid").unwrap_err();
    assert!(error.to_string().contains("format"));
}

#[test]
fn set_value_rejects_zero_context_window() {
    let dir = temp_config(FULL_CONFIG);
    let path = dir.path().join("config.toml");

    let error = set_value(&path, "model.balanced.context_window", "0").unwrap_err();
    assert!(error.to_string().contains("context_window"));
}

#[test]
fn set_value_rejects_non_numeric_for_integer_field() {
    let dir = temp_config(FULL_CONFIG);
    let path = dir.path().join("config.toml");

    let error = set_value(&path, "model.balanced.context_window", "abc").unwrap_err();
    assert!(error.to_string().contains("positive integer"));
    assert!(error.to_string().contains("abc"));
}

#[test]
fn set_value_rejects_negative_integer() {
    let dir = temp_config(FULL_CONFIG);
    let path = dir.path().join("config.toml");

    let error = set_value(&path, "model.balanced.context_window", "-1").unwrap_err();
    assert!(error.to_string().contains("positive integer"));
}

#[test]
fn set_value_rejects_zero_max_output_tokens() {
    let dir = temp_config(FULL_CONFIG);
    let path = dir.path().join("config.toml");

    let error = set_value(&path, "model.balanced.max_output_tokens", "0").unwrap_err();
    assert!(error.to_string().contains("positive integer"));
}

#[test]
fn set_value_rejects_missing_subsection() {
    let dir = temp_config(FULL_CONFIG);
    let path = dir.path().join("config.toml");

    let error = set_value(&path, "model.nonexistent.think", "high").unwrap_err();
    assert!(error.to_string().contains("unknown config key"));
}

#[test]
fn set_value_rejects_invalid_provider_reference() {
    let dir = temp_config(FULL_CONFIG);
    let path = dir.path().join("config.toml");

    let error = set_value(&path, "model.balanced.provider", "nonexistent").unwrap_err();
    assert!(error.to_string().contains("provider"));
}

#[test]
fn set_value_rejects_unknown_dot_path() {
    let dir = temp_config(FULL_CONFIG);
    let path = dir.path().join("config.toml");

    let error = set_value(&path, "unknown.field", "value").unwrap_err();
    assert!(error.to_string().contains("unknown config key"));
}

// ── show_redacted tests ──

#[test]
fn show_redacted_masks_plaintext_api_key() {
    let config = r#"
default_model = "balanced"

[model.strong]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "high"
context_window = 200000
max_output_tokens = 64000
purpose = "deep reasoning"

[model.balanced]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "medium"
context_window = 200000
max_output_tokens = 48000
purpose = "general purpose"

[model.light]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"
think = "off"
context_window = 200000
max_output_tokens = 32000
purpose = "quick tasks"

[provider.anthropic]
format = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "sk-ant-secret123"
"#;
    let dir = temp_config(config);
    let path = dir.path().join("config.toml");

    let output = show_redacted(&path).unwrap();
    assert!(output.contains("<redacted>"));
    assert!(!output.contains("sk-ant-secret123"));
}

#[test]
fn show_redacted_preserves_env_var_reference() {
    let _guard = crate::env_lock().lock().unwrap();
    std::env::set_var("_KUKU_TEST_SHOW_KEY", "test-value");
    let config = r#"
default_model = "balanced"

[model.strong]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "high"
context_window = 200000
max_output_tokens = 64000
purpose = "deep reasoning"

[model.balanced]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "medium"
context_window = 200000
max_output_tokens = 48000
purpose = "general purpose"

[model.light]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"
think = "off"
context_window = 200000
max_output_tokens = 32000
purpose = "quick tasks"

[provider.anthropic]
format = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "$_KUKU_TEST_SHOW_KEY"
"#;
    let dir = temp_config(config);
    let path = dir.path().join("config.toml");

    let output = show_redacted(&path).unwrap();
    assert!(output.contains("$_KUKU_TEST_SHOW_KEY"));
    std::env::remove_var("_KUKU_TEST_SHOW_KEY");
}

#[test]
fn show_redacted_errors_on_missing_file() {
    let error = show_redacted(std::path::Path::new("/nonexistent/config.toml")).unwrap_err();
    assert!(error.to_string().contains("required tier"));
}

#[test]
fn show_redacted_errors_on_invalid_config() {
    let dir = temp_config("not valid toml [[[");
    let path = dir.path().join("config.toml");

    let error = show_redacted(&path).unwrap_err();
    assert!(error.to_string().contains("invalid config"));
}

// ── discovery_config tests ──

#[test]
fn discovery_config_from_toml() {
    let toml = r#"
[discovery]
auto_discover = false
extra_user_paths = ["/opt/skills"]
extra_project_paths = [".custom/agents"]
"#;
    let file: ConfigFile = toml::from_str(toml).unwrap();
    let disc = file.discovery.unwrap();
    assert!(!disc.auto_discover);
    assert_eq!(disc.extra_user_paths.len(), 1);
    assert_eq!(disc.extra_project_paths.len(), 1);
}

#[test]
fn discovery_config_defaults_when_absent() {
    let toml = r#"
default_model = "balanced"

[model.strong]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "high"

[model.balanced]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "medium"

[model.light]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"
think = "off"

[provider.anthropic]
format = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "test-key"
"#;
    let file: ConfigFile = toml::from_str(toml).unwrap();
    let disc = file.discovery.unwrap_or_default();
    assert!(disc.auto_discover);
    assert!(disc.extra_user_paths.is_empty());
}

#[test]
fn discovery_config_propagated_to_resolved_config() {
    let toml = r#"
default_model = "balanced"

[model.strong]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "high"

[model.balanced]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "medium"

[model.light]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"
think = "off"

[provider.anthropic]
format = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "test-key"

[discovery]
auto_discover = false
"#;
    let file: ConfigFile = toml::from_str(toml).unwrap();
    let config = file.resolve().unwrap();
    assert!(!config.discovery.auto_discover);
}

// ── handoff_config tests ──

const HANDOFF_VALID_CONFIG: &str = r#"
default_model = "balanced"

[model.strong]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "high"

[model.balanced]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "medium"

[model.light]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"
think = "off"

[provider.anthropic]
format = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "test-key"
"#;

#[test]
fn handoff_config_round_trip() {
    let toml_str = format!(
        "{HANDOFF_VALID_CONFIG}\n[handoff]\nenabled = false\nthreshold = 0.5\nkeep_turns = 3\n"
    );
    let file: ConfigFile = toml::from_str(&toml_str).unwrap();
    let h = file.handoff.as_ref().unwrap();
    assert!(!h.enabled);
    assert!((h.threshold - 0.5).abs() < f64::EPSILON);
    assert_eq!(h.keep_turns, 3);

    let serialized = toml::to_string(&file).unwrap();
    let file2: ConfigFile = toml::from_str(&serialized).unwrap();
    assert_eq!(file.handoff, file2.handoff);
}

#[test]
fn handoff_config_defaults_when_absent() {
    let file: ConfigFile = toml::from_str(HANDOFF_VALID_CONFIG).unwrap();
    assert!(file.handoff.is_none());
    let config = file.resolve().unwrap();
    assert!(config.handoff.enabled);
    assert!((config.handoff.threshold - 0.7).abs() < f64::EPSILON);
    assert_eq!(config.handoff.keep_turns, 2);
}

#[test]
fn handoff_config_partial() {
    let toml_str = format!("{HANDOFF_VALID_CONFIG}\n[handoff]\nenabled = false\n");
    let file: ConfigFile = toml::from_str(&toml_str).unwrap();
    let config = file.resolve().unwrap();
    assert!(!config.handoff.enabled);
    assert!((config.handoff.threshold - 0.7).abs() < f64::EPSILON);
    assert_eq!(config.handoff.keep_turns, 2);
}

#[test]
fn handoff_config_invalid_threshold() {
    let toml_str = format!("{HANDOFF_VALID_CONFIG}\n[handoff]\nthreshold = 1.5\n");
    let file: ConfigFile = toml::from_str(&toml_str).unwrap();
    let err = file.resolve().unwrap_err();
    assert!(err.to_string().contains("threshold"));
}

#[test]
fn handoff_config_zero_keep_turns() {
    let toml_str = format!("{HANDOFF_VALID_CONFIG}\n[handoff]\nkeep_turns = 0\n");
    let file: ConfigFile = toml::from_str(&toml_str).unwrap();
    let err = file.resolve().unwrap_err();
    assert!(err.to_string().contains("keep_turns"));
}

#[test]
fn handoff_propagation() {
    let toml_str = format!(
        "{HANDOFF_VALID_CONFIG}\n[handoff]\nenabled = false\nthreshold = 0.5\nkeep_turns = 3\n"
    );
    let file: ConfigFile = toml::from_str(&toml_str).unwrap();
    let config = file.resolve().unwrap();
    assert!(!config.handoff.enabled);
    assert!((config.handoff.threshold - 0.5).abs() < f64::EPSILON);
    assert_eq!(config.handoff.keep_turns, 3);
}

#[test]
fn generate_default_includes_all_sections() {
    let toml_str = generate_default();
    let file: ConfigFile = toml::from_str(toml_str).unwrap();
    assert!(!file.model.is_empty());
    assert!(!file.provider.is_empty());
    assert!(file.discovery.is_some());
    let h = file.handoff.unwrap();
    assert!(h.enabled);
    assert!((h.threshold - 0.7).abs() < f64::EPSILON);
    assert_eq!(h.keep_turns, 2);
    assert!(file.update.is_some());
    let u = file.update.unwrap();
    assert_eq!(u.source, "github");
    assert_eq!(u.channel, "stable");
}

// ── update_config tests ──

#[test]
fn update_config_from_toml() {
    let input = r#"
default_model = "balanced"

[model.strong]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "high"

[model.balanced]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "medium"

[model.light]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"
think = "off"

[provider.anthropic]
format = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "test-key"

[update]
source = "mirror"
channel = "alpha"

[update.sources]
custom = "https://example.com/latest.json"
"#;
    let file: ConfigFile = toml::from_str(input).unwrap();
    let u = file.update.unwrap();
    assert_eq!(u.source, "mirror");
    assert_eq!(u.channel, "alpha");
    assert_eq!(u.sources["custom"], "https://example.com/latest.json");
}

#[test]
fn update_config_defaults_when_absent() {
    let input = r#"
default_model = "balanced"

[model.strong]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "high"

[model.balanced]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "medium"

[model.light]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"
think = "off"

[provider.anthropic]
format = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "test-key"
"#;
    let file: ConfigFile = toml::from_str(input).unwrap();
    assert!(file.update.is_none());
    let resolved = ConfigFile::resolve(&file).unwrap();
    assert_eq!(resolved.update.source, "github");
    assert_eq!(resolved.update.channel, "stable");
    assert!(resolved.update.sources.is_empty());
}

#[test]
fn update_config_propagated_to_resolved_config() {
    let dir = temp_config(
        r#"
default_model = "balanced"

[model.strong]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "high"

[model.balanced]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "medium"

[model.light]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"
think = "off"

[provider.anthropic]
format = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "test-key"

[update]
source = "mirror"
channel = "alpha"
"#,
    );
    let path = dir.path().join("config.toml");
    let file = load_and_patch_config(&path).unwrap();
    let config = file.resolve().unwrap();
    assert_eq!(config.update.source, "mirror");
    assert_eq!(config.update.channel, "alpha");
}

// ── patch_defaults tests ──

const PATCH_FULL_CONFIG: &str = r#"
default_model = "balanced"

[model.strong]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "high"

[model.balanced]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "medium"

[model.light]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"
think = "off"

[provider.anthropic]
format = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "test-key"

[handoff]
enabled = true
threshold = 0.8
keep_turns = 3

[plugin]
enabled = true

[update]
source = "github"
channel = "stable"
"#;

#[test]
fn no_change_when_complete() {
    let (patched, changed) = config_patch_defaults(PATCH_FULL_CONFIG).unwrap();
    assert!(!changed);
    assert_eq!(patched, PATCH_FULL_CONFIG);
}

#[test]
fn fills_missing_handoff() {
    let input = r#"
default_model = "balanced"

[model.strong]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "high"

[model.balanced]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "medium"

[model.light]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"
think = "off"

[provider.anthropic]
format = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "test-key"
"#;
    let (patched, changed) = config_patch_defaults(input).unwrap();
    assert!(changed);
    let file: ConfigFile = toml::from_str(&patched).unwrap();
    assert!(file.handoff.is_some());
    let h = file.handoff.unwrap();
    assert!(h.enabled);
    assert!((h.threshold - 0.7).abs() < f64::EPSILON);
    assert_eq!(h.keep_turns, 2);
}

#[test]
fn fills_missing_update() {
    let input = r#"
default_model = "balanced"

[model.strong]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "high"

[model.balanced]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "medium"

[model.light]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"
think = "off"

[provider.anthropic]
format = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "test-key"

[handoff]
enabled = true
threshold = 0.7
keep_turns = 2

[plugin]
enabled = true
"#;
    let (patched, changed) = config_patch_defaults(input).unwrap();
    assert!(changed);
    let file: ConfigFile = toml::from_str(&patched).unwrap();
    let u = file.update.unwrap();
    assert_eq!(u.source, "github");
    assert_eq!(u.channel, "stable");
}

#[test]
fn preserves_user_comments() {
    let input = r#"# My custom config
default_model = "balanced"

[model.strong]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "high"

[model.balanced]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "medium"

[model.light]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"
think = "off"

[provider.anthropic]
format = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "test-key"
"#;
    let (patched, _) = config_patch_defaults(input).unwrap();
    assert!(patched.contains("# My custom config"));
}

#[test]
fn preserves_existing_handoff_values() {
    let input = r#"
default_model = "balanced"

[model.strong]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "high"

[model.balanced]
provider = "anthropic"
model = "claude-sonnet-4-6"
think = "medium"

[model.light]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"
think = "off"

[provider.anthropic]
format = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "test-key"

[handoff]
enabled = false
threshold = 0.5
keep_turns = 5

[plugin]
enabled = true

[update]
source = "github"
channel = "stable"
"#;
    let (patched, changed) = config_patch_defaults(input).unwrap();
    assert!(!changed);
    let file: ConfigFile = toml::from_str(&patched).unwrap();
    let h = file.handoff.unwrap();
    assert!(!h.enabled);
    assert!((h.threshold - 0.5).abs() < f64::EPSILON);
    assert_eq!(h.keep_turns, 5);
}

#[test]
fn rejects_invalid_toml() {
    let result = config_patch_defaults("not [valid toml");
    assert!(result.is_err());
}

// ── ThinkLevel tests ──

#[test]
fn think_level_overhead_tokens_off() {
    assert_eq!(ThinkLevel::Off.overhead_tokens(), 0);
}

#[test]
fn think_level_overhead_tokens_low() {
    assert_eq!(ThinkLevel::Low.overhead_tokens(), 1024);
}

#[test]
fn think_level_overhead_tokens_medium() {
    assert_eq!(ThinkLevel::Medium.overhead_tokens(), 4096);
}

#[test]
fn think_level_overhead_tokens_high() {
    assert_eq!(ThinkLevel::High.overhead_tokens(), 16000);
}

#[test]
fn think_level_from_str_success() {
    assert_eq!("off".parse::<ThinkLevel>().unwrap(), ThinkLevel::Off);
    assert_eq!("low".parse::<ThinkLevel>().unwrap(), ThinkLevel::Low);
    assert_eq!("medium".parse::<ThinkLevel>().unwrap(), ThinkLevel::Medium);
    assert_eq!("high".parse::<ThinkLevel>().unwrap(), ThinkLevel::High);
}

#[test]
fn think_level_from_str_invalid() {
    let err = "invalid".parse::<ThinkLevel>().unwrap_err();
    assert!(err.to_string().contains("ThinkLevel"));
}
