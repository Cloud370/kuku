use std::path::Path;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::tool::ToolResultEnvelope;

const SMALL_CONTENT_THRESHOLD: usize = 50_000;
const MAX_HTML_BYTES: usize = 10 * 1024 * 1024;
const CACHE_MAX_ENTRIES: usize = 200;
const CACHE_TTL: Duration = Duration::from_secs(300);

pub(crate) async fn fetch_web(
    args: &Value,
    _workspace: &Path,
    config: &crate::config::Config,
    catalog: &crate::prompt::PromptCatalog,
    parent_request: &crate::event::RequestScope,
    request_evidence_recorder: &dyn crate::query::provider::RequestEvidenceRecorder,
) -> ToolResultEnvelope {
    let Some(url) = args.get("url").and_then(Value::as_str) else {
        return ToolResultEnvelope::error("failed: missing url", "fetch_web requires url");
    };
    let Some(prompt) = args.get("prompt").and_then(Value::as_str) else {
        return ToolResultEnvelope::error("failed: missing prompt", "fetch_web requires prompt");
    };
    let Some(model_tier) = args.get("model_tier").and_then(Value::as_str) else {
        return ToolResultEnvelope::error(
            "failed: missing model_tier",
            "fetch_web requires model_tier — use a tier name from your configured model tiers",
        );
    };

    if let Err(e) = super::fetch_url::validate_url(url) {
        return e;
    }

    if let Some(cached) = cache_get(url) {
        return ToolResultEnvelope::ok(
            format!("fetched (cached): {url}"),
            cached,
            serde_json::json!({"kind": "fetch_web", "url": url, "cached": true}),
        );
    }

    let html = match fetch_html(url).await {
        Ok(html) => html,
        Err(e) => return e,
    };

    let markdown = match html_to_markdown(url, &html) {
        Ok(md) => md,
        Err(e) => return e,
    };

    let result = if markdown.len() < SMALL_CONTENT_THRESHOLD {
        markdown.clone()
    } else {
        match call_secondary_llm(
            &markdown,
            prompt,
            model_tier,
            config,
            catalog,
            parent_request,
            request_evidence_recorder,
        )
        .await
        {
            Ok(summary) => summary,
            Err(_) => {
                let (truncated, _) = super::common::join_bounded_strings(
                    &markdown.lines().map(String::from).collect::<Vec<_>>(),
                    SMALL_CONTENT_THRESHOLD,
                    "[Content truncated — LLM summarization failed]",
                );
                truncated
            }
        }
    };

    cache_put(url, &result);

    ToolResultEnvelope::ok(
        format!("fetched {url}"),
        result.clone(),
        serde_json::json!({
            "kind": "fetch_web",
            "url": url,
            "prompt": prompt,
            "model_tier": model_tier,
            "content_length": result.len(),
            "cached": false,
        }),
    )
}

async fn fetch_html(url: &str) -> Result<String, ToolResultEnvelope> {
    let client = crate::provider::http_client::fetch_client();
    let response = client.get(url).send().await.map_err(|e| {
        ToolResultEnvelope::error("failed: fetch error", format!("failed to fetch: {e}"))
    })?;

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !content_type.contains("text/html")
        && !content_type.contains("text/plain")
        && !content_type.contains("application/xhtml")
    {
        return Err(ToolResultEnvelope::error(
            "failed: not HTML content",
            format!(
                "Content-Type is '{content_type}', not HTML. Use fetch_url to download non-HTML resources.",
            ),
        ));
    }

    if let Some(Ok(len)) = response.content_length().map(usize::try_from) {
        if len > MAX_HTML_BYTES {
            return Err(ToolResultEnvelope::error(
                "failed: response too large",
                format!("HTML body is {len} bytes, exceeds {MAX_HTML_BYTES} byte limit"),
            ));
        }
    }

    use tokio_stream::StreamExt;
    let mut buf = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|e| ToolResultEnvelope::error("failed: read error", format!("{e}")))?;
        if buf.len() + chunk.len() > MAX_HTML_BYTES {
            return Err(ToolResultEnvelope::error(
                "failed: response too large",
                format!("HTML body exceeds {MAX_HTML_BYTES} byte limit"),
            ));
        }
        buf.extend_from_slice(&chunk);
    }
    String::from_utf8(buf).map_err(|e| {
        ToolResultEnvelope::error("failed: encoding error", format!("invalid UTF-8: {e}"))
    })
}

