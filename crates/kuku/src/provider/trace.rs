use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use wreq::header::HeaderMap;

use crate::config::LogsConfig;
use crate::log::provider_trace::{append, segment_limit, trace_root};

use super::sse::parse_sse_frame;
use super::types::ProviderFailureKind;

const PROVIDER_TRACE_ENV: &str = "KUKU_PROVIDER_TRACE";
const REDACTED: &str = "<redacted>";
const BATCH_BYTES: usize = 32 * 1024;
const MAX_ALIASES: usize = 512;
const MAX_DETAILS_PER_FRAME: usize = 64;

#[derive(Debug, Clone)]
pub(crate) struct ProviderTraceMetadata {
    pub(crate) kuku_home: PathBuf,
    pub(crate) session_id: String,
    pub(crate) turn: u64,
    pub(crate) request_id: String,
    pub(crate) logs: LogsConfig,
    pub(crate) mode: ProviderTraceMode,
}

/// Policy for recording built-in provider HTTP and streaming diagnostics.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ProviderTraceMode {
    /// Record full traces unless the process explicitly disables them.
    #[default]
    Auto,
    Off,
    /// Record protocol structure without request bodies or provider content.
    Diagnostic,
    /// Record redacted headers, request bodies, bounded HTTP error bodies, and SSE.
    Full,
}

impl ProviderTraceMode {
    fn resolve(self) -> Self {
        if self == Self::Auto {
            Self::from_env(std::env::var(PROVIDER_TRACE_ENV).ok().as_deref())
        } else {
            self
        }
    }

    fn from_env(setting: Option<&str>) -> Self {
        if setting == Some("0") {
            Self::Off
        } else {
            Self::Full
        }
    }
}

pub(crate) struct ProviderTrace {
    root: PathBuf,
    metadata: ProviderTraceMetadata,
    provider: String,
    model: String,
    credentials: Vec<String>,
    state: Mutex<TraceState>,
}

#[derive(Default)]
struct TraceState {
    bytes: Vec<u8>,
    frame: u64,
    fingerprints: RandomState,
    aliases: HashMap<(&'static str, u64), usize>,
}

impl TraceState {
    fn alias(&mut self, key: &'static str, value: &Value) -> Option<usize> {
        let text = value.as_str().filter(|text| !text.is_empty())?;
        let key = (key, self.fingerprints.hash_one(text));
        let count = self.aliases.len();
        if count >= MAX_ALIASES {
            return self.aliases.get(&key).copied();
        }
        Some(*self.aliases.entry(key).or_insert(count + 1))
    }

    fn reasoning_metadata(&mut self, detail: &Value) -> Value {
        json!({
            "id_alias": self.alias("id", &detail["id"]),
            "type": match detail["type"].as_str() {
                Some(kind @ ("reasoning.text" | "reasoning.summary" | "reasoning.encrypted")) => json!(kind),
                _ => json!(self.alias("type", &detail["type"])),
            },
            "format": match detail["format"].as_str() {
                Some(format @ ("google-gemini-v1" | "anthropic-claude-v1" | "openai-responses-v1" | "xai-responses-v1" | "unknown")) => json!(format),
                _ => json!(self.alias("format", &detail["format"])),
            },
            "index": detail["index"].as_u64(),
        })
    }

