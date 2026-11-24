#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use std::time::Duration;

use serde_json::{Value, json};

use crate::execution::{
    CommandExecutionFacts, CommandInvocation, CommandOutputSummary, CommandResultKind,
    CommandStdin, CommandTermination, OutputReference, ReportedTests,
};
use crate::tool::ToolResultEnvelope;

use super::encoding::display_text;
use super::output::{
    CaptureMetadata, FinishedCapture, OutputChunk, OutputSegment, read_output_tails,
};
use super::summary::Summary;
use super::{
    CommandCapture, CommandOutcome, CommandRequest, CommandView, ModelOutputAccess, OutputRead,
    OutputStream, output_error, read_output,
};

const MAX_OUTPUT_PREVIEW_BYTES: usize = 2048;
const MAX_MIXED_STDOUT_TAIL_BYTES: usize = 384;
const SHORT_OUTPUT_LINES: u64 = 40;
const SHAPE_SAMPLE_BYTES: usize = 96;
const SHAPE_FIELDS: usize = 6;
const SHAPE_FIELD_BYTES: usize = 24;
const MIN_UNQUOTED_SAMPLE_COLUMNS: usize = 3;
const STDOUT_TAIL_HEADING: &str = "End of captured stdout:\n";
const STDERR_TAIL_HEADING: &str = "End of captured stderr:\n";

#[derive(Debug, PartialEq, Eq)]
enum LineFormat {
    JsonObject,
    Delimited { separator: char, columns: usize },
}

pub(super) fn render_command_result(
    request: &CommandRequest,
    outcome: CommandOutcome,
    capture: FinishedCapture,
    duration: Duration,
    output: CommandCapture<'_>,
) -> ToolResultEnvelope {
    let FinishedCapture {
        metadata,
        reports,
        summary: rule,
    } = capture;
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
        output_summary: None,
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
        CommandView::Model(access) => {
            render_model(status, summary, facts, &metadata, output, access, rule)
        }
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
    access: ModelOutputAccess,
    rule: Option<Summary>,
) -> ToolResultEnvelope {
    let output_preview_limit = output
        .limits
        .max_preview_bytes
        .min(MAX_OUTPUT_PREVIEW_BYTES);
    let failed =
        facts.timed_out || facts.signal.is_some() || facts.exit_code.is_some_and(|code| code != 0);
    let abbreviated = access != ModelOutputAccess::Unavailable
        && (metadata.stored_bytes > output_preview_limit as u64
            || facts.stdout_lines.saturating_add(facts.stderr_lines) > SHORT_OUTPUT_LINES);
    if abbreviated {
        if let Some(rule) = rule {
            if let Some((preview, content)) = render_rule(
                &summary,
                &rule,
                output.id,
                metadata,
                output_preview_limit,
                access,
            ) {
                summary.push_str(", summarized output");
                facts.output_filtered = true;
                facts.output_summary = Some(rule.source);
                facts.preview_truncated = true;
                facts.output_preview = preview;
                facts.output_preview_truncated = true;
                return ToolResultEnvelope::command(status, summary, content, true, facts);
            }
        }
    }
    let filtered = (abbreviated || access == ModelOutputAccess::Unavailable)
        && (!metadata.summary.is_empty() || !facts.reported_tests.is_empty());
    let show_end = failed
        && facts.capture_complete
        && (abbreviated || metadata.stored_bytes > output.limits.max_preview_bytes as u64);
    if show_end {
        return render_failure_tail(
            status,
            summary,
            facts,
            metadata,
            output,
            abbreviated.then_some(access),
            filtered,
        );
    }
    let read_limit = if filtered || abbreviated {
        output_preview_limit
    } else {
        output.limits.max_preview_bytes
    }
    .max(1);
    let request = OutputRead {
        offset: 0,
        limit: read_limit,
        search: None,
    };
    let chunk = match read_output(output.dir, output.id, request) {
        Ok(chunk) => chunk,
        Err(error) => return output_error("read", error, metadata.cleanup_incomplete),
    };
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
        raw_preview.to_owned()
    };
    let output_preview_truncated = chunk.has_more || output_preview.len() > output_preview_limit;
    truncate_utf8(&mut output_preview, output_preview_limit);
    let mut body = String::new();
    let include_raw = !filtered && !abbreviated;
    if include_raw {
        let text = if raw_preview.is_empty() {
            "(command produced no output)"
        } else {
            &raw_preview
        };
        body.push_str("\n\n");
        body.push_str(text);
    } else if abbreviated && !filtered {
        body.push_str("\n\nOutput shape from the observed prefix:");
        append_output_shape(&mut body, &chunk, &facts, access);
    }
    if filtered {
        append_report_evidence(&mut body, metadata, &facts, "");
    }
    if abbreviated {
        append_read_guidance(&mut body, access);
    }
    if !facts.capture_complete {
        summary.push_str(", capture incomplete");
    }
    let reference = render_reference(
        output.id,
        metadata,
        filtered,
        chunk.lossy_utf8 && include_raw,
    );
    let preview_truncated = abbreviated
        || (include_raw && chunk.has_more)
        || summary.len() + body.len() + reference.len() > output.limits.max_preview_bytes;
    if preview_truncated {
        summary.push_str(", preview truncated");
    }
    let truncated = !facts.capture_complete || preview_truncated;
    let mut model_content = format!("{summary}{body}");
    fit_content(
        &mut model_content,
        &reference,
        output.limits.max_preview_bytes,
    );
    facts.lossy_utf8 = chunk.lossy_utf8
        && (output_preview.contains(char::REPLACEMENT_CHARACTER)
            || (include_raw && model_content.contains(char::REPLACEMENT_CHARACTER)));
    facts.output_filtered = filtered;
    facts.preview_truncated = preview_truncated;
    facts.output_preview = output_preview;
    facts.output_preview_truncated = output_preview_truncated;
    ToolResultEnvelope::command(status, summary, model_content, truncated, facts)
}

