use kuku_client::{HistoryEntry, HistoryKind};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use serde_json::{Map, Value};
use unicode_width::UnicodeWidthStr;

use crate::markup::{render_diff, render_literal};
use crate::text::{cap, shorten, single_line};
use crate::theme::{emphasis, ACCENT, ERROR, MUTED, SUCCESS, WARNING};

use super::{append_bounded, body_inset, capped, render_rows, Row, ToolDetails, ToolView};

const MAX_DETAIL_BYTES: usize = 12 * 1024;
const MAX_LABEL_BYTES: usize = 256;
const MAX_PREVIEW_BYTES: usize = 2048;
const MAX_INVOCATION_ROWS: usize = 2;
const MAX_SUMMARY_ROWS: usize = 2;
const MAX_RESULT_ROWS: usize = 6;
const MAX_FILE_ROWS: usize = 3;
const MAX_WARNING_ROWS: usize = 3;

#[derive(Debug, Clone, Default)]
pub(super) struct Tool {
    name: String,
    invocation: String,
    start_summary: String,
    requested_change: String,
    progress: String,
    state: State,
}

impl Tool {
    pub(super) fn from_entry(entry: &HistoryEntry) -> Self {
        let name = capped(
            entry.details.get("tool").and_then(Value::as_str),
            MAX_LABEL_BYTES,
        );
        if entry.kind == HistoryKind::ToolResult {
            let outcome = Outcome::from_fields(&entry.details, entry.text.as_deref());
            return Self {
                name,
                state: State::Recorded(outcome),
                ..Self::default()
            };
        }

        let args = entry.details.get("args").unwrap_or(&Value::Null);
        let invocation = invocation(&name, args);
        let requested_change = if name == "edit_file" {
            edit_preview(args).unwrap_or_default()
        } else {
            String::new()
        };
        Self {
            name,
            invocation,
            requested_change,
            ..Self::default()
        }
    }

    pub(super) fn merge(&mut self, incoming: Self) {
        if !incoming.name.is_empty() {
            self.name = incoming.name;
        }
        if !incoming.invocation.is_empty() {
            self.invocation = incoming.invocation;
        }
        if !incoming.requested_change.is_empty() {
            self.requested_change = incoming.requested_change;
        }
        if let State::Recorded(outcome) = incoming.state {
            self.progress.clear();
            self.state = State::Recorded(outcome);
        }
    }

    pub(super) fn apply_live(&mut self, event: &Value) -> bool {
        if self.state.outcome().is_some() {
            return false;
        }
        match event["type"].as_str() {
            Some("tool_start") => {
                if let Some(name) = event["tool"].as_str() {
                    self.name = capped(Some(name), MAX_LABEL_BYTES);
                }
                self.start_summary = capped(
                    event["summary"]
                        .as_str()
                        .filter(|summary| *summary != self.name),
                    MAX_DETAIL_BYTES,
                );
                self.state = State::Running;
            }
            Some("tool_output") => {
                let event = &event["event"];
                let Some(text) = event["stdout"]
                    .as_str()
                    .or_else(|| event["stderr"].as_str())
                else {
                    return false;
                };
                self.state = State::Running;
                append_bounded(&mut self.progress, text, MAX_DETAIL_BYTES);
            }
            Some("tool_end") => {
                let Some(fields) = event.as_object() else {
                    return false;
                };
                let outcome = Outcome::from_fields(fields, None);
                if !outcome.content.is_empty() || self.progress == outcome.summary {
                    self.progress.clear();
                }
                self.state = State::Reported(outcome);
            }
            Some(_) | None => return false,
        }
        true
    }

    pub(super) fn clear_live(&mut self) -> bool {
        if matches!(self.state, State::Running | State::Reported(_)) {
            self.state = State::Requested;
            self.progress.clear();
            true
        } else {
            false
        }
    }

    pub(super) fn view(&self) -> ToolView {
        let outcome = self.state.outcome();
        ToolView {
            name: self.name.clone(),
            summary: self.summary().to_owned(),
            invocation: self.invocation().to_owned(),
            details: self.result().to_owned(),
            diff: self.diff().unwrap_or_default().to_owned(),
            output_id: outcome.and_then(|outcome| outcome.output_id.clone()),
        }
    }