fn html_to_markdown(url: &str, html: &str) -> Result<String, ToolResultEnvelope> {
    let parsed_url = url::Url::parse(url).map_err(|e| {
        ToolResultEnvelope::error(
            "failed: invalid url",
            format!("failed to parse url for readability: {e}"),
        )
    })?;

    let clean_html = match readability::extractor::extract(&mut html.as_bytes(), &parsed_url) {
        Ok(product) => product.content,
        Err(_) => html.to_string(),
    };

    htmd::convert(&clean_html).map_err(|e| {
        ToolResultEnvelope::error(
            "failed: html conversion",
            format!("failed to convert HTML to Markdown: {e}"),
        )
    })
}

async fn call_secondary_llm(
    content: &str,
    prompt: &str,
    model_tier: &str,
    config: &crate::config::Config,
    catalog: &crate::prompt::PromptCatalog,
    parent_request: &crate::event::RequestScope,
    request_evidence_recorder: &dyn crate::query::provider::RequestEvidenceRecorder,
) -> Result<String, ToolResultEnvelope> {
    use tokio_stream::StreamExt;

    let resolved =
        crate::provider::config::resolve_config(crate::provider::config::ResolveConfigInput {
            tier: Some(model_tier.to_string()),
            config: Some(config.clone()),
            ..Default::default()
        })
        .or_else(|_| {
            crate::provider::config::resolve_config(crate::provider::config::ResolveConfigInput {
                config: Some(config.clone()),
                ..Default::default()
            })
        })
        .map_err(|e| {
            ToolResultEnvelope::error(
                "failed: resolve provider",
                format!("cannot resolve tier '{model_tier}': {e}"),
            )
        })?;

    let max_chars = resolved.max_context_tokens as usize * 3;
    let truncated: String = content
        .chars()
        .take(max_chars.saturating_sub(2_000))
        .collect();
    let user_text = format!("{prompt}\n\n---\n\n{truncated}");

    let assembly = crate::context::ContextAssembly {
        system_prompt: catalog.tools["fetch-web"].text.clone(),
        prelude_messages: vec![crate::context::CanonicalMessage {
            role: crate::context::Role::User,
            blocks: vec![crate::context::MessageBlock::Text(user_text)],
        }],
        history: vec![],
        tools: vec![],
        prompt_asset_sources: vec![],
        project_instruction_sources: vec![],
        memory_sources: vec![],
        runtime_context: None,
        handoff_summary: None,
    };
    let request = crate::provider::types::ProviderRequest {
        assembly,
        catalog,
        current_input: crate::provider::types::CanonicalPromptInput { parts: vec![] },
        model: resolved.model.clone(),
        max_output_tokens: Some(resolved.max_output_tokens),
        temperature: None,
        stream: true,
        think_level: resolved.think_level,
        thinking: resolved.thinking.clone(),
    };

    let request_scope = crate::event::RequestScope {
        execution: parent_request.execution.clone(),
        request_id: crate::event::RequestId::try_new().map_err(|error| {
            ToolResultEnvelope::error(
                "failed: request identity",
                format!("cannot create secondary request identity: {error}"),
            )
        })?,
    };
    let started_fact = crate::event::RequestStarted {
        scope: request_scope.clone(),
        cause: crate::event::RequestCause::ToolContinuation {
            parent_request_id: parent_request.request_id.clone(),
        },
        provider: crate::query::provider::request::provider_fact(&resolved.kind),
        model: resolved.model.clone(),
        started_at: time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|error| {
                ToolResultEnvelope::error(
                    "failed: request timestamp",
                    format!("cannot format secondary request timestamp: {error}"),
                )
            })?,
    };
    let (request_started, provider_result) =
        crate::query::provider::request::begin_provider_request(
            request_evidence_recorder,
            started_fact,
            crate::provider::stream_provider(&resolved, &request, None),
        )
        .await
        .map_err(lifecycle_error)?;
    let mut stream = match provider_result {
        Ok(stream) => stream,
        Err(failure) => {
            request_evidence_recorder
                .record_failed(crate::query::provider::request::failed(
                    request_scope,
                    request_started,
                    failure.provider_request_id.clone(),
                    None,
                    failure.kind,
                    failure.message.clone(),
                ))
                .map_err(lifecycle_error)?;
            return Err(ToolResultEnvelope::error(
                "failed: LLM call",
                format!("secondary LLM error: {failure:?}"),
            ));
        }
    };

    let mut response_text = String::new();
    let mut provider_request_id = None;
    let mut usage = None;
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(crate::provider::chunk::ProviderChunk::StreamStart { request_id }) => {
                provider_request_id = Some(request_id);
            }
            Ok(crate::provider::chunk::ProviderChunk::TextDelta { text }) => {
                response_text.push_str(&text);
            }
            Ok(crate::provider::chunk::ProviderChunk::StreamUsage {
                input_tokens,
                output_tokens,
                cache_read_input_tokens,
                cache_creation_input_tokens,
            }) => {
                let usage = usage.get_or_insert(crate::provider::types::ProviderUsage {
                    input_tokens: None,
                    output_tokens: None,
                    cache_read_input_tokens: None,
                    cache_creation_input_tokens: None,
                });
                usage.input_tokens =
                    Some(usage.input_tokens.unwrap_or(0).saturating_add(input_tokens));
                usage.output_tokens = Some(
                    usage
                        .output_tokens
                        .unwrap_or(0)
                        .saturating_add(output_tokens),
                );
                usage.cache_read_input_tokens = Some(
                    usage
                        .cache_read_input_tokens
                        .unwrap_or(0)
                        .saturating_add(cache_read_input_tokens),
                );
                usage.cache_creation_input_tokens = Some(
                    usage
                        .cache_creation_input_tokens
                        .unwrap_or(0)
                        .saturating_add(cache_creation_input_tokens),
                );
            }
            Ok(crate::provider::chunk::ProviderChunk::ServerError { code, message }) => {
                let summary = format!("{code}: {message}");
                request_evidence_recorder
                    .record_failed(crate::query::provider::request::failed(
                        request_scope,
                        request_started,
                        provider_request_id,
                        usage.as_ref(),
                        crate::provider::types::ProviderFailureKind::Unknown,
                        summary.clone(),
                    ))
                    .map_err(lifecycle_error)?;
                return Err(ToolResultEnvelope::error(
                    "failed: LLM stream error",
                    summary,
                ));
            }
            Ok(_) => {}
            Err(e) => {
                request_evidence_recorder
                    .record_failed(crate::query::provider::request::failed(
                        request_scope,
                        request_started,
                        e.provider_request_id.clone().or(provider_request_id),
                        usage.as_ref(),
                        e.kind,
                        e.message.clone(),
                    ))
                    .map_err(lifecycle_error)?;
                return Err(ToolResultEnvelope::error(
                    "failed: LLM stream error",
                    format!("stream error: {e:?}"),
                ));
            }
        }
    }

    request_evidence_recorder
        .record_completed(crate::query::provider::request::completed(
            request_scope,
            request_started,
            provider_request_id,
            usage.as_ref(),
        ))
        .map_err(lifecycle_error)?;

    Ok(response_text)
}

