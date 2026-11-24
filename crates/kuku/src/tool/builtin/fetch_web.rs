use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::tool::ToolResultEnvelope;

const SMALL_CONTENT_THRESHOLD: usize = 50_000;
const MAX_HTML_TAG_STARTS: usize = 100_000;
const MAX_HTML_BYTES: usize = 10 * 1024 * 1024;
const CACHE_MAX_ENTRIES: usize = 200;
const CACHE_TTL: Duration = Duration::from_secs(300);

pub(crate) async fn fetch_web(
    args: &Value,
    host: &crate::WorkspaceExecutionHost,
    config: &crate::config::Config,
    catalog: &crate::prompt::PromptCatalog,
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
        let structured = serde_json::json!({"kind": "fetch_web", "url": url, "cached": true});
        return if cached.truncated {
            ToolResultEnvelope::ok_truncated(
                format!("fetched (cached): {url}"),
                cached.content,
                structured,
            )
        } else {
            ToolResultEnvelope::ok(
                format!("fetched (cached): {url}"),
                cached.content,
                structured,
            )
        };
    }

    let permit = match host.operations().acquire_owned().await {
        Ok(permit) => permit,
        Err(error) => {
            return ToolResultEnvelope::error("web capacity unavailable", error.to_string())
        }
    };
    let html = match fetch_html(url).await {
        Ok(html) => html,
        Err(e) => return e,
    };

    let source_url = url.to_owned();
    let markdown = match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        html_to_markdown(&source_url, html)
    })
    .await
    {
        Ok(Ok(markdown)) => markdown,
        Ok(Err(result)) => return result,
        Err(error) => {
            return ToolResultEnvelope::error("HTML conversion failed", error.to_string())
        }
    };

    let result = if markdown.len() < SMALL_CONTENT_THRESHOLD {
        FetchedContent {
            content: markdown,
            truncated: false,
        }
    } else {
        match call_secondary_llm(&markdown, prompt, model_tier, config, catalog).await {
            Ok(summary) => summary,
            Err(_) => bounded_fallback_content(&markdown),
        }
    };

    cache_put(url, &result.content, result.truncated);

    let structured = serde_json::json!({
        "kind": "fetch_web",
        "url": url,
        "prompt": prompt,
        "model_tier": model_tier,
        "content_length": result.content.len(),
        "cached": false,
    });
    if result.truncated {
        ToolResultEnvelope::ok_truncated(format!("fetched {url}"), result.content, structured)
    } else {
        ToolResultEnvelope::ok(format!("fetched {url}"), result.content, structured)
    }
}

fn bounded_fallback_content(markdown: &str) -> FetchedContent {
    const HINT: &str = "[Content truncated — LLM summarization failed]";
    let mut content = String::new();
    let mut truncated = false;
    for (index, line) in markdown.lines().enumerate() {
        let separator = usize::from(index > 0);
        if content.len() + line.len() + separator > SMALL_CONTENT_THRESHOLD - HINT.len() - 1 {
            if !content.is_empty() {
                content.push('\n');
            }
            content.push_str(HINT);
            truncated = true;
            break;
        }
        if index > 0 {
            content.push('\n');
        }
        content.push_str(line);
    }
    FetchedContent { content, truncated }
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
            return Err(ToolResultEnvelope::error_with_reason(
                "failed: response too large",
                format!("HTML body exceeds {MAX_HTML_BYTES} byte limit"),
                crate::tool::ToolErrorReason::ResourceLimit,
            ));
        }
        buf.extend_from_slice(&chunk);
    }
    String::from_utf8(buf).map_err(|e| {
        ToolResultEnvelope::error("failed: encoding error", format!("invalid UTF-8: {e}"))
    })
}