    pub(super) fn render(&self, width: usize, details: ToolDetails) -> Vec<Row> {
        let expanded = details == ToolDetails::Expanded;
        let heading_inset = body_inset(width);
        let inset = heading_inset + body_inset(width.saturating_sub(usize::from(heading_inset)));
        let caption = self.file_caption();
        let mut rows = self.render_header(width, heading_inset, inset, caption.as_deref());
        let invocation = self.invocation();
        let summary = self.summary();
        if expanded && caption.is_none() {
            rows.extend(render_preview(
                invocation,
                ACCENT,
                width,
                MAX_INVOCATION_ROWS,
                inset,
            ));
        }
        if caption.is_none() && (!expanded || summary != invocation) {
            let limit = if expanded { MAX_SUMMARY_ROWS } else { 1 };
            rows.extend(render_preview(summary, Color::Reset, width, limit, inset));
        }
        if expanded {
            let outcome = self.state.outcome();
            let command = outcome.and_then(|outcome| outcome.command_preview.as_ref());
            let result = if let Some(diff) = self.diff() {
                render_excerpt(diff, render_diff)
            } else {
                let text = command.map_or_else(
                    || self.result(),
                    |preview| &self.result()[..preview.body_end],
                );
                if text.is_empty() || text == summary || text == invocation {
                    Vec::new()
                } else {
                    render_excerpt(text, |text| render_literal(text, Style::default()))
                }
            };
            let limit = if caption.is_some() {
                MAX_FILE_ROWS
            } else {
                MAX_RESULT_ROWS
            };
            rows.extend(render_rows(result, width, limit, inset));
            if let Some(preview) = command {
                rows.extend(render_preview(
                    &preview.notes,
                    WARNING,
                    width,
                    MAX_WARNING_ROWS,
                    inset,
                ));
            }
            if outcome.is_some_and(|outcome| outcome.output_id.is_some()) {
                rows.extend(render_preview(
                    "Saved output · /tools",
                    ACCENT,
                    width,
                    2,
                    inset,
                ));
            }
        }
        rows.push(Row::plain(Line::default()));
        rows
    }

    fn render_header(
        &self,
        width: usize,
        inset: u16,
        detail_inset: u16,
        caption: Option<&str>,
    ) -> Vec<Row> {
        let columns = width.saturating_sub(usize::from(inset));
        let (status, color) = self.state.label();
        let status = shorten(&single_line(status), 24);
        let exit = self
            .state
            .outcome()
            .and_then(|outcome| outcome.exit_code)
            .map_or_else(String::new, |exit| format!(" · exit {exit}"));
        let status = format!("{status}{exit}");
        let title = match self.name.as_str() {
            "" if caption.is_some() => "Read file".to_owned(),
            "" => "Tool".to_owned(),
            "read_file" => "Read file".to_owned(),
            name => format!("Tool {}", single_line(name)),
        };
        let style = emphasis(ACCENT);
        let status_style = Style::default().fg(color);
        let mut title = Line::styled(shorten(&title, columns), style);
        let status_inline = title.width() + status.width() + 3 <= columns;
        if status_inline {
            title.push_span(Span::styled(format!(" · {status}"), status_style));
        }
        let separate_caption = caption
            .filter(|caption| !status_inline || title.width() + caption.width() + 3 > columns);
        if let Some(caption) = caption.filter(|_| separate_caption.is_none()) {
            title.push_span(Span::styled(format!(" · {caption}"), Style::new()));
        }
        let mut lines = vec![title];
        if !status_inline {
            lines.push(Line::styled(status, status_style));
        }
        let mut rows: Vec<_> = render_rows(lines, width, 4, inset).collect();
        if let Some(caption) = separate_caption {
            rows.extend(render_preview(
                caption,
                Color::Reset,
                width,
                MAX_INVOCATION_ROWS,
                detail_inset,
            ));
        }
        rows
    }

    fn file_caption(&self) -> Option<String> {
        if !matches!(self.name.as_str(), "read_file" | "") {
            return None;
        }
        let path = single_line(self.invocation());
        if path.is_empty() {
            return None;
        }
        let read = self
            .state
            .outcome()
            .and_then(|outcome| outcome.file_read.as_ref());
        if let Some(read) = read {
            return Some(format!("{path} · {}", read.caption()));
        }
        if self.name == "read_file" && self.state.outcome().is_none() {
            Some(path)
        } else {
            None
        }
    }

    fn invocation(&self) -> &str {
        if self.invocation.is_empty() || matches!(self.name.as_str(), "run_command" | "read_file") {
            if let Some(outcome) = self
                .state
                .outcome()
                .filter(|outcome| !outcome.invocation.is_empty())
            {
                return &outcome.invocation;
            }
        }
        if self.invocation.is_empty() {
            &self.start_summary
        } else {
            &self.invocation
        }
    }

