use std::path::Path;
use std::time::Duration;

use crate::tool::{ToolFaults, ToolResultEnvelope};

use super::output::CaptureMetadata;
use super::{
    output_error, read_output, CommandOutcome, CommandRequest, OutputLimits, OutputRead,
    OutputStream,
};

pub(super) fn render_command_result(
    request: &CommandRequest,
    outcome: CommandOutcome,
    metadata: CaptureMetadata,
    duration: Duration,
    output_dir: &Path,
    output_id: &str,
    limits: OutputLimits,
) -> ToolResultEnvelope {
    let chunk = match read_output(
        output_dir,
        output_id,
        OutputRead {
            offset: 0,
            limit: limits.max_preview_bytes.max(1),
            search: None,
        },
    ) {
        Ok(chunk) => chunk,
        Err(error) => return output_error("read", error, metadata.cleanup_incomplete),
    };
    let duration_ms = duration.as_millis().min(u64::MAX as u128) as u64;
    let mut summary = outcome_summary(request, &outcome, duration_ms);
    summary.push_str(&format!(
        ", stdout {} lines, stderr {} lines",
        metadata.stdout_lines, metadata.stderr_lines
    ));
    let filtered = !chunk.summary.is_empty();
    let mut model_content = if filtered {
        format!(
            "{}\n\nBuild/test summary (selected lines):\n{}",
            summary,
            chunk.summary.join("\n")
        )
    } else {
        let mut text = String::new();
        let mut previous_stream = None;
        for segment in &chunk.segments {
            if previous_stream != Some(segment.stream) {
                if !text.is_empty() {
                    text.push_str("\n\n");
                }
                text.push_str(match segment.stream {
                    OutputStream::Stdout => "stdout:\n",
                    OutputStream::Stderr => "stderr:\n",
                });
                previous_stream = Some(segment.stream);
            }
            text.push_str(&segment.text);
        }
        if text.is_empty() {
            text.push_str("(command produced no output)");
        }
        if outcome.cancelled || outcome.timed_out {
            text = format!("{summary}\n\n{text}");
        }
        text
    };
    let capture_incomplete = metadata.quota_truncated || metadata.cleanup_incomplete;
    let preview_truncated = !filtered && chunk.has_more;
    let reference = render_reference(output_id, &metadata, filtered, chunk.lossy_utf8);
    let content_truncated = model_content.len() + reference.len() > limits.max_preview_bytes;
    let truncated = capture_incomplete || preview_truncated || content_truncated;
    if truncated {
        summary.push_str(", output truncated");
    }
    let suffix = if truncated {
        format!("\n\n(Command output truncated.){reference}")
    } else {
        reference
    };
    fit_content(&mut model_content, &suffix, limits.max_preview_bytes);
    let status = if outcome.cancelled {
        "cancelled"
    } else if outcome.timed_out {
        "error"
    } else {
        "ok"
    };
    ToolResultEnvelope {
        status: status.to_string(),
        summary,
        model_content,
        truncated,
        faults: ToolFaults {
            cleanup_incomplete: metadata.cleanup_incomplete,
            ..Default::default()
        },
        structured: Some(serde_json::json!({
            "kind": "command_result",
            "command": request.command,
            "exit_code": outcome.exit_code,
            "duration_ms": duration_ms,
            "stdout_lines": metadata.stdout_lines,
            "stderr_lines": metadata.stderr_lines,
            "stdout_bytes": metadata.stdout_bytes,
            "stderr_bytes": metadata.stderr_bytes,
            "timed_out": outcome.timed_out,
            "cancelled": outcome.cancelled,
            "line_numbered": false,
            "output_id": output_id,
            "output_ref": {"id": output_id, "offset": 0, "bytes": metadata.stored_bytes},
            "output_order": "capture",
            "capture_complete": !capture_incomplete,
            "quota_truncated": metadata.quota_truncated,
            "cleanup_incomplete": metadata.cleanup_incomplete,
            "output_filtered": filtered,
            "preview_truncated": preview_truncated || content_truncated,
            "lossy_utf8": !filtered && chunk.lossy_utf8,
            "dropped_events": outcome.dropped_events,
            "summary_lines": chunk.summary,
        })),
    }
}

fn outcome_summary(request: &CommandRequest, outcome: &CommandOutcome, duration_ms: u64) -> String {
    if outcome.cancelled {
        format!("command cancelled after {duration_ms}ms")
    } else if outcome.timed_out {
        format!("command timed out after {}s", request.timeout_seconds)
    } else {
        format!(
            "command exited {} in {duration_ms}ms",
            outcome
                .exit_code
                .map_or_else(|| "unknown".to_string(), |code| code.to_string())
        )
    }
}

fn render_reference(
    output_id: &str,
    metadata: &CaptureMetadata,
    filtered: bool,
    lossy: bool,
) -> String {
    let mut reference = format!(
        "\n\nOutput: {output_id} ({} captured bytes; read by merged byte offset).",
        metadata.stored_bytes
    );
    if filtered {
        reference.push_str(" Preview filters build/test lines.");
    }
    if metadata.quota_truncated {
        reference.push_str(" Capture quota reached; further bytes were drained and discarded.");
    }
    if metadata.cleanup_incomplete {
        reference.push_str(" Pipe cleanup deadline reached; capture is incomplete.");
    }
    if lossy && !filtered {
        reference.push_str(" Preview replaces invalid or split UTF-8; stored bytes are unchanged.");
    }
    reference
}

fn fit_content(content: &mut String, suffix: &str, limit: usize) {
    let mut keep = limit.saturating_sub(suffix.len()).min(content.len());
    while !content.is_char_boundary(keep) {
        keep -= 1;
    }
    content.truncate(keep);
    content.push_str(suffix);
    let mut keep = limit.min(content.len());
    while !content.is_char_boundary(keep) {
        keep -= 1;
    }
    content.truncate(keep);
}