fn lifecycle_error(error: crate::error::Error) -> ToolResultEnvelope {
    ToolResultEnvelope::error(
        "failed: request evidence",
        format!("secondary request evidence error: {error}"),
    )
}

struct CacheEntry {
    content: String,
    inserted_at: Instant,
}

static URL_CACHE: LazyLock<Mutex<lru::LruCache<String, CacheEntry>>> = LazyLock::new(|| {
    Mutex::new(lru::LruCache::new(
        std::num::NonZeroUsize::new(CACHE_MAX_ENTRIES).unwrap(),
    ))
});

fn cache_get(url: &str) -> Option<String> {
    let mut cache = URL_CACHE.lock().ok()?;
    if let Some(entry) = cache.get(url) {
        if entry.inserted_at.elapsed() < CACHE_TTL {
            return Some(entry.content.clone());
        }
        cache.pop(url);
    }
    None
}

fn cache_put(url: &str, content: &str) {
    if let Ok(mut cache) = URL_CACHE.lock() {
        cache.put(
            url.to_string(),
            CacheEntry {
                content: content.to_string(),
                inserted_at: Instant::now(),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    fn parent_request() -> crate::event::RequestScope {
        crate::event::test_request_scope("fetch web parent")
    }

    fn test_context() -> (crate::config::Config, crate::prompt::PromptCatalog) {
        let catalog = crate::prompt::catalog::builtin_prompt_catalog();
        let toml_str = crate::config::generate_default();
        let file: crate::config::ConfigFile = toml::from_str(toml_str).unwrap();
        let config = file.resolve().unwrap();
        (config, catalog)
    }

    #[test]
    fn validate_model_tier_is_required() {
        let (config, catalog) = test_context();
        let args = serde_json::json!({
            "url": "https://example.com",
            "prompt": "summarize",
        });
        let result = tokio_test::block_on(fetch_web(
            &args,
            Path::new("."),
            &config,
            &catalog,
            &parent_request(),
            &crate::query::provider::LifecycleOnlyRecorder::new("events.jsonl"),
        ));
        assert_eq!(result.status, "error");
        assert!(result.model_content.contains("model_tier"));
    }

    #[test]
    fn validate_missing_required_params() {
        let (config, catalog) = test_context();
        let no_url = serde_json::json!({"prompt": "x", "model_tier": "light"});
        let r = tokio_test::block_on(fetch_web(
            &no_url,
            Path::new("."),
            &config,
            &catalog,
            &parent_request(),
            &crate::query::provider::LifecycleOnlyRecorder::new("events.jsonl"),
        ));
        assert_eq!(r.status, "error");

        let no_prompt = serde_json::json!({"url": "https://x.com", "model_tier": "light"});
        let r = tokio_test::block_on(fetch_web(
            &no_prompt,
            Path::new("."),
            &config,
            &catalog,
            &parent_request(),
            &crate::query::provider::LifecycleOnlyRecorder::new("events.jsonl"),
        ));
        assert_eq!(r.status, "error");

        let no_tier = serde_json::json!({"url": "https://x.com", "prompt": "x"});
        let r = tokio_test::block_on(fetch_web(
            &no_tier,
            Path::new("."),
            &config,
            &catalog,
            &parent_request(),
            &crate::query::provider::LifecycleOnlyRecorder::new("events.jsonl"),
        ));
        assert_eq!(r.status, "error");
    }

    #[test]
    fn cache_round_trip() {
        cache_put("https://test.com", "cached content");
        assert_eq!(
            cache_get("https://test.com"),
            Some("cached content".to_string())
        );
        assert_eq!(cache_get("https://other.com"), None);
    }

    #[test]
    fn html_to_markdown_converts_basic_html() {
        let html = "<html><body><h1>Title</h1><p>Paragraph</p></body></html>";
        let md = html_to_markdown("https://example.com", html).unwrap();
        assert!(
            md.contains("Paragraph"),
            "md should contain Paragraph, got: {md}"
        );
    }

    fn configure_mock_provider(config: &mut crate::config::Config, server: &MockServer) {
        let provider = config.providers.get_mut("anthropic").unwrap();
        provider.base_url = server.base_url();
        provider.credential = crate::config::StoredCredential::DirectValue(
            crate::config::SecretString::new("test-key"),
        );
    }

    fn secondary_response(text: &str) -> String {
        format!(
            "event: message_start\ndata: {}\n\n\
             event: content_block_start\ndata: {}\n\n\
             event: content_block_delta\ndata: {}\n\n\
             event: content_block_stop\ndata: {}\n\n\
             event: message_delta\ndata: {}\n\n\
             event: message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n",
            serde_json::json!({
                "type": "message_start",
                "message": {"id": "msg_secondary", "content": [], "usage": {"input_tokens": 7}}
            }),
            serde_json::json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "text", "text": ""}
            }),
            serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "text_delta", "text": text}
            }),
            serde_json::json!({"type": "content_block_stop", "index": 0}),
            serde_json::json!({
                "type": "message_delta",
                "delta": {"stop_reason": "end_turn"},
                "usage": {"output_tokens": 3}
            }),
        )
    }

    fn lifecycle_facts(events_path: &Path) -> Vec<crate::event::TaskEvent> {
        crate::event::EventStore::replay(events_path)
            .unwrap()
            .into_iter()
            .flat_map(|event| match event.payload {
                crate::event::EventPayload::TaskLedger(
                    crate::event::TaskLedgerRecord::Activity(batch),
                ) => batch.events().to_vec(),
                _ => Vec::new(),
            })
            .collect()
    }

    #[tokio::test]
    async fn secondary_provider_call_records_scoped_lifecycle() {
        let server = MockServer::start();
        let transport = server.mock(|when, then| {
            when.method(POST).path("/v1/messages");
            then.status(200).body(secondary_response("summary"));
        });
        let (mut config, catalog) = test_context();
        configure_mock_provider(&mut config, &server);
        let temp = tempfile::tempdir().unwrap();
        let events_path = temp.path().join("events.jsonl");
        let parent = parent_request();
        let recorder = crate::query::provider::LifecycleOnlyRecorder::new(&events_path);

        let result = call_secondary_llm(
            "long content",
            "summarize",
            "default",
            &config,
            &catalog,
            &parent,
            &recorder,
        )
        .await
        .unwrap();

        assert_eq!(result, "summary");
        transport.assert_hits(1);
        let facts = lifecycle_facts(&events_path);
        let started = facts
            .iter()
            .find_map(|event| match event {
                crate::event::TaskEvent::RequestStarted(value) => Some(value),
                _ => None,
            })
            .unwrap();
        let completed = facts
            .iter()
            .find_map(|event| match event {
                crate::event::TaskEvent::RequestCompleted(value) => Some(value),
                _ => None,
            })
            .unwrap();
        assert_eq!(started.scope, completed.scope);
        assert_eq!(started.scope.execution, parent.execution);
        assert_ne!(started.scope.request_id, parent.request_id);
        assert!(matches!(
            &started.cause,
            crate::event::RequestCause::ToolContinuation { parent_request_id }
                if parent_request_id == &parent.request_id
        ));
        assert_eq!(
            completed.provider_request_id.as_deref(),
            Some("msg_secondary")
        );
        assert_eq!(completed.usage.input_tokens, Some(7));
        assert_eq!(completed.usage.output_tokens, Some(3));
        assert_eq!(
            facts
                .iter()
                .filter(|event| matches!(event, crate::event::TaskEvent::RequestCompleted(_)))
                .count(),
            1
        );
        assert!(!facts
            .iter()
            .any(|event| matches!(event, crate::event::TaskEvent::RequestFailed(_))));
    }

    #[tokio::test]
    async fn secondary_evidence_append_failure_prevents_transport() {
        let server = MockServer::start();
        let transport = server.mock(|when, then| {
            when.method(POST).path("/v1/messages");
            then.status(200).body(secondary_response("unreachable"));
        });
        let (mut config, catalog) = test_context();
        configure_mock_provider(&mut config, &server);
        let temp = tempfile::tempdir().unwrap();
        let events_path = temp.path().join("events.jsonl");
        std::fs::create_dir(&events_path).unwrap();
        let recorder = crate::query::provider::LifecycleOnlyRecorder::new(&events_path);

        let result = call_secondary_llm(
            "long content",
            "summarize",
            "default",
            &config,
            &catalog,
            &parent_request(),
            &recorder,
        )
        .await;

        assert!(result.is_err());
        transport.assert_hits(0);
    }

    #[tokio::test]
    async fn secondary_stream_failure_records_one_failed_terminal() {
        let server = MockServer::start();
        let transport = server.mock(|when, then| {
            when.method(POST).path("/v1/messages");
            then.status(200).body(
                "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_failed\",\"content\":[],\"usage\":{\"input_tokens\":2}}}\n\n\
                 event: content_block_delta\ndata: {invalid json}\n\n",
            );
        });
        let (mut config, catalog) = test_context();
        configure_mock_provider(&mut config, &server);
        let temp = tempfile::tempdir().unwrap();
        let events_path = temp.path().join("events.jsonl");
        let recorder = crate::query::provider::LifecycleOnlyRecorder::new(&events_path);

        let result = call_secondary_llm(
            "long content",
            "summarize",
            "default",
            &config,
            &catalog,
            &parent_request(),
            &recorder,
        )
        .await;

        assert!(result.is_err());
        transport.assert_hits(1);
        let facts = lifecycle_facts(&events_path);
        let started_scope = facts.iter().find_map(|event| match event {
            crate::event::TaskEvent::RequestStarted(value) => Some(&value.scope),
            _ => None,
        });
        let failed_scope = facts.iter().find_map(|event| match event {
            crate::event::TaskEvent::RequestFailed(value) => Some(&value.scope),
            _ => None,
        });
        assert_eq!(started_scope, failed_scope);
        assert_eq!(
            facts
                .iter()
                .filter(|event| matches!(event, crate::event::TaskEvent::RequestFailed(_)))
                .count(),
            1
        );
        assert!(!facts
            .iter()
            .any(|event| matches!(event, crate::event::TaskEvent::RequestCompleted(_))));
    }
}
