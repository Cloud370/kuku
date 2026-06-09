use super::http_client;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

use crate::context::{CanonicalMessage, MessageBlock, Role};

use super::chunk::ProviderChunk;
use super::error::{classify_http_error, transport_error};
use super::sse::stream_sse_events;
use super::types::{ProviderFailure, ProviderRequest, ResolvedProvider};

const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_OUTPUT_TOKENS: u64 = 4096;

const TRUNCATED_STREAM_MESSAGE: &str = "provider stream ended before message_stop";

pub(crate) fn messages_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}/messages")
    } else {
        format!("{base}/v1/messages")
    }
}

pub(crate) fn render_body(request: &ProviderRequest<'_>) -> Value {
    let mut messages = request
        .assembly
        .prelude_messages
        .iter()
        .map(convert_canonical_message)
        .collect::<Vec<_>>();
    if let Some(summary) = &request.assembly.handoff_summary {
        let template = &request.catalog.handoff_context.text;
        let content = template.replace("{{handoff_summary}}", summary);
        messages.push(json!({
            "role": "user",
            "content": content,
        }));
    }
    messages.extend(
        request
            .assembly
            .history
            .iter()
            .map(convert_canonical_message),
    );

    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "max_tokens": request.max_output_tokens.unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS as u32),
        "stream": false,
        "system": request.assembly.system_prompt,
        "cache_control": {"type": "ephemeral"},
    });

    if let Some(temperature) = request.temperature {
        body["temperature"] = json!(temperature);
    }
    if request.think_level != crate::config::ThinkLevel::Off {
        body["thinking"] = json!({
            "type": "adaptive",
            "display": "summarized",
        });
        let effort = match request.think_level.as_str() {
            "low" => "low",
            "medium" => "medium",
            "high" => "max",
            _ => "medium",
        };
        body["output_config"] = json!({ "effort": effort });
    }
    if !request.assembly.tools.is_empty() {
        body["tools"] = json!(request
            .assembly
            .tools
            .iter()
            .map(|schema| {
                json!({
                    "name": schema.name,
                    "description": schema.description,
                    "input_schema": schema.input_schema,
                })
            })
            .collect::<Vec<_>>());
    }

    body
}

fn convert_canonical_message(message: &CanonicalMessage) -> Value {
    match message.role {
        Role::User => {
            let content = message
                .blocks
                .iter()
                .filter_map(|block| match block {
                    MessageBlock::Text(text) => Some(json!({"type": "text", "text": text})),
                    MessageBlock::ToolResult(result) => Some(json!({
                        "type": "tool_result",
                        "tool_use_id": result.tool_call_id,
                        "content": result.model_content,
                    })),
                    MessageBlock::ToolUse(_) | MessageBlock::Thinking(_) => None,
                })
                .collect::<Vec<_>>();

            json!({"role": "user", "content": content})
        }
        Role::Assistant => {
            let content = message
                .blocks
                .iter()
                .filter_map(|block| match block {
                    MessageBlock::Thinking(text) => {
                        // NOTE: Anthropic API requires a `signature` field on replayed thinking
                        // blocks for multi-turn sessions. Proxies that strip signatures will
                        // accept blocks without it; direct Anthropic API access needs signature
                        // capture from content_block_stop events (future work).
                        Some(json!({"type": "thinking", "thinking": text}))
                    }
                    MessageBlock::Text(text) => Some(json!({"type": "text", "text": text})),
                    MessageBlock::ToolUse(tool_use) => Some(json!({
                        "type": "tool_use",
                        "id": tool_use.id,
                        "name": tool_use.name,
                        "input": tool_use.args,
                    })),
                    MessageBlock::ToolResult(_) => None,
                })
                .collect::<Vec<_>>();

            json!({"role": "assistant", "content": content})
        }
    }
}

