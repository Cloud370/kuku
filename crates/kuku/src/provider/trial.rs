use std::io::Write;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use flate2::{write::ZlibEncoder, Compression};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio_stream::StreamExt;

use crate::config::{ProviderConfig, ProviderProfile};
use crate::{Error, Result};

use super::capabilities::resolve_settings;
use super::cost::{request_cost, RequestCost};
use super::prepare::{apply_settings, request_url};
use super::report::{normalize_usage, ReportAccumulator};
use super::types::{
    PreparedProviderRequest, ProviderKind, ProviderReport, ProviderUsage, ResolvedProvider,
    SecretString,
};

pub const TRIAL_OUTPUT_TOKENS: u32 = 128;
const TRIAL_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_TRIAL_RESPONSE: usize = 512 * 1024;
const COLORS: [(&str, [u8; 3]); 6] = [
    ("red", [255, 0, 0]),
    ("green", [0, 192, 0]),
    ("blue", [0, 0, 255]),
    ("yellow", [255, 255, 0]),
    ("white", [255, 255, 255]),
    ("black", [0, 0, 0]),
];

/// One opted-in, bounded request sent against the selected model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelTrial {
    Text {
        effort: Option<String>,
        service_tier: Option<String>,
    },
    ImageInput {
        service_tier: Option<String>,
    },
}

/// Timing-independent trial evidence a host can retain or render.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrialResult {
    pub text_returned: bool,
    pub observed_reasoning: bool,
    pub reported_effort: Option<String>,
    pub image_match: Option<bool>,
    pub expected_colors: Option<[String; 4]>,
    pub observed_colors: Vec<String>,
    pub usage: Option<ProviderUsage>,
    pub provider_report: Option<ProviderReport>,
    pub cost: RequestCost,
}

/// Test only the selected connection and model with synthetic input.
pub async fn trial_model(
    connection: &ProviderConfig,
    model: &str,
    trial: ModelTrial,
) -> Result<TrialResult> {
    tokio::time::timeout(TRIAL_TIMEOUT, trial_request(connection, model, trial))
        .await
        .map_err(|_| Error::InvalidArgument("Model trial timed out.".into()))?
}

async fn trial_request(
    connection: &ProviderConfig,
    model: &str,
    trial: ModelTrial,
) -> Result<TrialResult> {
    let capabilities = crate::adapter::inspect_connection_model(connection, model)?;
    let mut challenge = None;
    let (effort, service_tier) = match trial {
        ModelTrial::Text {
            effort,
            service_tier,
        } => (effort, service_tier),
        ModelTrial::ImageInput { service_tier } => {
            let colors = std::array::from_fn(|_| fastrand::usize(..COLORS.len()));
            challenge = Some((colors, color_grid(colors)?));
            (
                capabilities
                    .reasoning_efforts
                    .iter()
                    .find(|effort| *effort == "none")
                    .cloned(),
                service_tier,
            )
        }
    };
    let settings = resolve_settings("trial", &capabilities, effort, None, service_tier)?;
    let config = ResolvedProvider {
        connection_name: "trial".into(),
        model: model.into(),
        kind: ProviderKind::from(connection.format),
        base_url: connection.base_url.clone(),
        api_key: SecretString::from_config(connection.api_key.clone()),
        options: connection.options.clone(),
        capabilities,
        settings,
        ..ResolvedProvider::default()
    };
    let mut body = trial_body(&config, challenge.as_ref().map(|(_, data)| data.as_str()));
    apply_settings(&config, &mut body);
    body.as_object_mut()
        .expect("trial body")
        .remove("stream_options");
    if config.capabilities.profile == ProviderProfile::OpenRouter {
        body["provider"] = serde_json::to_value(config.options.routing.clone().unwrap_or_default())
            .map_err(|_| Error::InvalidArgument("Invalid routing for the model trial.".into()))?;
    }
    let prepared = PreparedProviderRequest {
        kind: config.kind,
        connection_name: config.connection_name.clone(),
        model: model.into(),
        url: request_url(&config).map_err(trial_error)?,
        body,
        settings: config.settings.clone(),
        correlation_id: None,
    };
    let response = super::transport::send_trial(&config, &prepared)
        .await
        .map_err(trial_error)?;
    let mut stream = response.response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| invalid_receipt())?;
        if bytes.len().saturating_add(chunk.len()) > MAX_TRIAL_RESPONSE {
            return Err(invalid_receipt());
        }
        bytes.extend_from_slice(&chunk);
    }
    let response: Value = serde_json::from_slice(&bytes).map_err(|_| invalid_receipt())?;
    interpret_trial(
        connection,
        model,
        &response,
        challenge.map(|(colors, _)| colors),
    )
}