fn render_failure_tail(
    status: &str,
    mut summary: String,
    mut facts: CommandExecutionFacts,
    metadata: &CaptureMetadata,
    output: CommandCapture<'_>,
    guidance: Option<ModelOutputAccess>,
    filtered: bool,
) -> ToolResultEnvelope {
    summary.push_str(", preview truncated");
    let limit = output.limits.max_preview_bytes;
    let tail_budget = |reference: &str| {
        MAX_OUTPUT_PREVIEW_BYTES
            .min(limit / 2)
            .min(limit.saturating_sub(summary.len() + reference.len() + 2))
    };
    let mut reference = render_reference(output.id, metadata, filtered, false);
    let (stdout_limit, stderr_limit) = failure_tail_limits(metadata, tail_budget(&reference));
    let [stdout, stderr] =
        match read_output_tails(output.dir, output.id, stdout_limit, stderr_limit) {
            Ok(tails) => tails,
            Err(error) => return output_error("read", error, metadata.cleanup_incomplete),
        };
    let decoded_lossy = stdout.lossy_utf8 || stderr.lossy_utf8;
    if decoded_lossy {
        reference = render_reference(output.id, metadata, filtered, true);
    }
    let (stdout_limit, stderr_limit) = failure_tail_limits(metadata, tail_budget(&reference));
    let mut preview = String::new();
    for (heading, tail, budget) in [
        (STDERR_TAIL_HEADING, stderr, stderr_limit),
        (STDOUT_TAIL_HEADING, stdout, stdout_limit),
    ] {
        if budget == 0 || tail.text.is_empty() {
            continue;
        }
        if !preview.is_empty() {
            preview.push_str("\n\n");
        }
        preview.push_str(heading);
        let start = tail
            .text
            .ceil_char_boundary(tail.text.len().saturating_sub(budget));
        preview.push_str(&tail.text[start..]);
    }
    let mut model_content = summary.clone();
    if !preview.is_empty() {
        model_content.push_str("\n\n");
        model_content.push_str(&preview);
    }
    if filtered {
        append_report_evidence(&mut model_content, metadata, &facts, &preview);
    }
    if let Some(access) = guidance {
        append_read_guidance(&mut model_content, access);
    }
    facts.lossy_utf8 = decoded_lossy && preview.contains(char::REPLACEMENT_CHARACTER);
    if decoded_lossy && !facts.lossy_utf8 {
        reference = render_reference(output.id, metadata, filtered, false);
    }
    fit_content(&mut model_content, &reference, limit);
    facts.output_filtered = filtered;
    facts.preview_truncated = true;
    facts.output_preview = preview;
    facts.output_preview_truncated = true;
    ToolResultEnvelope::command(status, summary, model_content, true, facts)
}

