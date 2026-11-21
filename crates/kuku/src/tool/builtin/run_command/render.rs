#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use std::time::Duration;

use serde_json::json;

use crate::execution::{
    CommandExecutionFacts, CommandResultKind, CommandStdin, CommandTermination, OutputReference,
    ReportedTests,
};
use crate::tool::ToolResultEnvelope;

use super::output::{read_stderr, CaptureMetadata, OutputSegment};
use super::{
    output_error, read_output, CommandCapture, CommandOutcome, CommandRequest, CommandView,
    OutputRead, OutputStream,
};

const MAX_OUTPUT_PREVIEW_BYTES: usize = 2048;

pub(super) fn render_command_result(
    request: &CommandRequest,
    outcome: CommandOutcome,
    capture: (CaptureMetadata, Vec<ReportedTests>),
    duration: Duration,
    output: CommandCapture<'_>,
) -> ToolResultEnvelope {
    let (metadata, reports) = capture;
    let duration_ms = duration.as_millis().min(u64::MAX as u128) as u64;
    let summary = format!(
        "{}, stdout {} lines, stderr {} lines",
        outcome_summary(request, &outcome, duration_ms),
        metadata.stdout_lines,
        metadata.stderr_lines
    );
    let termination = if outcome.cancelled {
        CommandTermination::Cancelled
    } else if outcome.timed_out {
        CommandTermination::TimedOut
    } else if let Some((termination, _)) = &outcome.start_error {
        *termination
    } else {
        CommandTermination::Exited
    };
    let status = match termination {
        CommandTermination::Exited => "ok",
        CommandTermination::Cancelled => "cancelled",
        CommandTermination::TimedOut
        | CommandTermination::ProgramMissing
        | CommandTermination::StartFailed => "error",
    };
    let facts = CommandExecutionFacts {
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
        output_id: output.id.into(),
        output_ref: OutputReference {
            id: output.id.into(),
            offset: 0,
            bytes: metadata.stored_bytes,
        },
        capture_complete: !metadata.quota_truncated && !metadata.cleanup_incomplete,
        quota_truncated: metadata.quota_truncated,
        cleanup_incomplete: metadata.cleanup_incomplete,
        output_filtered: false,
        preview_truncated: false,
        lossy_utf8: false,
        output_preview: String::new(),
        output_preview_truncated: metadata.stored_bytes > 0,
        dropped_events: outcome.dropped_events,
        summary_lines: metadata.summary.clone(),
        reported_tests: reports,
        side_effects_observed: false,
    };
    match output.view {
        CommandView::Model => render_model(status, summary, facts, &metadata, output),
        CommandView::Script { host_bytes } => {
            render_script(status, summary, facts, output, host_bytes)
        }
    }
}

