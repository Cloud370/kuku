use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use wreq::header::HeaderMap;

const PROVIDER_TRACE_ENV: &str = "KUKU_PROVIDER_TRACE";
const REDACTED: &str = "<redacted>";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderTraceMetadata {
    pub(crate) kuku_home: PathBuf,
    pub(crate) session_id: String,
    pub(crate) turn: u64,
    pub(crate) request_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnvTraceSetting {
    Enabled,
    Disabled,
}

impl EnvTraceSetting {
    pub(crate) fn current() -> Self {
        if std::env::var(PROVIDER_TRACE_ENV).as_deref() == Ok("1") {
            Self::Enabled
        } else {
            Self::Disabled
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ProviderTrace {
    path: PathBuf,
    session_id: String,
    turn: u64,
    request_id: String,
    provider: String,
    model: String,
}

impl ProviderTrace {
    pub(crate) fn from_request(
        metadata: Option<&ProviderTraceMetadata>,
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> Option<Self> {
        let metadata = metadata?.clone();
        Self::new(
            metadata,
            provider.into(),
            model.into(),
            EnvTraceSetting::current(),
        )
    }

    pub(crate) fn new(
        metadata: ProviderTraceMetadata,
        provider: String,
        model: String,
        setting: EnvTraceSetting,
    ) -> Option<Self> {
        if setting != EnvTraceSetting::Enabled {
            return None;
        }

        provider_trace_path(&metadata.kuku_home, &metadata.session_id)
            .ok()
            .map(|path| Self {
                path,
                session_id: metadata.session_id,
                turn: metadata.turn,
                request_id: metadata.request_id,
                provider,
                model,
            })
    }

    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn record(
        &self,
        direction: ProviderTraceDirection,
        url: Option<&str>,
        headers: Option<&HeaderMap>,
        payload: Value,
    ) {
        let _ = self.record_result(direction, url, headers, payload);
    }

    fn record_result(
        &self,
        direction: ProviderTraceDirection,
        url: Option<&str>,
        headers: Option<&HeaderMap>,
        payload: Value,
    ) -> std::io::Result<()> {
        let mut record = json!({
            "ts": now_timestamp(),
            "direction": direction.as_str(),
            "session_id": self.session_id,
            "turn": self.turn,
            "request_id": self.request_id,
            "provider": self.provider,
            "model": self.model,
        });
        if let Some(url) = url {
            record["url"] = json!(url);
        }
        if let Some(headers) = headers {
            record["headers"] = json!(redact_headers(headers));
        }
        merge_payload(&mut record, payload);

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut line = serde_json::to_vec(&record).map_err(std::io::Error::other)?;
        line.push(b'\n');
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        file.write_all(&line)?;
        file.flush()?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderTraceDirection {
    Request,
    Response,
    Event,
    Error,
}

impl ProviderTraceDirection {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Response => "response",
            Self::Event => "event",
            Self::Error => "error",
        }
    }
}

pub(crate) fn redact_headers(headers: &HeaderMap) -> serde_json::Map<String, Value> {
    headers
        .iter()
        .map(|(name, value)| {
            let name = name.as_str().to_ascii_lowercase();
            let value = if is_secret_header(&name) {
                REDACTED.to_string()
            } else {
                value.to_str().unwrap_or("<non-utf8>").to_string()
            };
            (name, json!(value))
        })
        .collect()
}

fn provider_trace_path(kuku_home: &Path, session_id: &str) -> std::io::Result<PathBuf> {
    validate_session_id(session_id)?;
    Ok(kuku_home
        .join("logs")
        .join("provider-trace")
        .join(current_day())
        .join(format!("{session_id}.jsonl")))
}

fn current_day() -> String {
    OffsetDateTime::now_utc().date().to_string()
}

fn now_timestamp() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| OffsetDateTime::now_utc().unix_timestamp().to_string())
}

fn validate_session_id(session_id: &str) -> std::io::Result<()> {
    let invalid = session_id.is_empty()
        || session_id == "."
        || session_id == ".."
        || session_id.contains("..")
        || session_id.ends_with('.')
        || session_id.ends_with(' ')
        || session_id.contains('/')
        || session_id.contains('\\')
        || session_id.contains('\0')
        || session_id.contains('<')
        || session_id.contains('>')
        || session_id.contains(':')
        || session_id.contains('"')
        || session_id.contains('|')
        || session_id.contains('?')
        || session_id.contains('*');
    if invalid {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid session id: {session_id}"),
        ))
    } else {
        Ok(())
    }
}

fn is_secret_header(name: &str) -> bool {
    matches!(
        name,
        "authorization"
            | "x-api-key"
            | "api-key"
            | "proxy-authorization"
            | "cookie"
            | "set-cookie"
            | "anthropic-api-key"
    )
}

fn merge_payload(record: &mut Value, payload: Value) {
    let Value::Object(record) = record else {
        return;
    };
    match payload {
        Value::Object(payload) => record.extend(payload),
        value => {
            record.insert("data".to_string(), value);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Mutex;

    use wreq::header::{HeaderMap, HeaderValue};

    use super::*;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn env_setting_requires_one() {
        let _guard = ENV_LOCK.lock().unwrap();
        let previous = std::env::var(PROVIDER_TRACE_ENV).ok();

        std::env::remove_var(PROVIDER_TRACE_ENV);
        assert_eq!(EnvTraceSetting::current(), EnvTraceSetting::Disabled);

        std::env::set_var(PROVIDER_TRACE_ENV, "0");
        assert_eq!(EnvTraceSetting::current(), EnvTraceSetting::Disabled);

        std::env::set_var(PROVIDER_TRACE_ENV, "1");
        assert_eq!(EnvTraceSetting::current(), EnvTraceSetting::Enabled);

        if let Some(previous) = previous {
            std::env::set_var(PROVIDER_TRACE_ENV, previous);
        } else {
            std::env::remove_var(PROVIDER_TRACE_ENV);
        }
    }

    #[test]
    fn disabled_env_does_not_create_writer() {
        let trace = ProviderTrace::new(
            ProviderTraceMetadata {
                kuku_home: Path::new("/tmp/kuku-home").to_path_buf(),
                session_id: "s_test".to_string(),
                turn: 2,
                request_id: "req_1".to_string(),
            },
            "anthropic".to_string(),
            "claude-test".to_string(),
            EnvTraceSetting::Disabled,
        );

        assert!(trace.is_none());
    }

    #[test]
    fn enabled_env_creates_provider_trace_day_path() {
        let trace = ProviderTrace::new(
            ProviderTraceMetadata {
                kuku_home: Path::new("/tmp/kuku-home").to_path_buf(),
                session_id: "s_test".to_string(),
                turn: 2,
                request_id: "req_1".to_string(),
            },
            "anthropic".to_string(),
            "claude-test".to_string(),
            EnvTraceSetting::Enabled,
        )
        .expect("trace should be enabled");

        assert!(trace
            .path()
            .starts_with("/tmp/kuku-home/logs/provider-trace/"));
        assert_eq!(trace.path().file_name().unwrap(), "s_test.jsonl");
    }

    #[test]
    fn redacts_secret_headers_case_insensitively() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "Authorization",
            HeaderValue::from_static("Bearer sk-secret"),
        );
        headers.insert("x-api-key", HeaderValue::from_static("api-secret"));
        headers.insert(
            "ANTHROPIC-API-KEY",
            HeaderValue::from_static("anthropic-secret"),
        );
        headers.insert("content-type", HeaderValue::from_static("application/json"));

        let redacted = redact_headers(&headers);

        assert_eq!(redacted["authorization"], "<redacted>");
        assert_eq!(redacted["x-api-key"], "<redacted>");
        assert_eq!(redacted["anthropic-api-key"], "<redacted>");
        assert_eq!(redacted["content-type"], "application/json");
    }
}
