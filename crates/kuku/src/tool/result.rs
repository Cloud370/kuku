use serde_json::Value;

use crate::execution::CommandExecutionFacts;

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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ToolFaults {
    pub(crate) output_persistence_failed: bool,
    pub(crate) cleanup_incomplete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolResultEnvelope {
    pub status: String,
    pub summary: String,
    pub model_content: String,
    pub truncated: bool,
    pub structured: Option<Value>,
    pub(crate) faults: ToolFaults,
    pub(crate) supplied_full_write: bool,
}

impl ToolResultEnvelope {
    pub(crate) fn command(
        status: &str,
        summary: String,
        model_content: String,
        truncated: bool,
        facts: CommandExecutionFacts,
    ) -> Self {
        Self {
            status: status.to_string(),
            summary,
            model_content,
            truncated,
            faults: ToolFaults {
                cleanup_incomplete: facts.cleanup_incomplete,
                ..Default::default()
            },
            structured: Some(serde_json::to_value(facts).expect("command facts serialize to JSON")),
            supplied_full_write: false,
        }
    }

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
            faults: ToolFaults::default(),
            supplied_full_write: false,
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
            faults: ToolFaults::default(),
            supplied_full_write: false,
        }
    }

    pub(crate) fn error(summary: impl Into<String>, model_content: impl Into<String>) -> Self {
        Self {
            status: "error".to_string(),
            summary: summary.into(),
            model_content: model_content.into(),
            truncated: false,
            structured: Some(serde_json::json!({"kind": "error"})),
            faults: ToolFaults::default(),
            supplied_full_write: false,
        }
    }

    pub(crate) fn output_failure(
        summary: impl Into<String>,
        model_content: impl Into<String>,
        kind: &str,
        cleanup_incomplete: bool,
    ) -> Self {
        Self {
            structured: Some(serde_json::json!({
                "kind": kind, "output_persistence_failed": true, "cleanup_incomplete": cleanup_incomplete,
            })),
            faults: ToolFaults {
                output_persistence_failed: true,
                cleanup_incomplete,
            },
            ..Self::error(summary, model_content)
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
            faults: ToolFaults::default(),
            supplied_full_write: false,
        }
    }

    pub(crate) fn blocked(summary: impl Into<String>, model_content: impl Into<String>) -> Self {
        Self {
            status: "blocked".to_string(),
            summary: summary.into(),
            model_content: model_content.into(),
            truncated: false,
            structured: Some(Self::blocked_marker()),
            faults: ToolFaults::default(),
            supplied_full_write: false,
        }
    }

    pub(crate) fn cancelled(summary: impl Into<String>) -> Self {
        Self {
            status: "cancelled".to_string(),
            summary: summary.into(),
            model_content: String::new(),
            truncated: false,
            structured: Some(serde_json::json!({"kind": "cancelled"})),
            faults: ToolFaults::default(),
            supplied_full_write: false,
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