fn trial_body(config: &ResolvedProvider, image: Option<&str>) -> Value {
    let prompt = if image.is_some() {
        "What are the colors at the top left, top right, bottom left, and bottom right? Give one English color per position, in that order."
    } else {
        "What is 17 times 23 minus 14? Reply with the number only."
    };
    let mut content = vec![match config.kind {
        ProviderKind::OpenAiResponses => json!({"type":"input_text","text":prompt}),
        _ => json!({"type":"text","text":prompt}),
    }];
    if let Some(data) = image {
        let data_url = format!("data:image/png;base64,{data}");
        content.push(match config.kind {
            ProviderKind::OpenAiResponses => json!({
                "type":"input_image","image_url":data_url,"detail":"low"
            }),
            ProviderKind::OpenAiCompatible => json!({
                "type":"image_url","image_url":{"url":data_url,"detail":"low"}
            }),
            ProviderKind::Anthropic => json!({
                "type":"image","source":{"type":"base64","media_type":"image/png","data":data}
            }),
        });
    }
    let messages = json!([{"role":"user","content":content}]);
    match config.kind {
        ProviderKind::OpenAiResponses => json!({
            "model":config.model,"input":messages,"max_output_tokens":TRIAL_OUTPUT_TOKENS
        }),
        ProviderKind::OpenAiCompatible | ProviderKind::Anthropic => json!({
            "model":config.model,"messages":messages,"max_tokens":TRIAL_OUTPUT_TOKENS
        }),
    }
}

fn interpret_trial(
    connection: &ProviderConfig,
    model: &str,
    response: &Value,
    expected: Option<[usize; 4]>,
) -> Result<TrialResult> {
    use crate::config::ProviderFormat;
    let (text, reasoning_block) = match connection.format {
        ProviderFormat::OpenAiChat => {
            let message = response
                .pointer("/choices/0/message")
                .ok_or_else(invalid_receipt)?;
            (
                message["content"].as_str().unwrap_or_default().to_owned(),
                ["reasoning", "reasoning_content", "reasoning_details"]
                    .iter()
                    .any(|name| present_content(&message[*name])),
            )
        }
        ProviderFormat::OpenAiResponses => {
            let output = response["output"].as_array().ok_or_else(invalid_receipt)?;
            let content = output
                .iter()
                .filter_map(|item| item["content"].as_array())
                .flatten();
            (
                texts(content, "output_text"),
                output.iter().any(|item| item["type"] == "reasoning"),
            )
        }
        ProviderFormat::Anthropic => {
            let content = response["content"].as_array().ok_or_else(invalid_receipt)?;
            (
                texts(content.iter(), "text"),
                content.iter().any(|item| {
                    matches!(
                        item["type"].as_str(),
                        Some("thinking" | "redacted_thinking")
                    )
                }),
            )
        }
    };
    let mut reports = ReportAccumulator::default();
    let report = reports.observe(response).map_err(trial_error)?;
    let usage = report.as_ref().and_then(|report| {
        report
            .raw_usage
            .as_ref()
            .map(|raw| normalize_usage(connection.format, raw))
    });
    let observed_reasoning = reasoning_block
        || [
            "/usage/output_tokens_details/reasoning_tokens",
            "/usage/completion_tokens_details/reasoning_tokens",
        ]
        .iter()
        .any(|path| {
            response
                .pointer(path)
                .and_then(Value::as_u64)
                .is_some_and(|count| count > 0)
        });
    let expected_colors = expected.map(|indexes| indexes.map(|index| COLORS[index].0.to_owned()));
    let observed_colors = expected_colors
        .as_ref()
        .map_or_else(Vec::new, |_| recognized_colors(&text));
    let image_match = expected_colors
        .as_ref()
        .map(|expected| observed_colors.as_slice() == expected.as_slice());
    let cost = request_cost(
        connection.options.model_pricing.get(model),
        usage.as_ref(),
        report.as_ref(),
    );
    Ok(TrialResult {
        text_returned: !text.trim().is_empty(),
        observed_reasoning,
        reported_effort: report
            .as_ref()
            .and_then(|report| report.reasoning_effort.clone()),
        image_match,
        expected_colors,
        observed_colors,
        usage,
        provider_report: report,
        cost,
    })
}

