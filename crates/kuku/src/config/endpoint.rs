use url::Url;

use crate::{Error, Result};

use super::{ProviderFormat, ProviderOptions};

/// Resolve the exact request destination for a Base URL or an explicit endpoint.
pub fn resolve_request_url(
    format: ProviderFormat,
    base_url: &str,
    endpoint_url: Option<&str>,
) -> Result<String> {
    if let Some(endpoint) = endpoint_url {
        let endpoint = endpoint.trim();
        http_url(endpoint)?;
        return Ok(endpoint.to_owned());
    }
    let base = base_url.trim();
    let base = if base.is_empty() {
        format.default_base_url()
    } else {
        base
    };
    let url = http_url(base)?;
    if url.query().is_some() {
        return Err(Error::ConfigLoad(
            "Base URL cannot contain a query. Use full request URL mode.".into(),
        ));
    }
    let base = base.trim_end_matches('/');
    let suffix = match format {
        ProviderFormat::OpenAiChat => "/chat/completions",
        ProviderFormat::OpenAiResponses => "/responses",
        ProviderFormat::Anthropic if base.ends_with("/v1") => "/messages",
        ProviderFormat::Anthropic => "/v1/messages",
    };
    Ok(format!("{base}{suffix}"))
}

/// Resolve the model-list destination using an explicit URL or the protocol's standard path.
pub fn resolve_models_url(
    format: ProviderFormat,
    base_url: &str,
    options: &ProviderOptions,
) -> Result<String> {
    if let Some(url) = options.models_url.as_deref() {
        let url = url.trim();
        http_url(url)?;
        return Ok(url.to_owned());
    }
    let request = resolve_request_url(format, base_url, options.endpoint_url.as_deref())?;
    let mut url = http_url(&request)?;
    if url.host_str() == Some("api.deepseek.com") {
        return Ok("https://api.deepseek.com/models".into());
    }
    let path = url.path().trim_end_matches('/');
    let prefix = path.strip_suffix(format.request_suffix()).ok_or_else(|| {
        Error::ConfigLoad("Set a models URL to discover models for this request endpoint.".into())
    })?;
    let models_path = format!("{prefix}/models");
    url.set_path(&models_path);
    url.set_query(None);
    Ok(url.into())
}

fn http_url(value: &str) -> Result<Url> {
    Url::parse(value)
        .ok()
        .filter(|url| {
            matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.fragment().is_none()
        })
        .ok_or_else(|| {
            Error::ConfigLoad(
                "Use an absolute HTTP or HTTPS URL without credentials or a fragment.".into(),
            )
        })
}