pub(crate) async fn stream(
    config: &ResolvedProvider,
    request: &ProviderRequest<'_>,
) -> Result<super::ProviderChunkStream, ProviderFailure> {
    let mut body = render_body(request);
    body["stream"] = json!(true);
    let url = messages_url(&config.base_url);
    let client = http_client::api_client();

    let response = client
        .post(url)
        .header("content-type", "application/json")
        .header("x-api-key", config.api_key.expose())
        .header("anthropic-version", ANTHROPIC_VERSION)
        .json(&body)
        .send()
        .await
        .map_err(|error| transport_error(&error))?;

    let status = response.status();
    let request_id = response
        .headers()
        .get("request-id")
        .and_then(|v| v.to_str().ok())
        .map(ToOwned::to_owned);
    if !status.is_success() {
        let body_text = response.text().await.unwrap_or_default();
        let mut failure = classify_http_error(status.as_u16(), &body_text);
        failure.provider_request_id = request_id;
        return Err(failure);
    }

    let parser = Arc::new(Mutex::new(AnthropicSseParser::new()));
    let frame_parser = Arc::clone(&parser);
    let eof_parser = Arc::clone(&parser);
    Ok(stream_sse_events(
        response,
        move |frame| frame_parser.lock().unwrap().feed(frame),
        move || eof_parser.lock().unwrap().finish(),
    ))
}

struct AnthropicSseParser {
    chunks: Vec<ProviderChunk>,
    saw_terminal: bool,
}

impl AnthropicSseParser {
    fn new() -> Self {
        Self {
            chunks: Vec::new(),
            saw_terminal: false,
        }
    }