fn failure_tail_limits(metadata: &CaptureMetadata, budget: usize) -> (usize, usize) {
    let both_headings = STDOUT_TAIL_HEADING.len() + STDERR_TAIL_HEADING.len() + 2;
    let stdout = metadata.stdout_bytes as usize;
    let stderr = metadata.stderr_bytes as usize;
    match (stdout, stderr) {
        (_, 0) => (
            stdout.min(budget.saturating_sub(STDOUT_TAIL_HEADING.len())),
            0,
        ),
        (0, _) => (
            0,
            stderr.min(budget.saturating_sub(STDERR_TAIL_HEADING.len())),
        ),
        (_, _) if budget < both_headings + 2 => (
            0,
            stderr.min(budget.saturating_sub(STDERR_TAIL_HEADING.len())),
        ),
        (_, _) => {
            let bytes = budget - both_headings;
            let stdout_cap = stdout.min(MAX_MIXED_STDOUT_TAIL_BYTES);
            let stdout_share = stdout_cap.min((bytes / 3).max(1));
            let stderr_share = stderr.min(bytes - stdout_share);
            (stdout_cap.min(bytes - stderr_share), stderr_share)
        }
    }
}

fn append_read_guidance(body: &mut String, access: ModelOutputAccess) {
    body.push_str("\nRead the relevant range with read_output({id, offset, limit}) using the Output ID below.");
    if access == ModelOutputAccess::Script {
        body.push_str(
            "\nTo locate a known literal first, call tools.search_output({id, text}) in run_js.",
        );
    }
}

fn append_report_evidence(
    body: &mut String,
    metadata: &CaptureMetadata,
    facts: &CommandExecutionFacts,
    excerpt: &str,
) {
    let evidence = report_evidence(&metadata.summary, &facts.reported_tests, excerpt);
    if !evidence.is_empty() {
        body.push_str("\n\nBuild/test summary (selected lines):\n");
        body.push_str(&evidence);
    }
}

fn render_rule(
    outcome: &str,
    summary: &Summary,
    id: &str,
    capture: &CaptureMetadata,
    limit: usize,
    access: ModelOutputAccess,
) -> Option<(String, String)> {
    let CommandOutputSummary::Rule { rule, ranges } = &summary.source;
    let first = ranges.first()?;
    let preview = format!("Output summary ({rule}):\n{}", summary.text);
    let start = first.offset;
    let bytes = first.end_offset.checked_sub(start)?.min(4096);
    let mut content = format!(
        "{outcome}, summarized output\n\n{preview}\n\nFor the first selected range, call read_output({{id:{id:?}, offset:{start}, limit:{bytes}}})."
    );
    if access == ModelOutputAccess::Script {
        content.push(' ');
        content.push_str(summary.script_hint.unwrap_or("To locate a literal in the full capture, call tools.search_output({id, text}) in run_js."));
    }
    content.push_str(&render_reference(id, capture, false, false));
    (content.len() <= limit && (content.len() as u64) < capture.stored_bytes)
        .then_some((preview, content))
}

fn append_output_shape(
    body: &mut String,
    chunk: &OutputChunk,
    facts: &CommandExecutionFacts,
    access: ModelOutputAccess,
) {
    for (stream, label, bytes) in [
        (OutputStream::Stdout, "stdout", facts.stdout_bytes),
        (OutputStream::Stderr, "stderr", facts.stderr_bytes),
    ] {
        if bytes == 0 {
            continue;
        }
        body.push_str(&format!("\n{label}: {bytes} observed bytes"));
        if let Some((description, format)) =
            stream_shape(chunk, stream, bytes, facts.capture_complete)
        {
            body.push_str(". ");
            body.push_str(&description);
            if let (ModelOutputAccess::Script, Some(format)) = (access, format) {
                body.push_str(&format!("\nFor {label} records in run_js, iterate tools.output_lines({{id, stream:{label:?}}}). Check line.truncated and line.lossy_utf8 before "));
                match format {
                    LineFormat::JsonObject => body.push_str("JSON.parse(line.text)."),
                    LineFormat::Delimited { separator, columns } => {
                        let separator = json!(separator);
                        body.push_str(&format!("using line.text. A row that follows the sample has {columns} fields after line.text.split({separator})."));
                    }
                }
            }
        }
    }
}

