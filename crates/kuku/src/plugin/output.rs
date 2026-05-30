use serde::{Deserialize, Serialize};
use std::path::Path;

const OVERFLOW_THRESHOLD: usize = 100 * 1024;

/// Structured output parsed from a hook's stdout JSON.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct HookOutput {
    #[serde(default)]
    pub block: bool,
    #[serde(default)]
    pub updated_args: Option<serde_json::Value>,
    #[serde(default)]
    pub updated_result: Option<serde_json::Value>,
    #[serde(default)]
    pub additional_context: Option<String>,
}

/// Parse hook stdout as JSON, falling back to treating the entire output as additional context.
pub(crate) fn parse_stdout(stdout: &str) -> HookOutput {
    match serde_json::from_str::<HookOutput>(stdout) {
        Ok(output) => output,
        Err(_) => HookOutput {
            additional_context: Some(stdout.to_string()),
            ..Default::default()
        },
    }
}

/// Truncate oversized hook output and persist the full text to an overflow file.
pub(crate) fn handle_overflow(stdout: &str, session_dir: &Path, index: usize) -> String {
    if stdout.len() <= OVERFLOW_THRESHOLD {
        return stdout.to_string();
    }
    let truncated = &stdout[..OVERFLOW_THRESHOLD];
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let overflow_dir = session_dir.join("hook_overflow");
    let _ = std::fs::create_dir_all(&overflow_dir);
    let overflow_path = overflow_dir.join(format!("{index}_{timestamp}.out"));
    let _ = std::fs::write(&overflow_path, stdout);
    format!(
        "{}\n[truncated — full output: {}]",
        truncated,
        overflow_path.display()
    )
}

/// Merge multiple hook outputs, concatenating contexts and taking the last scalar value.
pub(crate) fn merge_outputs(outputs: &[HookOutput]) -> HookOutput {
    let mut result = HookOutput::default();
    let mut contexts = Vec::new();

    for output in outputs {
        if output.block {
            result.block = true;
        }
        if output.updated_args.is_some() {
            result.updated_args = output.updated_args.clone();
        }
        if output.updated_result.is_some() {
            result.updated_result = output.updated_result.clone();
        }
        if let Some(ref ctx) = output.additional_context {
            contexts.push(ctx.clone());
        }
    }

    if !contexts.is_empty() {
        result.additional_context = Some(contexts.join("\n"));
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_json_output() {
        let stdout = r#"{"block": true, "additional_context": "reason"}"#;
        let out = parse_stdout(stdout);
        assert!(out.block);
        assert_eq!(out.additional_context.as_deref(), Some("reason"));
    }

    #[test]
    fn parse_non_json_wraps_as_context() {
        let stdout = "just plain text output";
        let out = parse_stdout(stdout);
        assert!(!out.block);
        assert_eq!(
            out.additional_context.as_deref(),
            Some("just plain text output")
        );
    }

    #[test]
    fn merge_last_write_wins_scalars() {
        let outputs = vec![
            HookOutput {
                updated_args: Some(serde_json::json!({"a": 1})),
                ..Default::default()
            },
            HookOutput {
                updated_args: Some(serde_json::json!({"b": 2})),
                ..Default::default()
            },
        ];
        let merged = merge_outputs(&outputs);
        assert_eq!(merged.updated_args, Some(serde_json::json!({"b": 2})));
    }

    #[test]
    fn merge_concat_contexts() {
        let outputs = vec![
            HookOutput {
                additional_context: Some("first".into()),
                ..Default::default()
            },
            HookOutput {
                additional_context: Some("second".into()),
                ..Default::default()
            },
        ];
        let merged = merge_outputs(&outputs);
        assert_eq!(merged.additional_context.as_deref(), Some("first\nsecond"));
    }

    #[test]
    fn merge_block_any() {
        let outputs = vec![
            HookOutput {
                block: false,
                ..Default::default()
            },
            HookOutput {
                block: true,
                ..Default::default()
            },
        ];
        assert!(merge_outputs(&outputs).block);
    }

    #[test]
    fn overflow_truncates_and_writes_file() {
        let tmp = tempfile::tempdir().unwrap();
        let big = "x".repeat(200 * 1024);
        let result = handle_overflow(&big, tmp.path(), 0);
        assert!(result.len() < big.len());
        assert!(result.contains("[truncated"));
        let overflow_files: Vec<_> = std::fs::read_dir(tmp.path().join("hook_overflow"))
            .unwrap()
            .collect();
        assert_eq!(overflow_files.len(), 1);
    }
}
