use std::fmt::Write as _;

use serde_json::{Value, json};

use crate::AgentConfig;
use crate::config::{Config, ModelInfo};
use crate::tool::{ToolDefinition, ToolResultEnvelope};

const MAX_TEXT_BYTES: usize = 24 * 1024;
const MAX_STRUCTURED_ROWS_BYTES: usize = 128 * 1024;
const MAX_DEFAULT_BYTES: usize = 4096;
const MAX_PURPOSE_BYTES: usize = 240;
const OMISSION_FOOTER_BYTES: usize = 180;

pub(crate) fn definition() -> ToolDefinition {
    ToolDefinition {
        name: "list_models".into(),
        description: "List the currently configured model routes and their capabilities. Use this to resolve an uncertain model name or verify a configuration change. Does not contact providers.".into(),
        input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
        read_only: true,
        max_result_chars: MAX_TEXT_BYTES,
        risk: "read".into(),
    }
}

pub(crate) fn list_models(
    args: &Value,
    config: &Config,
    builtin_adapter: bool,
    max_output_tokens: Option<u32>,
) -> ToolResultEnvelope {
    if args.as_object().is_none_or(|args| !args.is_empty()) {
        return ToolResultEnvelope::error(
            "invalid model list arguments",
            "list_models takes no arguments",
        );
    }
    let default = config.default_model();
    let default_omitted = default.connection.len().saturating_add(default.model.len())
        > MAX_DEFAULT_BYTES
        || serde_json::to_string(default).is_ok_and(|value| value.len() > MAX_DEFAULT_BYTES);
    let mut text = if default_omitted {
        "Default for new Sessions: omitted (route ID is too long)\n".to_owned()
    } else {
        format!(
            "Default for new Sessions: {} {}\n",
            json!(&default.connection),
            json!(&default.model)
        )
    };
    text.push_str("Connection | Model | Thinking | Tier | Declared input → output | Image input on this route | Requested output | Purpose\n");
    let mut rows = Vec::new();
    let mut structured_bytes = 0;
    let mut omitted = 0;
    let mut shortened = 0;
    for info in config.model_infos() {
        let (line, row, purpose_truncated) =
            render_route(info, config, builtin_adapter, max_output_tokens);
        let row_bytes = row.to_string().len() + 1;
        if line.len() + text.len() + 1 + OMISSION_FOOTER_BYTES > MAX_TEXT_BYTES
            || row_bytes + structured_bytes > MAX_STRUCTURED_ROWS_BYTES
        {
            omitted += 1;
            continue;
        }
        structured_bytes += row_bytes;
        text.push_str(&line);
        text.push('\n');
        rows.push(row);
        shortened += usize::from(purpose_truncated);
    }
    if rows.is_empty() && omitted == 0 {
        text.push_str("No models are configured.\n");
    }
    if omitted > 0 {
        let _ = writeln!(
            text,
            "{omitted} routes omitted to fit this result. See kuku config models for all configured pairs."
        );
    }
    if default_omitted || shortened > 0 {
        text.push_str("Some metadata was shortened to fit this result.\n");
    }
    let summary = format!("{} configured models", rows.len() + omitted);
    let structured = json!({
        "kind": "model_catalog",
        "default_model": (!default_omitted).then_some(default),
        "models": rows,
        "omitted_routes": omitted,
        "default_omitted": default_omitted,
    });
    if omitted > 0 || shortened > 0 || default_omitted {
        ToolResultEnvelope::ok_truncated(summary, text, structured)
    } else {
        ToolResultEnvelope::ok(summary, text, structured)
    }
}

fn render_route(
    mut info: ModelInfo,
    config: &Config,
    builtin_adapter: bool,
    max_output_tokens: Option<u32>,
) -> (String, Value, bool) {
    let route = format!("{} | {}", json!(&info.connection), json!(&info.model));
    let selected = match crate::session::runtime::resolve_agent_model(
        config,
        &AgentConfig {
            connection: Some(info.connection.clone()),
            model: Some(info.model.clone()),
            ..AgentConfig::default()
        },
        max_output_tokens,
    ) {
        Ok(selected) => selected,
        Err(error) => {
            return (
                format!("{route} | unavailable: {}", json!(error.to_string())),
                json!({"connection":info.connection,"model":info.model,"error":error.to_string()}),
                false,
            );
        }
    };
    let purpose_truncated = info.purpose.len() > MAX_PURPOSE_BYTES;
    if purpose_truncated {
        info.purpose
            .truncate(info.purpose.floor_char_boundary(MAX_PURPOSE_BYTES));
        info.purpose.push('…');
    }
    info.image_input = selected.capabilities.image_input_support(builtin_adapter);
    info.reasoning_choices = selected
        .capabilities
        .selectable_reasoning_efforts()
        .map(str::to_owned)
        .collect();
    info.reasoning_effort = selected.settings.reasoning_effort.or_else(|| {
        selected
            .capabilities
            .reasoning_default
            .map(|level| level.value)
    });
    info.service_tier = selected.settings.service_tier;
    info.service_choices = selected
        .capabilities
        .service_tier_source
        .map(|_| selected.capabilities.service_tiers);
    info.input_modalities = selected
        .capabilities
        .input_modalities
        .map(|entry| entry.value);
    info.output_modalities = selected
        .capabilities
        .output_modalities
        .map(|entry| entry.value);
    info.output = selected.output;
    let thinking = choices(
        info.reasoning_effort.as_deref(),
        Some(&info.reasoning_choices),
    );
    let tier = choices(
        info.service_tier.as_deref(),
        info.service_choices.as_deref(),
    );
    let mut line = format!(
        "{route} | {thinking} | {tier} | {} → {} | {} | {}",
        declared(info.input_modalities.as_deref()),
        declared(info.output_modalities.as_deref()),
        info.image_input.label(),
        json!(info.output),
    );
    if !info.purpose.is_empty() {
        let _ = write!(line, " | {}", json!(&info.purpose));
    }
    let mut row = json!(info);
    if purpose_truncated {
        row["purpose_truncated"] = json!(true);
    }
    (line, row, purpose_truncated)
}

fn declared(values: Option<&[String]>) -> String {
    values.map_or_else(|| "?".to_owned(), |values| json!(values).to_string())
}

fn choices(default: Option<&str>, supported: Option<&[String]>) -> String {
    let default = default.map_or_else(|| "default".into(), |default| json!(default).to_string());
    match supported {
        Some([]) => default,
        Some(values) => format!("{default} {}", json!(values)),
        None => format!("{default} (?)"),
    }
}

#[cfg(test)]
mod tests;