fn stream_shape(
    chunk: &OutputChunk,
    stream: OutputStream,
    observed_bytes: u64,
    capture_complete: bool,
) -> Option<(String, Option<LineFormat>)> {
    let bytes: Vec<_> = chunk
        .segments
        .iter()
        .filter(|segment| segment.stream == stream)
        .flat_map(|segment| segment.as_bytes().iter().copied())
        .collect();
    let complete = capture_complete && bytes.len() as u64 == observed_bytes;
    let prefix = match std::str::from_utf8(&bytes) {
        Err(error) if !complete && error.error_len().is_none() => &bytes[..error.valid_up_to()],
        _ => &bytes[..],
    };
    let (text, lossy) = display_text(prefix, true, complete);
    (!lossy && !text.is_empty()).then(|| output_shape(&text, complete))
}

fn output_shape(prefix: &str, complete: bool) -> (String, Option<LineFormat>) {
    if complete {
        match serde_json::from_str::<Value>(prefix) {
            Ok(Value::Object(fields)) => return (json_object_shape("JSON object", &fields), None),
            Ok(Value::Array(items)) => return (format!("JSON array, {} items", items.len()), None),
            _ => {}
        }
    } else if let Some(shape) = json_prefix_shape(prefix) {
        return (shape.into(), None);
    }
    let mut lines = prefix
        .split_inclusive('\n')
        .filter(|line| complete || line.ends_with('\n'))
        .map(|line| line.trim_end_matches(['\r', '\n']))
        .filter(|line| !line.trim().is_empty());
    let (Some(first), Some(second)) = (lines.next(), lines.next()) else {
        return ("text".into(), None);
    };
    if let (Ok(Value::Object(fields)), Ok(Value::Object(_))) =
        (serde_json::from_str(first), serde_json::from_str(second))
    {
        return (
            json_object_shape("first two lines are JSON objects", &fields),
            Some(LineFormat::JsonObject),
        );
    }
    let columns = first.split('\t').count();
    if columns > 1 && second.split('\t').count() == columns {
        let unquoted = !first.contains('"') && !second.contains('"');
        return (
            format!("{columns} tab-separated fields in each of the first two lines"),
            unquoted.then_some(LineFormat::Delimited {
                separator: '\t',
                columns,
            }),
        );
    }
    let samples = line_samples([first, second]);
    if let Some(third) = lines.next() {
        if let Some((separator, label, columns)) = delimited_sample([first, second, third]) {
            return (
                format!(
                    "{columns} {label}-separated fields in each of the first three nonblank lines, sample {samples}"
                ),
                Some(LineFormat::Delimited { separator, columns }),
            );
        }
    }
    (
        format!("multiline text, first two nonblank lines {samples}"),
        None,
    )
}

fn json_prefix_shape(prefix: &str) -> Option<&'static str> {
    let prefix = prefix.trim_start();
    let shape = if let Some(fields) = prefix.strip_prefix('{') {
        let mut key = serde_json::Deserializer::from_str(fields).into_iter::<String>();
        key.next()?.ok()?;
        fields[key.byte_offset()..]
            .trim_start()
            .starts_with(':')
            .then_some("likely JSON object (bounded prefix)")?
    } else {
        let items = prefix.strip_prefix('[')?.trim_start();
        if items.len() < 2
            || !(items.starts_with([
                '{', '[', '"', '-', '0', '1', '2', '3', '4', '5', '6', '7', '8', '9',
            ]) || ["true", "false", "null"]
                .iter()
                .any(|token| items.starts_with(token)))
        {
            return None;
        }
        "likely JSON array (bounded prefix)"
    };
    serde_json::from_str::<serde::de::IgnoredAny>(prefix)
        .err()
        .filter(serde_json::Error::is_eof)
        .map(|_| shape)
}

fn delimited_sample(lines: [&str; 3]) -> Option<(char, &'static str, usize)> {
    if lines.iter().any(|line| line.contains('"')) {
        return None;
    }
    let mut matching =
        [('|', "pipe"), (',', "comma")]
            .into_iter()
            .filter_map(|(separator, name)| {
                let columns = lines[0].split(separator).count();
                (columns >= MIN_UNQUOTED_SAMPLE_COLUMNS
                    && lines[1..]
                        .iter()
                        .all(|line| line.split(separator).count() == columns))
                .then_some((separator, name, columns))
            });
    let shape = matching.next()?;
    matching.next().is_none().then_some(shape)
}

fn line_samples(lines: [&str; 2]) -> Value {
    json!(lines.map(|line| {
        let end = line.floor_char_boundary(line.len().min(SHAPE_SAMPLE_BYTES / 2));
        let suffix = if end < line.len() { "…" } else { "" };
        format!("{}{suffix}", &line[..end])
    }))
}