fn html_to_markdown(url: &str, html: String) -> Result<String, ToolResultEnvelope> {
    if memchr::memchr_iter(b'<', html.as_bytes())
        .take(MAX_HTML_TAG_STARTS + 1)
        .count()
        > MAX_HTML_TAG_STARTS
    {
        return Err(ToolResultEnvelope::error_with_reason(
            "HTML conversion budget reached",
            "The page contains too much markup to convert. Use fetch_url to inspect the retained response.",
            crate::tool::ToolErrorReason::ResourceLimit,
        ));
    }
    let parsed_url = url::Url::parse(url).map_err(|e| {
        ToolResultEnvelope::error(
            "failed: invalid url",
            format!("failed to parse url for readability: {e}"),
        )
    })?;

    let clean_html = match readability::extractor::extract(&mut html.as_bytes(), &parsed_url) {
        Ok(product) => product.content,
        Err(_) => html,
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
) -> Result<FetchedContent, ToolResultEnvelope> {
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
    let end = content
        .char_indices()
        .nth(max_chars.saturating_sub(2_000))
        .map_or(content.len(), |(end, _)| end);
    let truncated = &content[..end];
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
        max_output_tokens: resolved.max_output_tokens,
        temperature: None,
        stream: true,
        think_level: resolved.think_level,
        thinking: resolved.thinking.clone(),
    };

    let mut stream = crate::provider::stream_provider(&resolved, &request, None, None)
        .await
        .map_err(|e| {
            ToolResultEnvelope::error("failed: LLM call", format!("secondary LLM error: {e:?}"))
        })?;

    let mut response_text = String::new();
    let mut truncated = false;
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(crate::provider::chunk::ProviderChunk::TextDelta { text }) => {
                let remaining = SMALL_CONTENT_THRESHOLD - response_text.len();
                if text.len() > remaining {
                    response_text.push_str(&text[..text.floor_char_boundary(remaining)]);
                    truncated = true;
                    break;
                }
                response_text.push_str(&text);
            }
            Ok(_) => {}
            Err(e) => {
                return Err(ToolResultEnvelope::error(
                    "failed: LLM stream error",
                    format!("stream error: {e:?}"),
                ));
            }
        }
    }

    Ok(FetchedContent {
        content: response_text,
        truncated,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FetchedContent {
    content: String,
    truncated: bool,
}

struct CacheEntry {
    result: FetchedContent,
    inserted_at: Instant,
}

static URL_CACHE: LazyLock<Mutex<lru::LruCache<String, CacheEntry>>> = LazyLock::new(|| {
    Mutex::new(lru::LruCache::new(
        std::num::NonZeroUsize::new(CACHE_MAX_ENTRIES).unwrap(),
    ))
});

fn cache_get(url: &str) -> Option<FetchedContent> {
    let mut cache = URL_CACHE.lock().ok()?;
    if let Some(entry) = cache.get(url) {
        if entry.inserted_at.elapsed() < CACHE_TTL {
            return Some(entry.result.clone());
        }
        cache.pop(url);
    }
    None
}

fn cache_put(url: &str, content: &str, truncated: bool) {
    if let Ok(mut cache) = URL_CACHE.lock() {
        cache.put(
            url.to_string(),
            CacheEntry {
                result: FetchedContent {
                    content: content.to_string(),
                    truncated,
                },
                inserted_at: Instant::now(),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            &crate::WorkspaceExecutionHost::default(),
            &config,
            &catalog,
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
            &crate::WorkspaceExecutionHost::default(),
            &config,
            &catalog,
        ));
        assert_eq!(r.status, "error");

        let no_prompt = serde_json::json!({"url": "https://x.com", "model_tier": "light"});
        let r = tokio_test::block_on(fetch_web(
            &no_prompt,
            &crate::WorkspaceExecutionHost::default(),
            &config,
            &catalog,
        ));
        assert_eq!(r.status, "error");

        let no_tier = serde_json::json!({"url": "https://x.com", "prompt": "x"});
        let r = tokio_test::block_on(fetch_web(
            &no_tier,
            &crate::WorkspaceExecutionHost::default(),
            &config,
            &catalog,
        ));
        assert_eq!(r.status, "error");
    }

    #[test]
    fn cache_round_trip() {
        cache_put("https://test.com", "cached content", true);
        let cached = cache_get("https://test.com").unwrap();
        assert_eq!(cached.content, "cached content");
        assert!(cached.truncated);
        assert_eq!(cache_get("https://other.com"), None);
    }

    #[test]
    fn cached_fetch_preserves_truncation() {
        let (config, catalog) = test_context();
        let url = "https://cached-truncated.example.com";
        cache_put(url, "cached truncated content", true);
        let args = serde_json::json!({
            "url": url,
            "prompt": "summarize",
            "model_tier": "light",
        });

        let result = tokio_test::block_on(fetch_web(
            &args,
            &crate::WorkspaceExecutionHost::default(),
            &config,
            &catalog,
        ));

        assert_eq!(result.status, "ok");
        assert!(result.truncated);
        assert_eq!(result.model_content, "cached truncated content");
    }

    #[test]
    fn fallback_content_reports_bounded_truncation() {
        let markdown = "x".repeat(SMALL_CONTENT_THRESHOLD + 100);

        let fallback = bounded_fallback_content(&markdown);

        assert!(fallback.truncated);
        assert!(fallback.content.len() <= SMALL_CONTENT_THRESHOLD);
    }

    #[test]
    fn html_to_markdown_converts_basic_html() {
        let html = "<html><body><h1>Title</h1><p>Paragraph</p></body></html>";
        let md = html_to_markdown("https://example.com", html.to_owned()).unwrap();
        assert!(
            md.contains("Paragraph"),
            "md should contain Paragraph, got: {md}"
        );
    }

    #[test]
    fn dense_markup_reaches_the_conversion_budget_before_html_parsing() {
        let html = "<x>".repeat(MAX_HTML_TAG_STARTS + 1);
        let error = html_to_markdown("https://example.com", html).unwrap_err();
        assert_eq!(error.structured.unwrap()["reason_code"], "resource_limit");
    }
}
