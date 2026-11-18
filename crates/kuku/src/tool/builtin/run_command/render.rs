#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::ExitStatus;
use std::time::Duration;

use crate::execution::{
    CommandExecutionFacts, CommandResultKind, CommandStdin, CommandTermination, OutputReference,
    ReportedTests,
};
use crate::tool::ToolResultEnvelope;

use super::output::{CaptureMetadata, OutputSegment};
use super::{
    output_error, read_output, CommandOutcome, CommandRequest, OutputLimits, OutputRead,
    OutputStream,
};

const MAX_OUTPUT_PREVIEW_BYTES: usize = 2048;

pub(super) fn render_command_result(
    request: &CommandRequest,
    outcome: CommandOutcome,
    capture: (CaptureMetadata, Vec<ReportedTests>),
    duration: Duration,
    output_dir: &Path,
    output_id: &str,
    limits: OutputLimits,
) -> ToolResultEnvelope {
    let (metadata, reports) = capture;
    let filtered = !metadata.summary.is_empty() || !reports.is_empty();
    let output_preview_limit = limits.max_preview_bytes.min(MAX_OUTPUT_PREVIEW_BYTES);
    let read_limit = if filtered {
        output_preview_limit
    } else {
        limits.max_preview_bytes
    };
    let chunk = match read_output(
        output_dir,
        output_id,
        OutputRead {
            offset: 0,
            limit: read_limit.max(1),
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
    let raw_preview = render_streams(&chunk.segments);
    let mut output_preview = if chunk
        .segments
        .iter()
        .all(|segment| segment.stream == OutputStream::Stdout)
    {
        raw_preview
            .strip_prefix("stdout:\n")
            .unwrap_or(&raw_preview)
            .to_string()
    } else {
        raw_preview.clone()
    };
    let output_preview_truncated = chunk.has_more || output_preview.len() > output_preview_limit;
    truncate_utf8(&mut output_preview, output_preview_limit);
    let mut model_content = if filtered {
        format!(
            "{}\n\nBuild/test summary (selected lines):\n{}",
            summary,
            report_evidence(&metadata.summary, &reports)
        )
    } else {
        let text = if raw_preview.is_empty() {
            "(command produced no output)"
        } else {
            &raw_preview
        };
        format!("{summary}\n\n{text}")
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
    let lossy_utf8 = chunk.lossy_utf8
        && (output_preview.contains(char::REPLACEMENT_CHARACTER)
            || (!filtered && model_content.contains(char::REPLACEMENT_CHARACTER)));
    let status = if outcome.cancelled {
        "cancelled"
    } else if outcome.timed_out || outcome.start_error.is_some() {
        "error"
    } else {
        "ok"
    };
    let termination = if outcome.cancelled {
        CommandTermination::Cancelled
    } else if outcome.timed_out {
        CommandTermination::TimedOut
    } else if let Some((termination, _)) = &outcome.start_error {
        *termination
    } else {
        CommandTermination::Exited
    };
    ToolResultEnvelope::command(
        status,
        summary,
        model_content,
        truncated,
        CommandExecutionFacts {
            kind: CommandResultKind::CommandResult,
            invocation: request.invocation.clone(),
            cwd: request.cwd.to_string_lossy().into_owned(),
            stdin: CommandStdin::Null,
            termination,
            started: outcome.started,
            exit_code: outcome.exit_status.and_then(|status| status.code()),
            signal: exit_signal(outcome.exit_status),
            start_error: outcome.start_error.map(|(_, error)| error),
            duration_ms,
            stdout_lines: metadata.stdout_lines,
            stderr_lines: metadata.stderr_lines,
            stdout_bytes: metadata.stdout_bytes,
            stderr_bytes: metadata.stderr_bytes,
            timed_out: outcome.timed_out,
            cancelled: outcome.cancelled,
            output_id: output_id.to_string(),
            output_ref: OutputReference {
                id: output_id.to_string(),
                offset: 0,
                bytes: metadata.stored_bytes,
            },
            capture_complete: !capture_incomplete,
            quota_truncated: metadata.quota_truncated,
            cleanup_incomplete: metadata.cleanup_incomplete,
            output_filtered: filtered,
            preview_truncated: preview_truncated || content_truncated,
            lossy_utf8,
            output_preview,
            output_preview_truncated,
            dropped_events: outcome.dropped_events,
            summary_lines: chunk.summary,
            reported_tests: reports,
            side_effects_observed: false,
        },
    )
}

fn outcome_summary(request: &CommandRequest, outcome: &CommandOutcome, duration_ms: u64) -> String {
    if outcome.cancelled {
        format!("command cancelled after {duration_ms}ms")
    } else if outcome.timed_out {
        format!("command timed out after {}s", request.timeout_seconds)
    } else if let Some((termination, error)) = &outcome.start_error {
        let kind = if *termination == CommandTermination::ProgramMissing {
            "program missing"
        } else {
            "program could not start"
        };
        format!("command {kind}: {error}")
    } else if let Some(signal) = exit_signal(outcome.exit_status) {
        format!("command terminated by signal {signal} in {duration_ms}ms")
    } else {
        format!(
            "command exited {} in {duration_ms}ms",
            outcome
                .exit_status
                .and_then(|status| status.code())
                .map_or_else(|| "unknown".to_string(), |code| code.to_string())
        )
    }
}

#[cfg(unix)]
fn exit_signal(status: Option<ExitStatus>) -> Option<i32> {
    status.and_then(|status| status.signal())
}

#[cfg(not(unix))]
fn exit_signal(_: Option<ExitStatus>) -> Option<i32> {
    None
}

fn render_streams(segments: &[OutputSegment]) -> String {
    let mut text = String::new();
    let mut previous_stream = None;
    for segment in segments {
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
    text
}

fn report_evidence(summary: &[String], reports: &[ReportedTests]) -> String {
    let mut lines = Vec::new();
    for report in reports {
        let mut counts = vec![
            format!("{} passed", report.passed),
            format!("{} failed", report.failed),
            format!("{} skipped", report.skipped),
        ];
        for (count, label) in [
            (report.errors, "errors"),
            (report.measured, "measured"),
            (report.filtered_out, "filtered out"),
            (report.expected_failures, "expected failures"),
            (report.unexpected_passes, "unexpected passes"),
            (report.warnings, "warnings"),
        ] {
            if count > 0 {
                counts.push(format!("{count} {label}"));
            }
        }
        lines.push(format!(
            "Program-reported {:?} tests across {} summaries: {}.",
            report.format,
            report.suites,
            counts.join(", ")
        ));
        if let Some(failure) = &report.first_failure {
            lines.push(format!("First reported failure: {failure}"));
        }
        lines.extend(report.evidence.iter().cloned());
    }
    for line in summary {
        if !lines.contains(line) {
            lines.push(line.clone());
        }
    }
    lines.join("\n")
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
    truncate_utf8(content, limit.saturating_sub(suffix.len()));
    content.push_str(suffix);
    truncate_utf8(content, limit);
}

fn truncate_utf8(content: &mut String, limit: usize) {
    content.truncate(content.floor_char_boundary(limit.min(content.len())));
}