    fn frame_metadata(&mut self, raw: &str) -> (Value, bool) {
        let mut record = json!({"sequence": self.frame, "bytes": raw.len()});
        let Some(frame) = parse_sse_frame(raw) else {
            record["event"] = json!("comment_or_empty");
            return (record, false);
        };
        if frame.data.trim() == "[DONE]" {
            record["terminal"] = json!("done");
            return (record, true);
        }
        let Ok(data) = serde_json::from_str::<Value>(&frame.data) else {
            record["event"] = json!("malformed_json");
            return (record, false);
        };
        let event = frame.event.as_deref().or_else(|| data["type"].as_str());
        let terminal = match event {
            Some("message_stop") => Some("message_stop"),
            Some("response.completed") => Some("response.completed"),
            Some("response.incomplete") => Some("response.incomplete"),
            Some("response.failed") => Some("response.failed"),
            Some("error") => Some("error"),
            _ if data.get("error").is_some() => Some("error"),
            _ => None,
        };
        if let Some(event) = event {
            record["event"] = match event {
                "message_start"
                | "message_delta"
                | "message_stop"
                | "content_block_start"
                | "content_block_delta"
                | "content_block_stop"
                | "response.created"
                | "response.output_text.delta"
                | "response.output_item.added"
                | "response.output_item.done"
                | "response.completed"
                | "response.failed"
                | "response.incomplete"
                | "error"
                | "ping" => json!(event),
                _ => json!("other"),
            };
        }
        let choice = data.pointer("/choices/0");
        if let Some(choice) = choice {
            if let Some(reason) = choice["finish_reason"].as_str() {
                record["finish_reason"] = match reason {
                    "stop" | "length" | "tool_calls" | "content_filter" | "function_call" => {
                        json!(reason)
                    }
                    _ => json!("other"),
                };
            }
            if let Some(details) = choice
                .pointer("/delta/reasoning_details")
                .and_then(Value::as_array)
            {
                record["reasoning_details"] = json!(
                    details
                        .iter()
                        .take(MAX_DETAILS_PER_FRAME)
                        .map(|item| self.reasoning_metadata(item))
                        .collect::<Vec<_>>()
                );
                if details.len() > MAX_DETAILS_PER_FRAME {
                    record["reasoning_details_truncated"] = json!(true);
                }
            }
        }
        if let Some(terminal) = terminal {
            record["terminal"] = json!(terminal);
        }
        let flush = terminal.is_some() || record.get("finish_reason").is_some();
        (record, flush)
    }
}

impl ProviderTrace {
    pub(crate) fn from_request(
        metadata: Option<ProviderTraceMetadata>,
        provider: String,
        model: String,
    ) -> Option<Self> {
        let mut metadata = metadata?;
        metadata.mode = metadata.mode.resolve();
        if metadata.mode == ProviderTraceMode::Off || segment_limit(&metadata.logs) == 0 {
            return None;
        }
        Some(Self {
            root: trace_root(&metadata.kuku_home),
            metadata,
            provider,
            model,
            credentials: Vec::new(),
            state: Mutex::new(TraceState::default()),
        })
    }

    pub(crate) fn full(&self) -> bool {
        self.metadata.mode == ProviderTraceMode::Full
    }

    pub(super) fn request(
        &mut self,
        url: &str,
        headers: &HeaderMap,
        body: &Value,
        custom_fields: &[String],
        api_key: &str,
    ) {
        let value = if self.full() {
            remember_credential(&mut self.credentials, api_key);
            for (name, value) in headers {
                if (value.is_sensitive() || is_credential_field(name.as_str()))
                    && let Ok(value) = std::str::from_utf8(value.as_bytes())
                {
                    remember_credential(&mut self.credentials, value);
                    if let Some((_, token)) = value.split_once(' ')
                        && name.as_str() == "authorization"
                    {
                        remember_credential(&mut self.credentials, token);
                    }
                }
            }
            let url = redact_url_credentials(url, |credential| {
                remember_credential(&mut self.credentials, credential)
            });
            let mut body = body.clone();
            for name in custom_fields {
                if let Some(value) = body.get_mut(name) {
                    redact_credential_field(name, value, &mut self.credentials);
                }
            }
            redact_known_values(&mut body, &self.credentials);
            json!({"url":self.credential_redacted(&url, false),"headers":self.headers(headers),"body":body})
        } else {
            json!({})
        };
        self.record(&mut self.state.lock().unwrap(), "request", value, false);
    }

    pub(crate) fn response(&self, url: &str, status: u16, headers: &HeaderMap) {
        let value = if self.full() {
            let url = redact_url_credentials(url, |_| {});
            json!({"url":self.credential_redacted(&url, false),"status":status,"headers":self.headers(headers)})
        } else {
            json!({"status":status})
        };
        self.record(&mut self.state.lock().unwrap(), "response", value, false);
    }

