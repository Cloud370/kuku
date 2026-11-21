use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_stream::StreamExt;
use url::Url;

use crate::config::{resolve_request_url, OpenRouterRouting, ProviderConfig, TokenRates};
use crate::{Error, Result};

use super::capabilities::normalized_tier;

/// An advertised quote for one OpenRouter route. Rates are in USD per million tokens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteQuote {
    pub tag: String,
    pub provider: String,
    pub author_route: bool,
    pub service_tier: Option<String>,
    pub rates: TokenRates,
}

fn same_endpoint(tag: &str, endpoint: &str) -> bool {
    tag == endpoint
        || match (tag.rsplit_once('/'), endpoint.rsplit_once('/')) {
            (Some((tag_base, "fast")), Some((endpoint_base, "priority")))
            | (Some((tag_base, "priority")), Some((endpoint_base, "fast"))) => {
                tag_base == endpoint_base
            }
            _ => false,
        }
}

fn endpoint_matches(tag: &str, endpoint: &str) -> bool {
    same_endpoint(tag, endpoint)
        || tag
            .strip_prefix(endpoint)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn allowed_by_only(quote: &RouteQuote, routing: &OpenRouterRouting) -> bool {
    routing.only.is_empty()
        || routing
            .only
            .iter()
            .any(|endpoint| endpoint_matches(&quote.tag, endpoint))
}

pub(super) fn service_tiers(
    quotes: &[RouteQuote],
    routing: Option<&OpenRouterRouting>,
) -> Option<Vec<String>> {
    if quotes.is_empty() {
        return None;
    }
    let mut values: Vec<_> = quotes
        .iter()
        .filter(|quote| {
            routing.is_none_or(|routing| {
                allowed_by_only(quote, routing)
                    && (routing.allow_fallbacks != Some(false)
                        || routing
                            .order
                            .first()
                            .is_none_or(|tag| endpoint_matches(&quote.tag, tag)))
            })
        })
        .map(|quote| normalized_tier(quote.service_tier.as_deref()))
        .map(str::to_owned)
        .collect();
    values.sort_unstable();
    values.dedup();
    Some(values)
}

/// Pick a quote to display, honoring a preferred endpoint and requested service tier.
/// The returned quote describes its own route even when runtime fallback is enabled.
pub fn route_quote_for_display<'a>(
    quotes: &'a [RouteQuote],
    routing: Option<&OpenRouterRouting>,
    service_tier: Option<&str>,
) -> Option<&'a RouteQuote> {
    let allowed =
        |quote: &&RouteQuote| routing.is_none_or(|routing| allowed_by_only(quote, routing));
    let eligible = |quote: &&RouteQuote| {
        normalized_tier(quote.service_tier.as_deref()) == normalized_tier(service_tier)
    };
    let quotes = quotes.iter().filter(allowed);
    if let Some(preferred) =
        routing.and_then(|routing| routing.order.first().or_else(|| routing.only.first()))
    {
        if service_tier.is_none() && preferred.contains('/') {
            if let Some(exact) = quotes
                .clone()
                .find(|quote| same_endpoint(&quote.tag, preferred))
            {
                return Some(exact);
            }
        }
        return quotes
            .clone()
            .filter(eligible)
            .find(|quote| quote.tag == *preferred)
            .or_else(|| {
                quotes
                    .filter(eligible)
                    .find(|quote| endpoint_matches(&quote.tag, preferred))
            });
    }
    quotes.filter(eligible).find(|quote| quote.author_route)
}

/// Resolve OpenRouter's public rate endpoint without using the connection credential.
pub fn openrouter_quote_url(connection: &ProviderConfig, model: &str) -> Option<String> {
    let url = resolve_request_url(
        connection.format,
        &connection.base_url,
        connection.options.endpoint_url.as_deref(),
    )
    .ok()?;
    let mut url = Url::parse(&url).ok()?;
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some("openrouter.ai" | "us.openrouter.ai" | "eu.openrouter.ai")
        )
        || url.port_or_known_default() != Some(443)
    {
        return None;
    }
    let (author, slug) = model.split_once('/')?;
    if [author, slug].iter().any(|part| {
        part.is_empty()
            || matches!(*part, "." | "..")
            || !part.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b':')
            })
    }) {
        return None;
    }
    url.set_query(None);
    url.set_path("");
    url.path_segments_mut().ok()?.clear().extend([
        "api",
        "v1",
        "models",
        author,
        slug,
        "endpoints",
    ]);
    Some(url.into())
}

/// Read the full-route price quotes for an exact model on an OpenRouter connection.
pub async fn openrouter_quotes(
    connection: &ProviderConfig,
    model: &str,
) -> Result<Vec<RouteQuote>> {
    let url = openrouter_quote_url(connection, model)
        .ok_or_else(|| quote_error("a known OpenRouter endpoint and model are required"))?;
    let response = super::http_client::api_client(&connection.options.proxy)?
        .get(url)
        .redirect(wreq::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|error| {
            quote_error(if error.is_timeout() {
                "request timed out"
            } else {
                "connection failed"
            })
        })?;
    if !response.status().is_success() {
        return Err(quote_error(&format!("HTTP {}", response.status().as_u16())));
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| quote_error("could not read route prices"))?;
        if body.len().saturating_add(chunk.len()) > 2 * 1024 * 1024 {
            return Err(quote_error("route prices exceed the response limit"));
        }
        body.extend_from_slice(&chunk);
    }
    let value: Value =
        serde_json::from_slice(&body).map_err(|_| quote_error("invalid price data"))?;
    parse_quotes(&value, model)
}