    fn summary(&self) -> &str {
        self.state
            .outcome()
            .map_or(self.start_summary.as_str(), |outcome| {
                outcome.summary.as_str()
            })
    }

    fn result(&self) -> &str {
        self.state
            .outcome()
            .map(|outcome| outcome.content.as_str())
            .filter(|content| !content.is_empty())
            .unwrap_or(self.progress.as_str())
    }

    fn diff(&self) -> Option<&str> {
        let outcome = self.state.outcome()?;
        if !outcome.diff.is_empty() {
            Some(&outcome.diff)
        } else if outcome.status == "ok" && !self.requested_change.is_empty() {
            Some(&self.requested_change)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Default)]
enum State {
    #[default]
    Requested,
    Running,
    Reported(Outcome),
    Recorded(Outcome),
}

impl State {
    fn outcome(&self) -> Option<&Outcome> {
        match self {
            Self::Requested | Self::Running => None,
            Self::Reported(outcome) | Self::Recorded(outcome) => Some(outcome),
        }
    }

    fn label(&self) -> (&str, Color) {
        match self {
            Self::Requested => ("requested", MUTED),
            Self::Running => ("running", ACCENT),
            Self::Reported(outcome) | Self::Recorded(outcome) => match outcome.status.as_str() {
                "ok" if outcome.exit_code.is_some_and(|code| code != 0) => ("failed", ERROR),
                "ok" => ("ok", SUCCESS),
                "error" => ("error", ERROR),
                "blocked" | "cancelled" => (&outcome.status, WARNING),
                "" => ("finished", MUTED),
                status => (status, MUTED),
            },
        }
    }
}

#[derive(Debug, Clone)]
struct Outcome {
    status: String,
    summary: String,
    invocation: String,
    content: String,
    diff: String,
    exit_code: Option<i64>,
    output_id: Option<String>,
    command_preview: Option<CommandPreview>,
    file_read: Option<FileRead>,
}

impl Outcome {
    fn from_fields(fields: &Map<String, Value>, fallback_summary: Option<&str>) -> Self {
        let get = |key| fields.get(key).and_then(Value::as_str);
        let structured = fields
            .get("structured")
            .or_else(|| fields.get("result"))
            .unwrap_or(&Value::Null);
        let content = get("model_content");
        let output_id = get("output_id")
            .or_else(|| structured["output_id"].as_str())
            .or_else(|| structured["output_ref"]["id"].as_str());
        let truncated = fields
            .get("truncated")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Self {
            status: capped(get("status"), MAX_LABEL_BYTES),
            summary: capped(get("summary").or(fallback_summary), MAX_DETAIL_BYTES),
            invocation: capped(
                structured["command"]
                    .as_str()
                    .or_else(|| structured["path"].as_str()),
                MAX_DETAIL_BYTES,
            ),
            content: capped(content, MAX_DETAIL_BYTES),
            diff: capped(
                structured["diff"].as_str().or_else(|| get("diff")),
                MAX_DETAIL_BYTES,
            ),
            exit_code: structured["exit_code"].as_i64(),
            output_id: output_id.map(str::to_owned),
            command_preview: CommandPreview::from_result(content, structured, output_id),
            file_read: FileRead::from_result(structured, truncated),
        }
    }
}

#[derive(Debug, Clone)]
struct FileRead {
    start: u64,
    count: u64,
    total: u64,
    cached: bool,
    truncated: bool,
}

impl FileRead {
    fn from_result(structured: &Value, truncated: bool) -> Option<Self> {
        if structured["kind"].as_str() != Some("file_content") {
            return None;
        }
        Some(Self {
            start: structured["start_line"].as_u64()?,
            count: structured["line_count"].as_u64()?,
            total: structured["total_lines"].as_u64()?,
            cached: structured["cached"].as_bool().unwrap_or(false),
            truncated,
        })
    }