    pub(crate) fn frame(&self, frame: &str) {
        let mut state = self.state.lock().unwrap();
        state.frame += 1;
        let (mut value, terminal) = state.frame_metadata(frame);
        if self.full() {
            value["frame"] = json!(self.credential_redacted(frame, false));
        }
        self.record(&mut state, "event", value, terminal);
    }

    pub(crate) fn failure(&self, kind: ProviderFailureKind, http_body: Option<(&str, bool)>) {
        let mut state = self.state.lock().unwrap();
        let mut value = json!({
            "failure_kind": kind.as_event_kind(),
            "after_sequence": state.frame,
        });
        if self.full()
            && let Some((body, truncated)) = http_body
        {
            value["body"] = json!(self.credential_redacted(body, truncated));
            value["body_truncated"] = json!(truncated);
        }
        self.record(&mut state, "error", value, true);
    }

    pub(crate) fn flush(&self) {
        self.flush_batch(&mut self.state.lock().unwrap(), OffsetDateTime::now_utc());
    }

    fn credential_redacted(&self, text: &str, truncated: bool) -> String {
        let mut text = text.to_owned();
        for credential in &self.credentials {
            redact_secret(&mut text, credential, truncated);
        }
        text
    }

    fn headers(&self, headers: &HeaderMap) -> serde_json::Map<String, Value> {
        let mut headers = redact_headers(headers);
        for value in headers.values_mut() {
            redact_known_values(value, &self.credentials);
        }
        headers
    }

    fn record(&self, state: &mut TraceState, direction: &str, value: Value, flush: bool) {
        let now = OffsetDateTime::now_utc();
        let mut record = json!({
            "ts": now.format(&Rfc3339).unwrap_or_default(),
            "direction": direction,
            "session_id": self.metadata.session_id,
            "turn": self.metadata.turn,
            "request_id": self.metadata.request_id,
            "provider": self.provider,
            "model": self.model,
        });
        let fields = record.as_object_mut().expect("trace metadata is an object");
        if let Value::Object(value) = value {
            fields.extend(value);
        }
        let limit = segment_limit(&self.metadata.logs) as usize;
        let Ok(mut line) = serde_json::to_vec(&record) else {
            return;
        };
        if line.len() >= limit {
            let object = record.as_object_mut().expect("trace metadata is an object");
            object.retain(|key, _| {
                matches!(
                    key.as_str(),
                    "ts" | "direction"
                        | "session_id"
                        | "turn"
                        | "request_id"
                        | "provider"
                        | "model"
                        | "sequence"
                )
            });
            object.insert("truncated".into(), json!(true));
            object.insert("bytes".into(), json!(line.len()));
            line = serde_json::to_vec(&record).unwrap_or_default();
        }
        if line.len() >= limit || line.is_empty() {
            return;
        }
        if state.bytes.len() + line.len() + 1 > BATCH_BYTES.min(limit) {
            self.flush_batch(state, now);
        }
        state.bytes.extend(line);
        state.bytes.push(b'\n');
        if flush || state.bytes.len() >= BATCH_BYTES.min(limit) {
            self.flush_batch(state, now);
        }
    }

