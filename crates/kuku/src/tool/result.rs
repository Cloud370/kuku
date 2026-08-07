use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolErrorReason {
    SnapshotRequired,
    FullSnapshotRequired,
    SnapshotStale,
    OldTextNotVisible,
}

impl ToolErrorReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::SnapshotRequired => "snapshot_required",
            Self::FullSnapshotRequired => "full_snapshot_required",
            Self::SnapshotStale => "snapshot_stale",
            Self::OldTextNotVisible => "old_text_not_visible",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolResultEnvelope {
    pub status: String,
    pub summary: String,
    pub model_content: String,
    pub truncated: bool,
    pub structured: Option<Value>,
}

impl ToolResultEnvelope {
    pub(crate) fn blocked_marker() -> Value {
        serde_json::json!({"kind": "blocked"})
    }

    pub(crate) fn ok(
        summary: impl Into<String>,
        model_content: impl Into<String>,
        structured: Value,
    ) -> Self {
        Self {
            status: "ok".to_string(),
            summary: summary.into(),
            model_content: model_content.into(),
            truncated: false,
            structured: Some(structured),
        }
    }

    pub(crate) fn ok_truncated(
        summary: impl Into<String>,
        model_content: impl Into<String>,
        structured: Value,
    ) -> Self {
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

    pub(crate) fn error_with_reason(
        summary: impl Into<String>,
        model_content: impl Into<String>,
        reason: ToolErrorReason,
    ) -> Self {
        Self {
            status: "error".to_string(),
            summary: summary.into(),
            model_content: model_content.into(),
            truncated: false,
            structured: Some(serde_json::json!({
                "kind": "error",
                "reason_code": reason.as_str(),
            })),
        }
    }

    pub(crate) fn blocked(summary: impl Into<String>, model_content: impl Into<String>) -> Self {
        Self {
            status: "blocked".to_string(),
            summary: summary.into(),
            model_content: model_content.into(),
            truncated: false,
            structured: Some(Self::blocked_marker()),
        }
    }

    #[allow(dead_code)] // will be used when slot cancellation is fully wired
    pub(crate) fn cancelled(summary: impl Into<String>) -> Self {
        Self {
            status: "cancelled".to_string(),
            summary: summary.into(),
            model_content: String::new(),
            truncated: false,
            structured: Some(serde_json::json!({"kind": "cancelled"})),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_error_reasons_preserve_error_kind_and_stable_codes() {
        let cases = [
            (ToolErrorReason::SnapshotRequired, "snapshot_required"),
            (
                ToolErrorReason::FullSnapshotRequired,
                "full_snapshot_required",
            ),
            (ToolErrorReason::SnapshotStale, "snapshot_stale"),
            (ToolErrorReason::OldTextNotVisible, "old_text_not_visible"),
        ];

        for (reason, expected_code) in cases {
            let result = ToolResultEnvelope::error_with_reason("failed", "recover", reason);
            let structured = result.structured.unwrap();

            assert_eq!(result.status, "error");
            assert_eq!(structured["kind"], "error");
            assert_eq!(structured["reason_code"], expected_code);
        }
    }

    #[test]
    fn generic_error_shape_remains_unchanged() {
        let result = ToolResultEnvelope::error("failed", "recover");
        assert_eq!(
            result.structured,
            Some(serde_json::json!({"kind": "error"}))
        );
    }
}