    fn caption(&self) -> String {
        let Self {
            start,
            count,
            total,
            cached,
            truncated,
        } = *self;
        let mut caption = match (total, count) {
            (0, _) => "empty file".to_owned(),
            (_, 0) => format!("no lines at {start} of {total}"),
            (_, 1) => format!("line {start} of {total}"),
            _ => {
                let end = start.saturating_add(count - 1);
                format!("lines {start}–{end} of {total}")
            }
        };
        if cached {
            caption.push_str(" · cached");
        }
        if truncated {
            caption.push_str(" · truncated");
        }
        caption
    }
}

#[derive(Debug, Clone)]
struct CommandPreview {
    body_end: usize,
    notes: String,
}

impl CommandPreview {
    fn from_result(
        content: Option<&str>,
        structured: &Value,
        output_id: Option<&str>,
    ) -> Option<Self> {
        if structured["kind"].as_str() != Some("command_result") {
            return None;
        }
        let bytes = structured["output_ref"]["bytes"].as_u64()?;
        let id = output_id?;
        let reference =
            format!("\n\nOutput: {id} ({bytes} captured bytes; read by merged byte offset).");
        let (body, notes) = content?.rsplit_once(&reference)?;
        if notes.contains(['\n', '\r']) || (!notes.is_empty() && !notes.starts_with(' ')) {
            return None;
        }
        Some(Self {
            body_end: cap(body.trim_end_matches('\n'), MAX_DETAIL_BYTES).len(),
            notes: capped(Some(notes.trim_start()), MAX_DETAIL_BYTES),
        })
    }
}

fn invocation(tool: &str, args: &Value) -> String {
    let get = |key| {
        args.get(key)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
    };
    if matches!(tool, "find_files" | "search_text") {
        return match (get("pattern"), get("path")) {
            (Some(pattern), Some(path)) => format!(
                "{} in {}",
                cap(pattern, MAX_DETAIL_BYTES / 2),
                cap(path, MAX_DETAIL_BYTES / 2)
            ),
            (Some(value), None) | (None, Some(value)) => capped(Some(value), MAX_DETAIL_BYTES),
            (None, None) => arguments_json(args),
        };
    }
    let key = match tool {
        "run_command" => Some("command"),
        "edit_file" | "read_file" | "write_file" => Some("path"),
        "fetch_url" => Some("url"),
        _ => None,
    };
    key.and_then(get).map_or_else(
        || arguments_json(args),
        |value| capped(Some(value), MAX_DETAIL_BYTES),
    )
}

fn arguments_json(args: &Value) -> String {
    if args.is_null() || args.as_object().is_some_and(Map::is_empty) {
        return String::new();
    }
    let mut bytes = [0; MAX_DETAIL_BYTES];
    let mut room = bytes.as_mut_slice();
    let incomplete = serde_json::to_writer(&mut room, args).is_err();
    let length = MAX_DETAIL_BYTES - room.len();
    let mut text = String::from_utf8_lossy(&bytes[..length]).into_owned();
    if incomplete {
        text.push('…');
    }
    text
}

fn edit_preview(args: &Value) -> Option<String> {
    let old = cap(args["old_text"].as_str()?, MAX_DETAIL_BYTES / 2);
    let new = cap(args["new_text"].as_str()?, MAX_DETAIL_BYTES / 2);
    let mut preview = String::from("Requested replacement\n");
    for (prefix, text) in [('-', old), ('+', new)] {
        for line in text.lines() {
            preview.push(prefix);
            preview.push_str(line);
            preview.push('\n');
        }
    }
    Some(preview)
}

fn render_excerpt(
    text: &str,
    render: impl FnOnce(&str) -> Vec<Line<'static>>,
) -> Vec<Line<'static>> {
    let excerpt = cap(text, MAX_PREVIEW_BYTES);
    let mut lines = render(excerpt);
    if excerpt.len() < text.len() {
        if let Some(line) = lines.last_mut() {
            line.spans
                .push(Span::styled("…", Style::default().fg(MUTED)));
        }
    }
    lines
}