fn json_object_shape(kind: &str, fields: &serde_json::Map<String, Value>) -> String {
    let keys: Vec<_> = fields
        .keys()
        .filter(|key| key.len() <= SHAPE_FIELD_BYTES)
        .take(SHAPE_FIELDS)
        .collect();
    format!(
        "{kind}, {} fields, sample keys {}",
        fields.len(),
        json!(keys)
    )
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
        let process = match &request.invocation {
            CommandInvocation::Program { .. } => "command",
            CommandInvocation::Shell { .. } => "shell",
        };
        format!(
            "{process} exited {} in {duration_ms}ms",
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

fn report_evidence(summary: &[String], reports: &[ReportedTests], excerpt: &str) -> String {
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
    lines.retain(|line| !excerpt.lines().any(|raw| raw.trim() == line));
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
    if lossy {
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

#[cfg(test)]
mod tests {
    use super::super::output::{OutputLimits, OutputWriter};
    use super::{LineFormat, OutputRead, OutputStream, output_shape, read_output, stream_shape};

    #[test]
    fn bounded_json_shape_requires_a_plausible_document_prefix() {
        for (prefix, expected) in [
            (
                r#"{"meta":{"total":800},"items":[{"payload":"#,
                "likely JSON object (bounded prefix)",
            ),
            (r#"[{"id":1},{"id":2"#, "likely JSON array (bounded prefix)"),
            ("{build started", "text"),
            ("[INFO] first line", "text"),
            ("{\"log message:", "text"),
        ] {
            assert_eq!(
                output_shape(prefix, false),
                (expected.into(), None),
                "{prefix}"
            );
        }
        assert_eq!(
            output_shape("{\"id\":1}\n{\"id\":2}\na partial line", false).1,
            Some(LineFormat::JsonObject)
        );
    }

    #[tokio::test]
    async fn output_shape_rejoins_streams_before_decoding_and_sampling() {
        let directory = tempfile::tempdir().unwrap();
        let mut writer = OutputWriter::create(directory.path(), "mixed", OutputLimits::default())
            .await
            .unwrap();
        let leading = b"\x1b[32m{\"name\":\"\xe4";
        let trailing = b"\xbd\xa0\"}\x1b[0m\n{\"name\":\"second\"}\n\xe4\xb8\xad";
        let diagnostic = b"\xff\x01\n";
        writer.write(OutputStream::Stdout, leading).await.unwrap();
        writer
            .write(OutputStream::Stderr, diagnostic)
            .await
            .unwrap();
        writer.write(OutputStream::Stdout, trailing).await.unwrap();
        writer.finish(false).await.unwrap();
        let stdout_bytes = leading.len() + trailing.len();
        let output = read_output(
            directory.path(),
            "mixed",
            OutputRead {
                limit: stdout_bytes + diagnostic.len() - 2,
                ..OutputRead::default()
            },
        )
        .unwrap();

        assert_eq!(
            Some((
                "first two lines are JSON objects, 1 fields, sample keys [\"name\"]".into(),
                Some(LineFormat::JsonObject),
            )),
            stream_shape(&output, OutputStream::Stdout, stdout_bytes as u64, true),
        );
        assert_eq!(
            None,
            stream_shape(&output, OutputStream::Stderr, diagnostic.len() as u64, true),
        );
    }

    #[test]
    fn multiline_shape_samples_two_complete_lines() {
        assert_eq!(
            (
                r#"multiline text, first two nonblank lines ["project/","project/src/lib.rs"]"#
                    .into(),
                None,
            ),
            output_shape("project/\n\nproject/src/lib.rs\nproject/src/partial", false),
        );
    }

    #[test]
    fn delimiter_shape_uses_only_complete_unquoted_samples() {
        for (text, separator, description) in [
            (
                "one|two|three\na|b|c\nx|y|z\npartial",
                '|',
                "pipe-separated",
            ),
            ("1,2,3\n4,5,6\n7,8,9\npartial", ',', "comma-separated"),
        ] {
            let (shape, format) = output_shape(text, false);
            assert!(shape.contains(description));
            assert_eq!(
                Some(LineFormat::Delimited {
                    separator,
                    columns: 3
                }),
                format
            );
        }
        for text in [
            "a,b,c\nd,e,f\ng,h",
            "a,b,c\n\"quoted,value\",z\ng,h,i\n",
            "a|b|c,d,e\na|b|c,d,e\na|b|c,d,e\n",
        ] {
            assert_eq!(None, output_shape(text, false).1);
        }
    }
}
