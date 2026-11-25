use wreq::header::{HeaderMap, HeaderValue};

use crate::config::ProviderAuth;

use super::error::{classify_http_error, transport_error};
use super::media::{self, ImageWire};
use super::prepare::{authentication, invalid_request};
use super::trace::{ProviderTrace, ProviderTraceMetadata};
use super::types::{
    PreparedProviderRequest, ProviderFailure, ProviderFailureKind, ProviderKind, ResolvedProvider,
};

const MAX_ERROR_BODY_BYTES: usize = 4096;

pub(crate) struct TransportResponse {
    pub(crate) response: wreq::Response,
    pub(crate) trace: Option<ProviderTrace>,
    pub(super) contains_images: bool,
}

pub(crate) async fn send_prepared(
    config: &ResolvedProvider,
    prepared: &PreparedProviderRequest,
    trace_metadata: Option<ProviderTraceMetadata>,
) -> Result<TransportResponse, ProviderFailure> {
    send(config, prepared, trace_metadata, None).await
}

pub(super) async fn send_trial(
    config: &ResolvedProvider,
    prepared: &PreparedProviderRequest,
    error_body: &mut String,
) -> Result<TransportResponse, ProviderFailure> {
    send(config, prepared, None, Some(error_body)).await
}

async fn send(
    config: &ResolvedProvider,
    prepared: &PreparedProviderRequest,
    trace_metadata: Option<ProviderTraceMetadata>,
    trial_error_body: Option<&mut String>,
) -> Result<TransportResponse, ProviderFailure> {
    let expected_url = super::prepare::request_url(config)?;
    if config.connection_name != prepared.connection_name
        || config.kind != prepared.kind
        || config.model != prepared.model
        || expected_url != prepared.url
    {
        return Err(invalid_request(
            "prepared request belongs to a different connection, endpoint, protocol, or model",
        ));
    }
    let subscription = config.options.subscription.as_ref();
    let credential = match subscription {
        Some(account) => {
            Box::pin(super::subscription::authorization_token(
                account,
                &config.options.proxy,
            ))
            .await
        }
        None => config.api_key.resolve(),
    };
    let api_key = credential.map_err(|error| ProviderFailure {
        kind: ProviderFailureKind::Authentication,
        message: format!("could not resolve connection credentials: {error}"),
        status: None,
        provider_request_id: None,
        retry_after: None,
        retryable: false,
    })?;
    let headers = request_headers(config, prepared, &api_key)?;
    let trace = ProviderTrace::from_request(
        trace_metadata.filter(|_| subscription.is_none()),
        config.connection_name.clone(),
        prepared.model.clone(),
    );
    let wire = ImageWire::for_kind(prepared.kind);
    let contains_images = media::has_inline_images(wire, prepared.body.as_json());
    if let Some(trace) = &trace {
        let body = trace.full().then(|| {
            let mut body = prepared.body.as_json().clone();
            for key in &prepared.fields.private_body_keys {
                if let Some(field) = body.get_mut(key) {
                    *field = serde_json::json!("<redacted>");
                }
            }
            media::redact_images(wire, &mut body);
            body
        });
        trace.request(&prepared.url, &headers, body);
    }
    let client = super::http_client::api_client(&config.options.proxy).map_err(|error| {
        let failure = invalid_request(&error.to_string());
        if let Some(trace) = &trace {
            trace.failure(failure.kind);
        }
        failure
    })?;
    let mut request = client
        .post(&prepared.url)
        .headers(headers)
        .body(prepared.body.as_bytes().clone());
    if trial_error_body.is_some()
        || subscription.is_some()
        || !prepared.fields.headers.is_empty()
        || !prepared.fields.private_body_keys.is_empty()
    {
        request = request.redirect(wreq::redirect::Policy::none());
    }
    let response = request.send().await.map_err(|error| {
        let failure = transport_error(&error);
        if let Some(trace) = &trace {
            trace.failure(failure.kind);
        }
        failure
    })?;
    let status = response.status();
    if let Some(trace) = &trace {
        trace.response(&prepared.url, status.as_u16(), response.headers());
    }
    if !status.is_success() {
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| super::error::retry_after(value, std::time::SystemTime::now()));
        let request_id = response
            .headers()
            .get("x-request-id")
            .or_else(|| response.headers().get("request-id"))
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body = read_error_body(response).await;
        let mut body = if api_key.is_empty() {
            body
        } else {
            body.replace(&api_key, "<redacted>")
        };
        for value in prepared
            .fields
            .headers
            .values()
            .filter(|value| value.is_sensitive())
        {
            if let Ok(value) = value.to_str() {
                if !value.is_empty() {
                    body = body.replace(value, "<redacted>");
                }
            }
        }
        let mut failure = classify_http_error(status.as_u16(), &body);
        if contains_images {
            super::error::omit_image_request_error_details(&mut failure);
        }
        failure.retry_after = retry_after;
        if let Some(delay) = retry_after {
            failure
                .message
                .push_str(&format!(" (Retry-After: {}s)", delay.as_secs_f64().ceil()));
        }
        failure.provider_request_id = request_id;
        if let Some(trace) = &trace {
            trace.failure(failure.kind);
        }
        if let Some(capture) = trial_error_body {
            *capture = body;
        }
        return Err(failure);
    }
    Ok(TransportResponse {
        response,
        trace,
        contains_images,
    })
}