    fn flush_batch(&self, state: &mut TraceState, now: OffsetDateTime) {
        let _ = append(&self.root, &self.metadata.logs, now.date(), &state.bytes);
        state.bytes.clear();
    }
}

fn redact_headers(headers: &HeaderMap) -> serde_json::Map<String, Value> {
    let mut redacted = serde_json::Map::new();
    for (name, raw) in headers {
        let name = name.as_str();
        let value = if raw.is_sensitive() || is_credential_field(name) {
            json!(REDACTED)
        } else {
            json!(String::from_utf8_lossy(raw.as_bytes()))
        };
        match redacted.entry(name) {
            serde_json::map::Entry::Vacant(entry) => {
                entry.insert(value);
            }
            serde_json::map::Entry::Occupied(mut entry) => match entry.get_mut() {
                Value::Array(values) => values.push(value),
                first => *first = Value::Array(vec![std::mem::take(first), value]),
            },
        }
    }
    redacted
}

pub(super) fn is_credential_field(name: &str) -> bool {
    let name = name.to_ascii_lowercase().replace('_', "-");
    matches!(
        name.as_str(),
        "authorization"
            | "proxy-authorization"
            | "cookie"
            | "set-cookie"
            | "key"
            | "api-key"
            | "apikey"
            | "token"
            | "secret"
            | "password"
            | "credential"
            | "credentials"
    ) || [
        "-api-key",
        "-token",
        "-secret",
        "-password",
        "-credential",
        "-credentials",
    ]
    .iter()
    .any(|suffix| name.ends_with(suffix))
}

fn remember_credential(credentials: &mut Vec<String>, credential: &str) {
    if !credential.is_empty() && !credentials.iter().any(|known| known == credential) {
        credentials.push(credential.to_owned());
    }
}

fn redact_credential_field(name: &str, value: &mut Value, credentials: &mut Vec<String>) {
    fn remember_values(value: &Value, credentials: &mut Vec<String>) {
        match value {
            Value::String(secret) => remember_credential(credentials, secret),
            Value::Object(fields) => fields
                .values()
                .for_each(|value| remember_values(value, credentials)),
            Value::Array(values) => values
                .iter()
                .for_each(|value| remember_values(value, credentials)),
            _ => {}
        }
    }
    if is_credential_field(name) {
        remember_values(value, credentials);
        *value = json!(REDACTED);
    } else {
        match value {
            Value::Object(fields) => fields
                .iter_mut()
                .for_each(|(name, value)| redact_credential_field(name, value, credentials)),
            Value::Array(values) => values
                .iter_mut()
                .for_each(|value| redact_credential_field("", value, credentials)),
            _ => {}
        }
    }
}

fn redact_known_values(value: &mut Value, credentials: &[String]) {
    match value {
        Value::String(text) => {
            for credential in credentials {
                redact_secret(text, credential, false);
            }
        }
        Value::Object(fields) => fields
            .values_mut()
            .for_each(|value| redact_known_values(value, credentials)),
        Value::Array(values) => values
            .iter_mut()
            .for_each(|value| redact_known_values(value, credentials)),
        _ => {}
    }
}

fn redact_url_credentials(url: &str, mut on_secret: impl FnMut(&str)) -> String {
    let Ok(mut parsed) = url::Url::parse(url) else {
        return url.to_owned();
    };
    let mut replaced = false;
    let parameters = parsed
        .query_pairs()
        .map(|(name, value)| {
            if is_credential_field(&name)
                || matches!(name.as_ref(), "sig" | "signature" | "x-amz-signature")
            {
                on_secret(&value);
                replaced = true;
                (name.into_owned(), REDACTED.to_owned())
            } else {
                (name.into_owned(), value.into_owned())
            }
        })
        .collect::<Vec<_>>();
    if !replaced {
        return url.to_owned();
    }
    parsed.query_pairs_mut().clear().extend_pairs(parameters);
    parsed.into()
}

pub(super) fn redact_secret(body: &mut String, secret: &str, truncated: bool) {
    if secret.is_empty() {
        return;
    }
    if body.contains(secret) {
        *body = body.replace(secret, REDACTED);
    }
    // A bounded read can end halfway through a credential returned by the upstream.
    if truncated
        && let Some((offset, _)) = body
            .char_indices()
            .rev()
            .take_while(|&(offset, _)| body.len() - offset < secret.len())
            .filter(|&(offset, _)| secret.starts_with(&body[offset..]))
            .last()
    {
        body.replace_range(offset.., REDACTED);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use wreq::header::{HeaderMap, HeaderValue};

    use crate::config::LogsConfig;

    use super::{ProviderTrace, ProviderTraceMetadata, ProviderTraceMode};

    #[test]
    fn full_trace_keeps_correlation_on_oversized_frame() {
        let home = tempfile::tempdir().unwrap();
        let trace = ProviderTrace::from_request(
            Some(ProviderTraceMetadata {
                kuku_home: home.path().into(),
                session_id: "large-frame".into(),
                turn: 7,
                request_id: "req_1".into(),
                logs: LogsConfig {
                    max_total_size_mb: 1,
                    ..LogsConfig::default()
                },
                mode: ProviderTraceMode::Full,
            }),
            "fixture".into(),
            "model-fixture".into(),
        )
        .unwrap();
        trace.frame(&format!("data: {}", "OVERSIZED".repeat(128 * 1024)));
        trace.flush();
        let root = super::trace_root(home.path());
        let path = std::fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
            .unwrap();
        let contents = std::fs::read_to_string(path).unwrap();
        let record: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(record["truncated"], true);
        assert_eq!(record["sequence"], 1);
        assert_eq!(record["session_id"], "large-frame");
        assert_eq!(record["request_id"], "req_1");
        assert!(!contents.contains("OVERSIZED"));
    }

    #[test]
    fn auto_mode_is_full_unless_explicitly_disabled() {
        use ProviderTraceMode::{Full, Off};
        assert_eq!(ProviderTraceMode::from_env(None), Full);
        assert_eq!(ProviderTraceMode::from_env(Some("1")), Full);
        assert_eq!(ProviderTraceMode::from_env(Some("0")), Off);
    }

    #[test]
    fn full_trace_redacts_credentials_but_keeps_wire_data() {
        let home = tempfile::tempdir().unwrap();
        let mut trace = ProviderTrace::from_request(
            Some(ProviderTraceMetadata {
                kuku_home: home.path().into(),
                session_id: "wire".into(),
                turn: 1,
                request_id: "req-wire".into(),
                logs: LogsConfig::default(),
                mode: ProviderTraceMode::Full,
            }),
            "wire-test".into(),
            "test-model".into(),
        )
        .unwrap();
        let mut request = HeaderMap::new();
        request.insert(
            "authorization",
            HeaderValue::from_static("Bearer example-bearer-value"),
        );
        request.insert("x-debug-region", HeaderValue::from_static("region-one"));
        request.append("x-debug-region", HeaderValue::from_static("region-two"));
        let url = "https://example.test/generate?key=example-url-secret&region=region-one";
        trace.request(
            url,
            &request,
            &json!({
                "config": {"client_secret":"example-body-secret", "color":"blue"},
                "messages":[{"role":"user","content":"retained prompt"}],
            }),
            &["config".into()],
            "example-bearer-value",
        );
        let mut response = HeaderMap::new();
        response.insert("set-cookie", HeaderValue::from_static("sid=example-cookie"));
        response.insert(
            "x-debug-echo",
            HeaderValue::from_static("echo example-bearer-value"),
        );
        trace.response(url, 200, &response);
        trace.frame("data: {\"type\":\"error\",\"detail\":\"retained detail: example-bearer-value example-body-secret example-url-secret\"}");
        trace.flush();

        let records: String = std::fs::read_dir(home.path().join("logs/provider-trace"))
            .unwrap()
            .filter_map(|entry| {
                let path = entry.unwrap().path();
                path.extension()
                    .is_some_and(|ext| ext == "jsonl")
                    .then(|| std::fs::read_to_string(path).unwrap())
            })
            .collect();
        for secret in [
            "example-bearer-value",
            "example-body-secret",
            "example-url-secret",
            "example-cookie",
        ] {
            assert!(!records.contains(secret));
        }
        let records: Vec<Value> = records
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let [request, response, frame] = records.as_slice() else {
            panic!("expected request, response, and frame");
        };
        assert_eq!(
            request["headers"]["x-debug-region"],
            json!(["region-one", "region-two"])
        );
        assert_eq!(
            request["body"]["config"],
            json!({"client_secret":"<redacted>", "color":"blue"})
        );
        assert_eq!(request["body"]["messages"][0]["content"], "retained prompt");
        assert!(
            request["url"]
                .as_str()
                .unwrap()
                .contains("region=region-one")
        );
        assert_eq!(response["headers"]["x-debug-echo"], "echo <redacted>");
        assert!(
            frame["frame"]
                .as_str()
                .unwrap()
                .contains("retained detail: <redacted> <redacted> <redacted>")
        );
    }
}