fn render_preview(
    text: &str,
    color: Color,
    width: usize,
    limit: usize,
    inset: u16,
) -> impl Iterator<Item = Row> {
    let lines = if text.is_empty() {
        Vec::new()
    } else {
        render_excerpt(text, |text| {
            render_literal(text, Style::default().fg(color))
        })
    };
    render_rows(lines, width, limit, inset)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::transcript::Transcript;

    #[test]
    fn command_preview_keeps_stdout_and_warnings_without_the_model_output_reference() {
        let mut transcript = Transcript::default();
        let marker = "Output: saved-run (42 captured bytes; read by merged byte offset).";
        let warning = "Capture quota reached; further bytes were drained and discarded.";
        let raw = format!(
            "stdout:\nOutput: from the process\nstderr:\ntry fewer inputs\n\n\n{marker} {warning}"
        );
        transcript.apply(
            serde_json::from_value(json!({
                "id": 1, "agent": "main", "turn": 1, "type": "tool_result", "tool": "run_command",
                "tool_call_id": "cmd", "status": "error", "summary": "command timed out; output truncated",
                "model_content": raw,
                "structured": {"kind": "command_result", "command": "check", "output_ref": {"id": "saved-run", "bytes": 42}}
            })).unwrap(),
            None,
        );

        let view = transcript.project(96, 18);
        let rendered = view
            .lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("command timed out; output truncated"));
        assert!(rendered.contains("stdout:\n    Output: from the process"));
        assert!(rendered.contains(&format!(
            "stderr:\n    try fewer inputs\n    {warning}\n    Saved output · /tools"
        )));
        assert!(!rendered.contains(marker));
        assert_eq!(raw, transcript.tools()[0].details);
        assert_eq!(
            Some("saved-run"),
            transcript.tools()[0].output_id.as_deref()
        );
    }

    #[test]
    fn custom_tool_arguments_and_output_have_bounded_inline_previews() {
        let mut transcript = Transcript::default();
        let args = "中文".repeat(8_000);
        transcript.apply(
            serde_json::from_value(json!({
                "id": 1, "agent": "main", "turn": 1, "type": "tool_call",
                "tool": "custom_fixture", "tool_call_id": "custom", "args": {"key": args}
            }))
            .unwrap(),
            None,
        );
        transcript.apply(serde_json::from_value(json!({
            "id": 2, "agent": "main", "turn": 1, "type": "tool_result", "tool_call_id": "custom",
            "status": "ok", "model_content": format!("first\u{001b}line\n{}later line", "中\n".repeat(50)),
            "output_id": "retained"
        })).unwrap(), None);
        let view = transcript.project(52, 40);
        let plain = view
            .lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(view.lines.len() <= 14);
        assert!(plain.contains("{\"key\":\"中文"));
        assert!(plain.contains("first␛line"));
        assert!(plain.contains('…'));
        assert!(!plain.contains("later line"));
        assert!(!plain.contains('\u{001b}'));
        let tools = transcript.tools();
        assert!(tools[0].details.ends_with("later line"));
        assert_eq!(Some("retained"), tools[0].output_id.as_deref());
    }

    #[test]
    fn adjacent_reads_share_one_hint_and_show_each_path_with_its_actual_line_range() {
        let mut transcript = Transcript::default();
        let raw = "17\talpha\n18\tbeta\n19\tgamma\n20\tdelta\n21\tepsilon";
        for (first, name, path) in [(1, "read-a", "src/lib.rs"), (4, "read-b", "src/main.rs")] {
            for (id, entry) in (first..).zip([
                json!({"type": "assistant", "text": "", "thinking": "Read this file"}),
                json!({"type": "tool_call", "tool": "read_file", "tool_call_id": name, "args": {"path": format!("./{path}")}}),
                json!({"type": "tool_result", "tool_call_id": name, "status": "ok", "summary": format!("read {path}, lines 17-21 of 30, results truncated"), "model_content": raw, "truncated": true, "structured": {"kind": "file_content", "path": path, "start_line": 17, "line_count": 5, "total_lines": 30, "cached": true}}),
            ]) {
                let mut entry = entry.as_object().unwrap().clone();
                entry.extend([
                    ("id".into(), json!(id)),
                    ("agent".into(), json!("main")),
                    ("turn".into(), json!(7)),
                ]);
                transcript.apply(serde_json::from_value(entry.into()).unwrap(), None);
            }
        }

        let view = transcript.project(84, 24);
        let rendered = view
            .lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(1, rendered.matches("Turn 7").count());
        assert_eq!(1, rendered.matches("src/lib.rs").count());
        assert_eq!(1, rendered.matches("src/main.rs").count());
        assert!(rendered
            .contains("Read file · ok · src/lib.rs · lines 17–21 of 30 · cached · truncated"));
        assert!(rendered.contains("…\n  Read file") && !rendered.contains("Assistant"));
        assert!(rendered.contains("19    gamma") && !rendered.contains("epsilon"));
        assert_eq!(Some("Ctrl+O hide"), view.tool_hint.as_deref());
        assert!(!rendered.contains("Ctrl+O"));
        assert_eq!(raw, transcript.tools()[0].details);

        transcript.toggle_details();
        let collapsed = transcript.project(84, 24);
        assert_eq!(Some("Ctrl+O show"), collapsed.tool_hint.as_deref());
        assert!(!collapsed
            .lines
            .iter()
            .any(|line| line.to_string().contains("alpha")));
        assert!(collapsed
            .lines
            .iter()
            .any(|line| line.to_string().contains("src/lib.rs")));
    }
}
