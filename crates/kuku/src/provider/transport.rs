use serde_json::json;
use wreq::header::{HeaderMap, HeaderValue};

use crate::config::ProviderAuth;

use super::error::{classify_http_error, transport_error};
use super::prepare::{authentication, invalid_request};
use super::trace::{ProviderTrace, ProviderTraceDirection, ProviderTraceMetadata};
use super::types::{PreparedProviderRequest, ProviderFailure, ProviderKind, ResolvedProvider};

const MAX_ERROR_BODY_BYTES: usize = 4096;

pub(crate) struct TransportResponse {
    pub(crate) response: wreq::Response,
    pub(crate) trace: Option<ProviderTrace>,
}

pub(crate) async fn send_prepared(
    config: &ResolvedProvider,
    prepared: &PreparedProviderRequest,
    trace_metadata: Option<ProviderTraceMetadata>,
) -> Result<TransportResponse, ProviderFailure> {
    let expected_url = match config.kind {
        ProviderKind::Anthropic => super::anthropic::messages_url(&config.base_url),
        ProviderKind::OpenAiCompatible => {
            super::openai_compat::chat_completions_url(&config.base_url)
        }
        ProviderKind::OpenAiResponses => super::openai_responses::responses_url(&config.base_url),
    };
    if config.connection_name != prepared.connection_name
        || config.kind != prepared.kind
        || config.model != prepared.model
        || expected_url != prepared.url
    {
        return Err(invalid_request(
            "prepared request belongs to a different connection, endpoint, protocol, or model",
        ));
    }
    let headers = request_headers(config, prepared)?;
    let trace = ProviderTrace::from_request(
        trace_metadata.as_ref(),
        config.connection_name.clone(),
        prepared.model.clone(),
    );
    if let Some(trace) = &trace {
        trace.record(
            ProviderTraceDirection::Request,
            Some(&prepared.url),
            Some(&headers),
            json!({"body": prepared.body}),
        );
    }
    let response = super::http_client::api_client()
        .post(&prepared.url)
        .headers(headers)
        .json(&prepared.body)
        .send()
        .await
        .map_err(|error| {
            if let Some(trace) = &trace {
                trace.record(
                    ProviderTraceDirection::Error,
                    Some(&prepared.url),
                    None,
                    json!({"error": "provider transport failed"}),
                );
            }
            transport_error(&error)
        })?;
    let status = response.status();
    if let Some(trace) = &trace {
        trace.record(
            ProviderTraceDirection::Response,
            Some(&prepared.url),
            Some(response.headers()),
            json!({"status": status.as_u16()}),
        );
    }
    if !status.is_success() {
        let request_id = response
            .headers()
            .get("x-request-id")
            .or_else(|| response.headers().get("request-id"))
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body = read_error_body(response).await;
        let body = if config.api_key.expose().is_empty() {
            body
        } else {
            body.replace(config.api_key.expose(), "<redacted>")
        };
        let mut failure = classify_http_error(status.as_u16(), &body);
        failure.provider_request_id = request_id;
        return Err(failure);
    }
    Ok(TransportResponse { response, trace })
}

fn request_headers(
    config: &ResolvedProvider,
    prepared: &PreparedProviderRequest,
) -> Result<HeaderMap, ProviderFailure> {
    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    let (name, value) = match authentication(config) {
        ProviderAuth::ApiKey => ("x-api-key", config.api_key.expose().to_string()),
        ProviderAuth::Bearer => (
            "authorization",
            format!("Bearer {}", config.api_key.expose()),
        ),
    };
    let mut credential = HeaderValue::from_str(&value)
        .map_err(|_| invalid_request("credential is not a valid HTTP header value"))?;
    credential.set_sensitive(true);
    headers.insert(name, credential);
    if prepared.kind == ProviderKind::Anthropic {
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
    }
    if let Some(correlation) = &prepared.correlation_id {
        headers.insert(
            "x-session-id",
            HeaderValue::from_str(correlation).map_err(|_| {
                invalid_request("transport correlation is not a valid HTTP header value")
            })?,
        );
    }
    Ok(headers)
}

async fn read_error_body(response: wreq::Response) -> String {
    use tokio_stream::StreamExt;

    let read = async move {
        let mut stream = response.bytes_stream();
        let mut body = Vec::new();
        while let Some(Ok(bytes)) = stream.next().await {
            let remaining = MAX_ERROR_BODY_BYTES.saturating_sub(body.len());
            body.extend_from_slice(&bytes[..bytes.len().min(remaining)]);
            if body.len() == MAX_ERROR_BODY_BYTES {
                break;
            }
        }
        String::from_utf8_lossy(&body).into_owned()
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), read)
        .await
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderProfile;
    use crate::provider::types::SecretString;

    fn prepared(config: &ResolvedProvider) -> PreparedProviderRequest {
        PreparedProviderRequest {
            kind: config.kind,
            connection_name: config.connection_name.clone(),
            model: config.model.clone(),
            url: "http://localhost/v1/messages".into(),
            body: json!({}),
            settings: Default::default(),
            correlation_id: None,
        }
    }

    #[test]
    fn messages_authentication_is_independent_of_the_wire_protocol() {
        let mut config = ResolvedProvider {
            kind: ProviderKind::Anthropic,
            api_key: SecretString::new("test-key"),
            ..Default::default()
        };
        config.capabilities.profile = ProviderProfile::OpenRouter;
        let headers = request_headers(&config, &prepared(&config)).unwrap();
        assert_eq!("Bearer test-key", headers["authorization"]);
        assert!(headers.get("x-api-key").is_none());
        config.options.auth = Some(ProviderAuth::ApiKey);
        let headers = request_headers(&config, &prepared(&config)).unwrap();
        assert_eq!("test-key", headers["x-api-key"]);
        assert!(headers.get("authorization").is_none());
    }

    #[test]
    fn invalid_credential_headers_fail_before_dispatch() {
        let config = ResolvedProvider {
            api_key: SecretString::new("invalid\nkey"),
            ..Default::default()
        };
        let failure = request_headers(&config, &prepared(&config)).unwrap_err();
        assert!(!failure.retryable);
        assert!(!failure.message.contains("key"));
    }
}
