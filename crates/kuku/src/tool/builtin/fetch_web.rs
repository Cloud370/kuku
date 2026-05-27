use std::path::Path;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::tool::ToolResultEnvelope;

const SMALL_CONTENT_THRESHOLD: usize = 50_000;
const CACHE_MAX_ENTRIES: usize = 200;
const CACHE_TTL: Duration = Duration::from_secs(300);

pub(crate) async fn fetch_web(
    args: &Value,
    _workspace: &Path,
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
        match call_secondary_llm(&markdown, prompt, model_tier, config, catalog).await {
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

    response.text().await.map_err(|e| {
        ToolResultEnvelope::error("failed: read error", format!("failed to read body: {e}"))
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
        system_prompt: catalog.fetch_web.text.clone(),
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
    };
    let request = crate::provider::types::ProviderRequest {
        assembly,
        model: resolved.model.clone(),
        max_output_tokens: Some(resolved.max_output_tokens),
        temperature: None,
        stream: true,
        think_level: resolved.think_level.as_str().to_string(),
        thinking: resolved.thinking.clone(),
    };

    let mut stream = crate::provider::stream_provider(&resolved, &request)
        .await
        .map_err(|e| {
            ToolResultEnvelope::error("failed: LLM call", format!("secondary LLM error: {e:?}"))
        })?;

    let mut response_text = String::new();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(crate::provider::chunk::ProviderChunk::TextDelta { text }) => {
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

    Ok(response_text)
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
        let result = tokio_test::block_on(fetch_web(&args, Path::new("."), &config, &catalog));
        assert_eq!(result.status, "error");
        assert!(result.model_content.contains("model_tier"));
    }

    #[test]
    fn validate_missing_required_params() {
        let (config, catalog) = test_context();
        let no_url = serde_json::json!({"prompt": "x", "model_tier": "light"});
        let r = tokio_test::block_on(fetch_web(&no_url, Path::new("."), &config, &catalog));
        assert_eq!(r.status, "error");

        let no_prompt = serde_json::json!({"url": "https://x.com", "model_tier": "light"});
        let r = tokio_test::block_on(fetch_web(&no_prompt, Path::new("."), &config, &catalog));
        assert_eq!(r.status, "error");

        let no_tier = serde_json::json!({"url": "https://x.com", "prompt": "x"});
        let r = tokio_test::block_on(fetch_web(&no_tier, Path::new("."), &config, &catalog));
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
}
