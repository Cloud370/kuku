use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LogLevel {
    #[serde(rename = "trace")]
    Trace,
    #[serde(rename = "debug")]
    Debug,
    #[serde(rename = "info")]
    Info,
    #[serde(rename = "warn")]
    Warn,
    #[serde(rename = "error")]
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LogScope {
    #[serde(rename = "session")]
    Session,
    #[serde(rename = "runtime")]
    Runtime,
    #[serde(rename = "host")]
    Host,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HostKind {
    #[serde(rename = "cli")]
    Cli,
    #[serde(rename = "server")]
    Server,
    #[serde(rename = "webui")]
    Webui,
}

impl HostKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Server => "server",
            Self::Webui => "webui",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogRecord {
    pub ts: String,
    pub level: LogLevel,
    pub scope: LogScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<HostKind>,
    pub kind: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl LogRecord {
    pub fn new(ts: impl Into<String>, level: LogLevel, scope: LogScope) -> Self {
        Self {
            ts: ts.into(),
            level,
            scope,
            host: None,
            kind: String::new(),
            message: String::new(),
            session_id: None,
            workspace: None,
            run_id: None,
            request_id: None,
            turn: None,
            data: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_record_omits_empty_optional_fields() {
        let mut record = LogRecord::new("2026-06-06T00:00:00Z", LogLevel::Info, LogScope::Runtime);
        record.kind = "runtime.start".to_string();
        record.message = "runtime started".to_string();

        let json = serde_json::to_value(&record).unwrap();
        assert_eq!(json["scope"], "runtime");
        assert!(json.get("host").is_none());
        assert!(json.get("session_id").is_none());
        assert!(json.get("data").is_none());
    }
}