fn request_headers(
    config: &ResolvedProvider,
    prepared: &PreparedProviderRequest,
    api_key: &str,
) -> Result<HeaderMap, ProviderFailure> {
    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    let (name, value) = match authentication(config) {
        ProviderAuth::ApiKey => ("x-api-key", api_key.to_string()),
        ProviderAuth::Bearer => ("authorization", format!("Bearer {api_key}")),
    };
    let mut credential = HeaderValue::from_str(&value)
        .map_err(|_| invalid_request("credential is not a valid HTTP header value"))?;
    credential.set_sensitive(true);
    headers.insert(name, credential);
    if prepared.kind == ProviderKind::Anthropic {
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
    }
    headers.extend(prepared.fields.headers.clone());
    if !prepared.fields.request_id_headers.is_empty() {
        let id = uuid::Uuid::new_v4().to_string();
        let value = HeaderValue::from_str(&id).expect("UUID fits an HTTP header");
        for name in &prepared.fields.request_id_headers {
            headers.insert(name, value.clone());
        }
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
    use serde_json::json;

    use super::*;
    use crate::config::ProviderProfile;
    use crate::provider::types::{PreparedBody, SecretString};

    fn prepared(config: &ResolvedProvider) -> PreparedProviderRequest {
        PreparedProviderRequest {
            kind: config.kind,
            connection_name: config.connection_name.clone(),
            model: config.model.clone(),
            url: "http://localhost/v1/messages".into(),
            body: PreparedBody::from_json(json!({})),
            settings: Default::default(),
            correlation_id: None,
            fields: Default::default(),
        }
    }

    fn with_preset(config: &ResolvedProvider, request: &mut PreparedProviderRequest) {
        request.fields = super::super::request_fields::prepare(
            config,
            &request.url,
            &mut json!({}),
            super::super::request_fields::RequestPurpose::Inference,
            request.correlation_id.as_deref(),
            || None,
        )
        .unwrap();
    }

    #[test]
    fn messages_authentication_is_independent_of_the_wire_protocol() {
        let mut config = ResolvedProvider {
            kind: ProviderKind::Anthropic,
            api_key: SecretString::new("test-key"),
            ..Default::default()
        };
        config.capabilities.profile = ProviderProfile::OpenRouter;
        let headers = request_headers(
            &config,
            &prepared(&config),
            &config.api_key.resolve().unwrap(),
        )
        .unwrap();
        assert_eq!("Bearer test-key", headers["authorization"]);
        assert!(headers.get("x-api-key").is_none());
        config.options.auth = Some(ProviderAuth::ApiKey);
        let headers = request_headers(
            &config,
            &prepared(&config),
            &config.api_key.resolve().unwrap(),
        )
        .unwrap();
        assert_eq!("test-key", headers["x-api-key"]);
        assert!(headers.get("authorization").is_none());
    }

    #[test]
    fn billing_metadata_is_requested_on_official_openrouter_accounting_routes() {
        let mut config = ResolvedProvider::default();
        config.capabilities.profile = ProviderProfile::OpenRouter;
        let mut request = prepared(&config);
        with_preset(&config, &mut request);
        assert!(
            !request_headers(&config, &request, "test-key")
                .unwrap()
                .contains_key("x-openrouter-metadata")
        );
        request.url = "https://eu.openrouter.ai/api/v1/messages".into();
        with_preset(&config, &mut request);
        assert_eq!(
            "enabled",
            request_headers(&config, &request, "test-key").unwrap()["x-openrouter-metadata"]
        );
        config
            .options
            .model_pricing
            .insert(config.model.clone(), crate::config::ModelPricing::UsageOnly);
        with_preset(&config, &mut request);
        assert!(
            !request_headers(&config, &request, "test-key")
                .unwrap()
                .contains_key("x-openrouter-metadata")
        );
    }

    #[test]
    fn chatgpt_uses_one_stable_session_header_and_a_per_dispatch_request_id() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = ResolvedProvider::default();
        config.options.subscription = Some(
            crate::config::SubscriptionAccount::new(
                directory.path(),
                crate::config::SubscriptionChannel::ChatGpt,
                "fixture",
            )
            .unwrap(),
        );
        let mut request = prepared(&config);
        request.correlation_id = Some("kuku-anonymous-id".into());
        with_preset(&config, &mut request);
        let first = request_headers(&config, &request, "fixture-token").unwrap();
        let retry = request_headers(&config, &request, "fixture-token").unwrap();
        assert_eq!(first["session-id"], "kuku-anonymous-id");
        assert_eq!(first["session-id"], retry["session-id"]);
        assert!(!first.contains_key("x-session-id"));
        assert!(!first.contains_key("session_id"));
        assert_ne!(first["x-client-request-id"], retry["x-client-request-id"]);
    }

    #[test]
    fn invalid_credential_headers_fail_before_dispatch() {
        let config = ResolvedProvider {
            api_key: SecretString::new("invalid\nkey"),
            ..Default::default()
        };
        let failure = request_headers(
            &config,
            &prepared(&config),
            &config.api_key.resolve().unwrap(),
        )
        .unwrap_err();
        assert!(!failure.retryable);
        assert!(!failure.message.contains("key"));
    }

    #[tokio::test]
    async fn environment_credentials_are_resolved_only_at_default_http_dispatch() {
        use httpmock::prelude::{MockServer, POST};

        let server = MockServer::start_async().await;
        let model = server
            .mock_async(|when, then| {
                when.method(POST).path("/chat/completions");
                then.status(200).body("not reached");
            })
            .await;
        let missing = format!("KUKU_PROVIDER_UNSET_CREDENTIAL_{}", std::process::id());
        assert!(std::env::var_os(&missing).is_none());
        let config = ResolvedProvider {
            base_url: server.url(""),
            api_key: SecretString::from_config(crate::config::ApiKey::Env(missing)),
            ..Default::default()
        };
        let mut request = prepared(&config);
        request.url = super::super::prepare::request_url(&config).unwrap();
        let failure = send_prepared(&config, &request, None)
            .await
            .err()
            .expect("missing credentials must fail before HTTP");
        assert_eq!(ProviderFailureKind::Authentication, failure.kind);
        assert!(!failure.retryable);
        assert!(failure.status.is_none());
        model.assert_hits_async(0).await;
    }
}
