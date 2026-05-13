use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolResultEnvelope {
    pub status: String,
    pub summary: String,
    pub model_content: String,
    pub truncated: bool,
    pub structured: Option<Value>,
}

impl ToolResultEnvelope {
    pub(crate) fn ok(summary: impl Into<String>, model_content: impl Into<String>, structured: Value) -> Self {
        Self {
            status: "ok".to_string(),
            summary: summary.into(),
            model_content: model_content.into(),
            truncated: false,
            structured: Some(structured),
        }
    }

    pub(crate) fn ok_truncated(summary: impl Into<String>, model_content: impl Into<String>, structured: Value) -> Self {
        Self {
            status: "ok".to_string(),
            summary: summary.into(),
            model_content: model_content.into(),
            truncated: true,
            structured: Some(structured),
        }
    }

    pub(crate) fn error(summary: impl Into<String>, model_content: impl Into<String>) -> Self {
        Self {
            status: "error".to_string(),
            summary: summary.into(),
            model_content: model_content.into(),
            truncated: false,
            structured: Some(serde_json::json!({"kind": "error"})),
        }
    }

    pub(crate) fn blocked(summary: impl Into<String>, model_content: impl Into<String>) -> Self {
        Self {
            status: "blocked".to_string(),
            summary: summary.into(),
            model_content: model_content.into(),
            truncated: false,
            structured: Some(serde_json::json!({"kind": "blocked"})),
        }
    }
}