fn recognized_colors(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_ascii_alphabetic())
        .filter_map(|word| {
            COLORS
                .iter()
                .find(|(color, _)| word.eq_ignore_ascii_case(color))
                .map(|(color, _)| (*color).to_owned())
        })
        .take(5)
        .collect()
}

fn texts<'a>(items: impl Iterator<Item = &'a Value>, kind: &str) -> String {
    items
        .filter(|item| item["type"] == kind)
        .filter_map(|item| item["text"].as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

fn present_content(value: &Value) -> bool {
    value.as_str().is_some_and(|value| !value.is_empty())
        || value.as_array().is_some_and(|value| !value.is_empty())
}

fn invalid_receipt() -> Error {
    Error::InvalidArgument("Model endpoint did not return a readable trial response.".into())
}

fn trial_error(error: super::types::ProviderFailure) -> Error {
    Error::Provider {
        kind: error.kind,
        message: error.status.map_or(error.message.clone(), |status| {
            format!("HTTP {status} · {}", error.message)
        }),
        provider: None,
        model: None,
    }
}

fn color_grid(colors: [usize; 4]) -> Result<String> {
    const SIDE: usize = 64;
    let mut raw = Vec::with_capacity(SIDE * (SIDE * 3 + 1));
    for y in 0..SIDE {
        raw.push(0);
        for x in 0..SIDE {
            let index = usize::from(y >= SIDE / 2) * 2 + usize::from(x >= SIDE / 2);
            raw.extend_from_slice(&COLORS[colors[index]].1);
        }
    }
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&raw)?;
    let mut data = b"\x89PNG\r\n\x1a\n".to_vec();
    let side = (SIDE as u32).to_be_bytes();
    let mut header = side.repeat(2);
    header.extend_from_slice(&[8, 2, 0, 0, 0]);
    png_chunk(&mut data, b"IHDR", &header);
    png_chunk(&mut data, b"IDAT", &encoder.finish()?);
    png_chunk(&mut data, b"IEND", &[]);
    Ok(STANDARD.encode(data))
}

fn png_chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = png.len();
    png.extend_from_slice(kind);
    png.extend_from_slice(data);
    png.extend_from_slice(&crc32fast::hash(&png[start..]).to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::recognized_colors;

    #[test]
    fn accepts_position_labels_and_keeps_extra_colors_for_mismatch() {
        assert_eq!(
            ["red", "red", "blue", "yellow"],
            recognized_colors(
                "Top left: Red, top right: red; bottom left: blue, bottom right: yellow."
            )
            .as_slice()
        );
        assert_eq!(
            5,
            recognized_colors("red yellow blue green black white").len()
        );
    }
}