fn parse_quotes(value: &Value, model: &str) -> Result<Vec<RouteQuote>> {
    let data = &value["data"];
    if data["id"].as_str() != Some(model) {
        return Err(quote_error(
            "the catalog did not confirm the exact model ID",
        ));
    }
    let author = model.split_once('/').map_or("", |(author, _)| author);
    let rows = data["endpoints"]
        .as_array()
        .ok_or_else(|| quote_error("missing endpoints"))?;
    Ok(rows
        .iter()
        .filter_map(|endpoint| {
            if endpoint["status"]
                .as_i64()
                .is_some_and(|status| status != 0)
            {
                return None;
            }
            let tag = endpoint["tag"].as_str()?;
            let service_tier = endpoint["service_tier"].as_str().map(str::to_owned);
            let provider = endpoint["provider_name"].as_str()?;
            let rates = &endpoint["pricing"];
            let rate = |name: &str| {
                let field = &rates[name];
                let value = field
                    .as_f64()
                    .or_else(|| field.as_str()?.parse::<f64>().ok())?
                    * 1_000_000.0;
                (value.is_finite() && value >= 0.0).then_some(value)
            };
            let base = tag.split('/').next()?;
            let author_route = match author {
                "deepseek" | "anthropic" | "openai" => author == base,
                "google" => matches!(base, "google-ai-studio" | "google-vertex"),
                _ => false,
            };
            Some(RouteQuote {
                tag: tag.into(),
                provider: provider.into(),
                author_route,
                service_tier,
                rates: TokenRates {
                    input: rate("prompt"),
                    output: rate("completion"),
                    cache_read: rate("input_cache_read"),
                    cache_write: rate("input_cache_write"),
                    cache_write_5m: None,
                    cache_write_1h: rate("input_cache_write_1h"),
                },
            })
        })
        .collect())
}

fn quote_error(detail: &str) -> Error {
    Error::InvalidArgument(format!("OpenRouter price discovery: {detail}"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::config::OpenRouterRouting;

    use super::{parse_quotes, route_quote_for_display, service_tiers};

    #[test]
    fn author_matching_keeps_full_service_tiers_and_write_rates() {
        let anthropic = json!({"data": {"id": "anthropic/claude-opus-5.5", "endpoints": [
            {"tag":"anthropic","provider_name":"Anthropic","status":0,"pricing":{
                "prompt":"0.000004","completion":"0.00002","input_cache_write":"0.000005","input_cache_write_1h":"0.000008"
            }},
            {"tag":"anthropic/fast","provider_name":"Anthropic","status":0,"service_tier":"priority","pricing":{
                "prompt":"0.000008","completion":"0.00004"
            }},
            {"tag":"google-vertex/global","provider_name":"Google","status":0,"pricing":{"prompt":"0.000004"}}
        ]}});
        let results = parse_quotes(&anthropic, "anthropic/claude-opus-5.5").unwrap();
        assert!(results[0].author_route && results[1].author_route && !results[2].author_route);
        assert_eq!(Some(4.0), results[0].rates.input);
        assert_eq!(
            route_quote_for_display(&results, None, None),
            route_quote_for_display(&results, None, Some("default")),
        );
        assert_eq!(Some(8.0), results[0].rates.cache_write_1h);
        assert_eq!("anthropic/fast", results[1].tag);
        assert_eq!(Some(8.0), results[1].rates.input);
        assert_eq!(Some("priority"), results[1].service_tier.as_deref());
        assert_eq!(
            "anthropic/fast",
            route_quote_for_display(&results, None, Some("priority"))
                .unwrap()
                .tag
        );
        let pinned = OpenRouterRouting {
            only: vec!["anthropic/priority".into()],
            ..OpenRouterRouting::default()
        };
        assert_eq!(
            Some(vec!["default".into(), "priority".into()]),
            service_tiers(&results, None)
        );
        assert_eq!(
            Some(vec!["priority".into()]),
            service_tiers(&results, Some(&pinned))
        );
        assert_eq!(
            "anthropic/fast",
            route_quote_for_display(&results, Some(&pinned), None)
                .unwrap()
                .tag
        );
        for tier in ["priority", "fast"] {
            assert_eq!(
                "anthropic/fast",
                route_quote_for_display(&results, Some(&pinned), Some(tier))
                    .unwrap()
                    .tag
            );
        }
        let google = json!({"data": {"id": "google/gemini-4-neon", "endpoints": [
            {"tag":"google-vertex/global","provider_name":"Google","pricing":{"prompt":"0.000003"}},
            {"tag":"google-ai-studio/flex","provider_name":"Google AI Studio","service_tier":"flex","pricing":{"prompt":"0.0000015"}}
        ]}});
        let results = parse_quotes(&google, "google/gemini-4-neon").unwrap();
        assert!(results.iter().all(|route| route.author_route));
        assert_eq!(Some(1.5), results[1].rates.input);
        assert_eq!(
            Some(vec!["default".into(), "flex".into()]),
            service_tiers(&results, None)
        );
        assert_eq!(None, service_tiers(&[], None));
    }
}