    fn feed(&mut self, frame: &str) -> Result<Vec<ProviderChunk>, ProviderFailure> {
        if frame.is_empty() {
            return Ok(self.take_chunks());
        }

        let mut event_type = "";
        let mut data_str = "";

        for line in frame.lines() {
            if let Some(rest) = line.strip_prefix("event:") {
                event_type = rest.trim();
            } else if let Some(rest) = line.strip_prefix("data:") {
                data_str = rest.trim();
            }
        }

        if data_str.is_empty() || event_type == "ping" {
            return Ok(self.take_chunks());
        }

        let data: Value = match serde_json::from_str(data_str) {
            Ok(v) => v,
            Err(_) => return Ok(self.take_chunks()),
        };

        match event_type {
            "message_start" => {
                if let Some(msg) = data.get("message") {
                    let rid = msg
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    self.chunks
                        .push(ProviderChunk::StreamStart { request_id: rid });
                    if let Some(usage) = msg.get("usage") {
                        self.chunks.push(ProviderChunk::StreamUsage {
                            input_tokens: usage
                                .get("input_tokens")
                                .and_then(Value::as_u64)
                                .unwrap_or(0),
                            output_tokens: 0,
                            cache_read_input_tokens: 0,
                            cache_creation_input_tokens: 0,
                        });
                    }
                }
            }
            "content_block_start" => {
                let index = data.get("index").and_then(Value::as_u64).unwrap_or(0);
                if let Some(block) = data.get("content_block") {
                    match block.get("type").and_then(Value::as_str) {
                        Some("text") => {}
                        Some("thinking") => {}
                        Some("tool_use") => {
                            let id = block
                                .get("id")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            let name = block
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            self.chunks.push(ProviderChunk::ToolCallStart {
                                index,
                                id: id.clone(),
                                name: name.clone(),
                            });
                        }
                        _ => {}
                    }
                }
            }
            "content_block_delta" => {
                let index = data.get("index").and_then(Value::as_u64).unwrap_or(0);
                if let Some(delta) = data.get("delta") {
                    match delta.get("type").and_then(Value::as_str) {
                        Some("text_delta") => {
                            let text = delta
                                .get("text")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            self.chunks.push(ProviderChunk::TextDelta { text });
                        }
                        Some("thinking_delta") => {
                            let text = delta
                                .get("thinking")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            self.chunks.push(ProviderChunk::ThinkingDelta { text });
                        }
                        Some("input_json_delta") => {
                            let fragment = delta
                                .get("partial_json")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            self.chunks
                                .push(ProviderChunk::ToolCallArgDelta { index, fragment });
                        }
                        _ => {}
                    }
                }
            }
            "content_block_stop" => {
                let index = data.get("index").and_then(Value::as_u64).unwrap_or(0);
                self.chunks.push(ProviderChunk::ContentBlockStop { index });
            }
            "message_delta" => {
                if let Some(delta) = data.get("delta") {
                    if let Some(reason) = delta.get("stop_reason").and_then(Value::as_str) {
                        self.chunks.push(ProviderChunk::StopReason {
                            reason: reason.to_string(),
                        });
                    }
                }
                if let Some(usage) = data.get("usage") {
                    self.chunks.push(ProviderChunk::StreamUsage {
                        input_tokens: usage
                            .get("input_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                        output_tokens: usage
                            .get("output_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                        cache_read_input_tokens: usage
                            .get("cache_read_input_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                        cache_creation_input_tokens: usage
                            .get("cache_creation_input_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                    });
                }
            }
            "message_stop" => {
                self.saw_terminal = true;
                self.chunks.push(ProviderChunk::StreamEnd);
            }
            "error" => {
                let code = data
                    .get("error")
                    .and_then(|e| e.get("type"))
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string();
                let message = data
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("server error")
                    .to_string();
                self.chunks
                    .push(ProviderChunk::ServerError { code, message });
            }
            _ => {}
        }

        Ok(self.take_chunks())
    }

    fn finish(&mut self) -> Result<Vec<ProviderChunk>, ProviderFailure> {
        if self.saw_terminal {
            return Ok(self.take_chunks());
        }

        Err(ProviderFailure {
            kind: crate::provider::types::ProviderFailureKind::Transport,
            message: TRUNCATED_STREAM_MESSAGE.to_string(),
            status: None,
            provider_request_id: None,
            retryable: true,
        })
    }

    fn take_chunks(&mut self) -> Vec<ProviderChunk> {
        std::mem::take(&mut self.chunks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{CanonicalMessage, ContextAssembly};
    use crate::prompt::PromptCatalog;
    use crate::provider::types::ProviderRequest;

    fn minimal_request<'a>(
        think_level: crate::config::ThinkLevel,
        catalog: &'a PromptCatalog,
    ) -> ProviderRequest<'a> {
        ProviderRequest {
            assembly: ContextAssembly {
                system_prompt: "test".into(),
                prelude_messages: vec![],
                history: vec![],
                tools: vec![],
                prompt_asset_sources: vec![],
                project_instruction_sources: vec![],
                memory_sources: vec![],
                runtime_context: None,
                handoff_summary: None,
            },
            catalog,
            current_input: crate::provider::types::CanonicalPromptInput {
                parts: vec![CanonicalMessage::user_text("input")],
            },
            model: "test-model".into(),
            max_output_tokens: Some(1024),
            temperature: None,
            stream: false,
            think_level,
            thinking: crate::config::ResolvedThinking {
                low: None,
                medium: None,
                high: None,
            },
        }
    }

    #[test]
    fn render_body_adaptive_thinking_high() {
        let catalog = crate::prompt::builtin_prompt_catalog();
        let body = render_body(&minimal_request(crate::config::ThinkLevel::High, &catalog));
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(body["thinking"]["display"], "summarized");
        assert_eq!(body["output_config"]["effort"], "max");
        assert!(body.get("budget_tokens").is_none());
    }

    #[test]
    fn render_body_adaptive_thinking_medium() {
        let catalog = crate::prompt::builtin_prompt_catalog();
        let body = render_body(&minimal_request(
            crate::config::ThinkLevel::Medium,
            &catalog,
        ));
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(body["output_config"]["effort"], "medium");
    }

    #[test]
    fn render_body_adaptive_thinking_low() {
        let catalog = crate::prompt::builtin_prompt_catalog();
        let body = render_body(&minimal_request(crate::config::ThinkLevel::Low, &catalog));
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(body["output_config"]["effort"], "low");
    }

    #[test]
    fn render_body_no_thinking_when_off() {
        let catalog = crate::prompt::builtin_prompt_catalog();
        let body = render_body(&minimal_request(crate::config::ThinkLevel::Off, &catalog));
        assert!(body.get("thinking").is_none());
        assert!(body.get("output_config").is_none());
    }

    #[test]
    fn render_body_includes_cache_control() {
        let catalog = crate::prompt::builtin_prompt_catalog();
        let body = render_body(&minimal_request(crate::config::ThinkLevel::Off, &catalog));
        let cc = body
            .get("cache_control")
            .expect("cache_control field should be present");
        assert_eq!(cc["type"], "ephemeral");
    }

    #[test]
    fn truncated_stream_does_not_emit_stream_end() {
        let mut parser = AnthropicSseParser::new();
        let mut chunks = parser
            .feed(
            "event: message_start\ndata: {\"message\":{\"id\":\"msg_truncated\",\"usage\":{\"input_tokens\":1}}}",
        )
            .unwrap();
        chunks.extend(parser.feed(
            "event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"partial\"}}",
        ).unwrap());
        chunks.extend(parser.feed("").unwrap());

        assert!(chunks.iter().any(|chunk| matches!(
            chunk,
            ProviderChunk::TextDelta { text } if text == "partial"
        )));
        assert!(!chunks
            .iter()
            .any(|chunk| matches!(chunk, ProviderChunk::StreamEnd)));
    }
}
