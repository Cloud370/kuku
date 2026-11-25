use serde_json::json;

use crate::config::{Config, ModelCapabilityDeclaration, parse_config_text};

use super::list_models;
use super::{MAX_DEFAULT_BYTES, MAX_PURPOSE_BYTES, MAX_STRUCTURED_ROWS_BYTES, MAX_TEXT_BYTES};

const CONFIG: &str = r#"
default_model = { connection = "gateway", model = "fixture" }

[provider.gateway]
format = "openai-chat"
base_url = "http://127.0.0.1:9"
api_key = "test-credential-never-in-catalog"
profile = "compatible"

[model.gateway.fixture]
max_output_tokens = 1024
"#;

#[test]
fn catalog_exposes_configured_modalities_separately_from_runtime_support() {
    let mut raw = parse_config_text(CONFIG).unwrap();
    raw.provider
        .get_mut("gateway")
        .unwrap()
        .options
        .model_capabilities
        .entry("fixture".into())
        .or_default()
        .manual = Some(ModelCapabilityDeclaration {
        input_modalities: Some(vec!["text".into(), "image".into()]),
        output_modalities: Some(vec!["text".into(), "image".into()]),
        ..Default::default()
    });
    let config = raw.resolve().unwrap();
    let result = list_models(&json!({}), &config, true, None);
    assert_eq!(result.status, "ok");
    assert!(!result.truncated);
    assert!(
        result
            .model_content
            .contains("Default for new Sessions: \"gateway\" \"fixture\"")
    );
    assert!(
        result
            .model_content
            .contains("[\"text\",\"image\"] → [\"text\",\"image\"]")
    );
    assert!(result.model_content.contains(" | supported | \"text\""));
    assert!(!result.model_content.contains("test-credential"));
    let payload = result.structured.unwrap();
    assert_eq!(payload["models"][0]["image_input"], "supported");
    assert_eq!(
        payload["models"][0]["input_modalities"],
        json!(["text", "image"])
    );
    assert_eq!(
        payload["models"][0]["output_modalities"],
        json!(["text", "image"])
    );
    assert_eq!(payload["models"][0]["output"], "text");
    assert_eq!(payload["omitted_routes"], 0);
    let custom_adapter = list_models(&json!({}), &config, false, None);
    assert!(
        custom_adapter
            .model_content
            .contains("unsupported on this route | \"text\"")
    );
    let custom_payload = custom_adapter.structured.unwrap();
    assert_eq!(
        custom_payload["models"][0]["input_modalities"],
        json!(["text", "image"])
    );
    assert_eq!(
        custom_payload["models"][0]["image_input"],
        "unsupported_on_route"
    );
    assert_eq!(
        list_models(&json!({"query":"fixture"}), &config, true, None).status,
        "error"
    );
}

#[test]
fn model_catalog_bounds_both_representations_without_cutting_model_ids() {
    let mut raw = parse_config_text(CONFIG).unwrap();
    let routes = raw.model.get_mut("gateway").unwrap();
    let route = routes["fixture"].clone();
    let enormous = "long".repeat(163_840);
    let mut described = route.clone();
    described.purpose = Some("🦀".repeat(150_000));
    routes.insert("fixture".into(), described.clone());
    routes.insert(enormous.clone(), route);
    for n in 0..512 {
        let mut described = described.clone();
        described.purpose = Some("auditor ".repeat(32));
        routes.insert(format!("extra-{n:03}"), described);
    }
    let config: Config = raw.resolve().unwrap();
    let result = list_models(&json!({}), &config, true, None);
    assert_eq!(result.status, "ok");
    assert!(result.truncated);
    assert!(result.model_content.len() <= MAX_TEXT_BYTES);
    assert!(result.model_content.contains("routes omitted"));
    let payload = result.structured.unwrap();
    let rows = payload["models"].as_array().unwrap();
    assert!(payload.to_string().len() <= MAX_STRUCTURED_ROWS_BYTES + MAX_DEFAULT_BYTES + 512);
    assert_eq!(
        rows.len() + payload["omitted_routes"].as_u64().unwrap() as usize,
        514
    );
    assert!(rows.iter().all(|row| row["model"] != enormous));
    let shown = rows.iter().find(|row| row["model"] == "fixture");
    // Alphabetically late routes can omit the long description entirely at this scale.
    if let Some(row) = shown {
        assert_eq!(row["purpose_truncated"], true);
        assert!(row["purpose"].as_str().unwrap().len() <= MAX_PURPOSE_BYTES + '…'.len_utf8());
    }
}

#[test]
fn long_optional_purpose_keeps_the_exact_route_usable_in_a_small_catalog() {
    let mut raw = parse_config_text(CONFIG).unwrap();
    raw.model
        .get_mut("gateway")
        .unwrap()
        .get_mut("fixture")
        .unwrap()
        .purpose = Some("🦀".repeat(800));
    let result = list_models(&json!({}), &raw.resolve().unwrap(), true, None);
    assert!(result.truncated);
    assert!(result.model_content.contains("Some metadata was shortened"));
    let payload = result.structured.unwrap();
    assert_eq!(payload["omitted_routes"], 0);
    assert_eq!(payload["models"][0]["model"], "fixture");
    let purpose = payload["models"][0]["purpose"].as_str().unwrap();
    assert!(purpose.len() <= MAX_PURPOSE_BYTES + '…'.len_utf8());
    assert!(purpose.ends_with('…'));
}