fn render_model(
    status: &str,
    mut summary: String,
    mut facts: CommandExecutionFacts,
    metadata: &CaptureMetadata,
    output: CommandCapture<'_>,
) -> ToolResultEnvelope {
    let filtered = !metadata.summary.is_empty() || !facts.reported_tests.is_empty();
    let output_preview_limit = output
        .limits
        .max_preview_bytes
        .min(MAX_OUTPUT_PREVIEW_BYTES);
    let failed =
        facts.timed_out || facts.signal.is_some() || facts.exit_code.is_some_and(|code| code != 0);
    let show_end = failed
        && !filtered
        && facts.capture_complete
        && metadata.stored_bytes > output.limits.max_preview_bytes as u64;
    let show_stderr = show_end && facts.stderr_bytes > 0;
    let read_limit = if show_end {
        output_preview_limit.min(output.limits.max_preview_bytes / 2)
    } else if filtered {
        output_preview_limit
    } else {
        output.limits.max_preview_bytes
    }
    .max(1);
    let source_bytes = if show_stderr {
        facts.stderr_bytes
    } else {
        metadata.stored_bytes
    };
    let request = OutputRead {
        offset: if show_end {
            source_bytes.saturating_sub(read_limit as u64)
        } else {
            0
        },
        limit: read_limit,
        search: None,
    };
    let result = if show_stderr {
        read_stderr(output.dir, output.id, request)
    } else {
        read_output(output.dir, output.id, request)
    };
    let chunk = match result {
        Ok(chunk) => chunk,
        Err(error) => return output_error("read", error, metadata.cleanup_incomplete),
    };
    let rendered = render_streams(&chunk.segments);
    let raw_preview = if show_end {
        &rendered[rendered.ceil_char_boundary(rendered.len().saturating_sub(read_limit))..]
    } else {
        rendered.as_str()
    };
    let mut output_preview = if chunk
        .segments
        .iter()
        .all(|segment| segment.stream == OutputStream::Stdout)
    {
        raw_preview
            .strip_prefix("stdout:\n")
            .unwrap_or(raw_preview)
            .to_string()
    } else {
        raw_preview.to_owned()
    };
    let output_preview_truncated =
        show_end || chunk.has_more || output_preview.len() > output_preview_limit;
    truncate_utf8(&mut output_preview, output_preview_limit);
    let mut model_content = if filtered {
        format!(
            "{}\n\nBuild/test summary (selected lines):\n{}",
            summary,
            report_evidence(&metadata.summary, &facts.reported_tests)
        )
    } else if show_end {
        let source = if show_stderr { "stderr" } else { "output" };
        format!("{summary}\n\nEnd of captured {source}:\n{raw_preview}")
    } else {
        let text = if raw_preview.is_empty() {
            "(command produced no output)"
        } else {
            raw_preview
        };
        format!("{summary}\n\n{text}")
    };
    let preview_truncated = !filtered && (show_end || chunk.has_more);
    let reference = render_reference(output.id, metadata, filtered, chunk.lossy_utf8);
    let content_truncated = model_content.len() + reference.len() > output.limits.max_preview_bytes;
    let truncated = !facts.capture_complete || preview_truncated || content_truncated;
    if truncated {
        summary.push_str(", output truncated");
    }
    let suffix = if truncated {
        format!("\n\n(Command output truncated.){reference}")
    } else {
        reference
    };
    fit_content(&mut model_content, &suffix, output.limits.max_preview_bytes);
    facts.lossy_utf8 = chunk.lossy_utf8
        && (output_preview.contains(char::REPLACEMENT_CHARACTER)
            || (!filtered && model_content.contains(char::REPLACEMENT_CHARACTER)));
    facts.output_filtered = filtered;
    facts.preview_truncated = preview_truncated || content_truncated;
    facts.output_preview = output_preview;
    facts.output_preview_truncated = output_preview_truncated;
    ToolResultEnvelope::command(status, summary, model_content, truncated, facts)
}

fn render_script(
    status: &str,
    summary: String,
    facts: CommandExecutionFacts,
    output: CommandCapture<'_>,
    host_bytes: usize,
) -> ToolResultEnvelope {
    // NOTE: An inline byte uses at most four JSON bytes. Reserve half the reply for metadata.
    let bound = output
        .limits
        .max_preview_bytes
        .min(MAX_OUTPUT_PREVIEW_BYTES)
        .min(host_bytes / 8);
    let size = facts.output_ref.bytes;
    let inline = if status == "ok" && facts.capture_complete && size > 0 && size <= bound as u64 {
        match read_output(
            output.dir,
            output.id,
            OutputRead {
                offset: 0,
                limit: size as usize,
                search: None,
            },
        ) {
            Ok(chunk) => Some(chunk.segments),
            Err(error) => return output_error("read", error, facts.cleanup_incomplete),
        }
    } else {
        None
    };
    let mut result = ToolResultEnvelope::command(
        status,
        summary,
        String::new(),
        !facts.capture_complete,
        facts,
    );
    if let Some(segments) = inline {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        for segment in segments {
            let stream = match segment.stream {
                OutputStream::Stdout => &mut stdout,
                OutputStream::Stderr => &mut stderr,
            };
            stream.extend_from_slice(segment.as_bytes());
        }
        result
            .structured
            .as_mut()
            .expect("command has structured facts")["inline_pipes"] =
            json!({"stdout":stdout, "stderr":stderr});
    }
    result
}

fn outcome_summary(request: &CommandRequest, outcome: &CommandOutcome, duration_ms: u64) -> String {
    if outcome.cancelled {
        format!("command cancelled after {duration_ms}ms")
    } else if outcome.timed_out {
        format!(
            "command timed out after {}s",
            request
                .timeout_seconds
                .expect("configured command deadline")
        )
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
