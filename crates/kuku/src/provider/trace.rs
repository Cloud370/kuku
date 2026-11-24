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
    /// Follow the process environment and the binary's build provenance.
    #[default]
    Auto,
    Off,
    /// Record protocol structure without request bodies or provider content.
    Diagnostic,
    /// Record request bodies and raw SSE with credential headers redacted.
    Full,
}

impl ProviderTraceMode {
    fn resolve(self) -> Self {
        if self == Self::Auto {
            Self::from_build_and_env(
                env!("KUKU_BUILD_IS_PREVIEW") == "true",
                std::env::var(PROVIDER_TRACE_ENV).ok().as_deref(),
            )
        } else {
            self
        }
    }

    fn from_build_and_env(preview: bool, setting: Option<&str>) -> Self {
        match setting {
            Some("1") => Self::Full,
            Some("0") => Self::Off,
            _ if preview => Self::Diagnostic,
            _ => Self::Off,
        }
    }
}

pub(crate) struct ProviderTrace {
    root: PathBuf,
    metadata: ProviderTraceMetadata,
    provider: String,
    model: String,
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
            state: Mutex::new(TraceState::default()),
        })
    }

    pub(crate) fn full(&self) -> bool {
        self.metadata.mode == ProviderTraceMode::Full
    }

    pub(crate) fn request(&self, url: &str, headers: &HeaderMap, body: Option<Value>) {
        let value = if self.full() {
            json!({"url":url,"headers":redact_headers(headers),"body":body})
        } else {
            json!({})
        };
        self.record(&mut self.state.lock().unwrap(), "request", value, false);
    }

    pub(crate) fn response(&self, url: &str, status: u16, headers: &HeaderMap) {
        let value = if self.full() {
            json!({"url":url,"status":status,"headers":redact_headers(headers)})
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
            value["frame"] = json!(frame);
        }
        self.record(&mut state, "event", value, terminal);
    }

    pub(crate) fn failure(&self, kind: ProviderFailureKind) {
        let mut state = self.state.lock().unwrap();
        let last_frame = state.frame;
        self.record(
            &mut state,
            "error",
            json!({
                "failure_kind": kind.as_event_kind(),
                "after_sequence": last_frame,
            }),
            true,
        );
    }

    pub(crate) fn flush(&self) {
        self.flush_batch(&mut self.state.lock().unwrap(), OffsetDateTime::now_utc());
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

pub(crate) fn redact_headers(headers: &HeaderMap) -> serde_json::Map<String, Value> {
    headers
        .iter()
        .map(|(name, value)| {
            let name = name.as_str().to_ascii_lowercase();
            let value = if value.is_sensitive() || is_secret_header(&name) {
                REDACTED.to_string()
            } else {
                value.to_str().unwrap_or("<non-utf8>").to_string()
            };
            (name, json!(value))
        })
        .collect()
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

#[cfg(test)]
mod tests {
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
    fn mode_follows_build_and_explicit_overrides() {
        use ProviderTraceMode::{Diagnostic, Full, Off};
        for (preview, default) in [(true, Diagnostic), (false, Off)] {
            assert_eq!(
                default,
                ProviderTraceMode::from_build_and_env(preview, None)
            );
            assert_eq!(
                Off,
                ProviderTraceMode::from_build_and_env(preview, Some("0"))
            );
            assert_eq!(
                Full,
                ProviderTraceMode::from_build_and_env(preview, Some("1"))
            );
        }
    }
}
