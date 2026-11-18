use std::borrow::Cow;

use kuku_client::{HistoryEntry, HistoryKind, Receipt};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use serde_json::{Map, Value};
use unicode_width::UnicodeWidthStr;

use crate::markup::{render_diff, render_literal};
use crate::text::{cap, shorten, single_line};
use crate::theme::{emphasis, ACCENT, ERROR, MUTED, SUCCESS, WARNING};

use super::agent::{AgentCard, AgentRequest};
use super::{append_bounded, body_inset, capped, render_rows, Row, ToolDetails, ToolView};

const MAX_DETAIL_BYTES: usize = 12 * 1024;
const MAX_LABEL_BYTES: usize = 256;
const MAX_PREVIEW_BYTES: usize = 2048;
const MAX_INVOCATION_ROWS: usize = 2;
const MAX_SUMMARY_ROWS: usize = 2;
const MAX_RESULT_ROWS: usize = 6;
const MAX_FILE_ROWS: usize = 3;
const MAX_WARNING_ROWS: usize = 3;
const MAX_INSPECTION_ROWS: usize = 12;
const MAX_ARGUMENT_ROWS: usize = 5;

#[derive(Debug, Clone, Default)]
pub(super) struct Tool {
    name: String,
    invocation: String,
    arguments: String,
    start_summary: String,
    requested_change: String,
    progress: String,
    state: State,
    agent_request: Option<AgentRequest>,
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
        let arguments = arguments_json(args);
        let agent_request = AgentRequest::from_call(&name, args);
        let requested_change = if name == "edit_file" {
            edit_preview(args).unwrap_or_default()
        } else {
            String::new()
        };
        Self {
            name,
            invocation,
            arguments,
            requested_change,
            agent_request,
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
        if !incoming.arguments.is_empty() {
            self.arguments = incoming.arguments;
        }
        if !incoming.requested_change.is_empty() {
            self.requested_change = incoming.requested_change;
        }
        if incoming.agent_request.is_some() {
            self.agent_request = incoming.agent_request;
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
                let changed = append_bounded(&mut self.progress, text, MAX_DETAIL_BYTES)
                    || !matches!(self.state, State::Running);
                self.state = State::Running;
                return changed;
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

    pub(super) fn agent_card(&self) -> Option<AgentCard<'_>> {
        let request = self.agent_request.as_ref()?;
        let outcome = self.state.outcome();
        let status = self.state.label().0;
        let status = if status == "ok" { "reported" } else { status };
        let receipt = match &self.state {
            State::Recorded(outcome) => outcome.receipt.as_ref(),
            State::Requested | State::Running | State::Reported(_) => None,
        };
        let error = outcome
            .filter(|outcome| outcome.status != "ok")
            .map_or("", |outcome| {
                if outcome.content.is_empty() {
                    &outcome.summary
                } else {
                    &outcome.content
                }
            });
        Some(request.card(receipt, status, error))
    }

    pub(super) fn group_name(&self) -> Option<&str> {
        let State::Recorded(outcome) = &self.state else {
            return None;
        };
        (!self.name.is_empty()
            && !matches!(
                self.name.as_str(),
                "create_agent" | "send_message" | "list_agents" | "ask_user"
            )
            && outcome.status == "ok"
            && outcome.exit_code.is_none_or(|code| code == 0)
            && !outcome.attention)
            .then_some(self.name.as_str())
    }

    pub(super) fn read_path(&self) -> Option<&str> {
        self.state
            .outcome()
            .filter(|outcome| outcome.file_read.is_some())
            .map(|outcome| outcome.invocation.as_str())
            .filter(|path| !path.is_empty())
    }

    pub(super) fn detail_choice(&self) -> (String, String) {
        let title = self.title();
        let status = self.state.label().0;
        let subject = if self.invocation().is_empty() {
            self.summary()
        } else {
            self.invocation()
        };
        let summary = if subject.is_empty() {
            status.to_owned()
        } else {
            format!(
                "{} · {status}",
                shorten(&single_line(cap(subject, MAX_LABEL_BYTES)), 110)
            )
        };
        (title.into_owned(), summary)
    }

    pub(super) fn render(&self, width: usize, details: ToolDetails) -> Vec<Row> {
        let inspecting = details == ToolDetails::Inspection;
        let expanded = inspecting || details == ToolDetails::Expanded;
        let heading_inset = body_inset(width);
        let inset = heading_inset + body_inset(width.saturating_sub(usize::from(heading_inset)));
        let caption = self.file_caption();
        if details == ToolDetails::GroupMember {
            let target =
                caption.unwrap_or_else(|| single_line(cap(self.invocation(), MAX_LABEL_BYTES)));
            let columns = width.saturating_sub(usize::from(heading_inset));
            let mut label = self.title().into_owned();
            if !target.is_empty() {
                label.push_str(&format!(" · {target}"));
            }
            let caption = shorten(&label, columns);
            let mut rows: Vec<_> = render_rows(
                vec![Line::styled(caption, emphasis(ACCENT))],
                width,
                1,
                heading_inset,
            )
            .collect();
            rows.push(Row::plain(Line::default()));
            return rows;
        }
        let mut rows = self.render_header(width, heading_inset, inset, caption.as_deref());
        if !inspecting && self.is_missing_read() {
            rows.extend(render_preview(
                self.result(),
                ERROR,
                width,
                MAX_SUMMARY_ROWS,
                inset,
            ));
            rows.push(Row::plain(Line::default()));
            return rows;
        }
        let invocation = self.invocation();
        let summary = self.summary();
        if expanded && caption.is_none() && (!inspecting || invocation != self.arguments) {
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
        if inspecting {
            let name = single_line(&self.name);
            if self.arguments.is_empty() {
                if !name.is_empty() {
                    rows.extend(render_preview(
                        &format!("Tool ID · {name}"),
                        MUTED,
                        width,
                        2,
                        inset,
                    ));
                }
            } else {
                let label = if name.is_empty() {
                    "Requested arguments".to_owned()
                } else {
                    format!("Requested arguments · {name}")
                };
                rows.extend(render_preview(&label, MUTED, width, 2, inset));
                rows.extend(render_preview(
                    &self.arguments,
                    Color::Reset,
                    width,
                    MAX_ARGUMENT_ROWS,
                    inset,
                ));
            }
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
            let limit = if inspecting {
                MAX_INSPECTION_ROWS
            } else if caption.is_some() {
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

    fn title(&self) -> Cow<'_, str> {
        if self.name.is_empty() && self.read_path().is_some() {
            tool_title("read_file")
        } else {
            tool_title(&self.name)
        }
    }

    fn is_missing_read(&self) -> bool {
        let path = self.invocation.as_str();
        self.name == "read_file"
            && self
                .state
                .outcome()
                .is_some_and(|outcome| outcome.status == "error")
            && !path.is_empty()
            && self.summary().strip_prefix("failed: path not found: ") == Some(path)
            && self.result().strip_prefix("path does not exist: ") == Some(path)
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
        let title = self.title();
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
        if !self.invocation.is_empty() {
            &self.invocation
        } else if !self.arguments.is_empty() {
            &self.arguments
        } else {
            &self.start_summary
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

pub(super) fn tool_title(name: &str) -> Cow<'_, str> {
    Cow::Borrowed(match name {
        "" => "Tool",
        "read_file" => "Read file",
        "find_files" => "Find files",
        "search_text" => "Search text",
        "write_file" => "Write file",
        "edit_file" => "Edit file",
        "run_command" => "Run command",
        "read_output" => "Read saved output",
        "fetch_url" => "Download URL",
        "fetch_web" => "Fetch web page",
        "query_session" => "Query Session",
        "remember_memory" => "Save memory",
        "forget_memory" => "Remove memory",
        "use_skill" => "Use skill",
        "list_skills" => "List skills",
        "search_skills" => "Search skills",
        "agent" => "Run Agent",
        "create_agent" => "Create Agent",
        "list_agents" => "List Agents",
        "send_message" => "Send message",
        "ask_user" => "Ask user",
        name => return Cow::Owned(single_line(name)),
    })
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
                "ok" if outcome.attention => ("ok with warnings", WARNING),
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
    receipt: Option<Receipt>,
    attention: bool,
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
        let attention = [
            "cleanup_incomplete",
            "quota_truncated",
            "output_persistence_failed",
            "timed_out",
            "cancelled",
        ]
        .into_iter()
        .any(|key| structured[key].as_bool() == Some(true))
            || structured["capture_complete"].as_bool() == Some(false)
            || structured["stderr_lines"]
                .as_u64()
                .is_some_and(|lines| lines > 0);
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
            receipt: (get("status") == Some("ok"))
                .then(|| agent_receipt(structured))
                .flatten(),
            attention,
        }
    }
}

fn agent_receipt(structured: &Value) -> Option<Receipt> {
    let agent = structured["agent"].as_str()?;
    let command_id = structured["command_id"].as_str()?;
    let event_id = structured["event_id"].as_u64()?;
    if agent.is_empty() || command_id.is_empty() || event_id == 0 {
        return None;
    }
    Some(Receipt {
        agent: agent.to_owned(),
        command_id: command_id.to_owned(),
        event_id,
    })
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
            (None, None) => String::new(),
        };
    }
    let key = match tool {
        "run_command" => Some("command"),
        "edit_file" | "read_file" | "write_file" => Some("path"),
        "fetch_url" | "fetch_web" => Some("url"),
        _ => None,
    };
    capped(key.and_then(get), MAX_DETAIL_BYTES)
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

    use super::{Tool, MAX_DETAIL_BYTES};

    #[test]
    fn late_live_output_does_not_change_a_full_preview() {
        let mut tool = Tool::default();
        let first = json!({"type":"tool_output", "event":{"stdout":"中".repeat(MAX_DETAIL_BYTES)}});
        assert!(tool.apply_live(&first));
        let details = tool.view().details;
        assert!(details.ends_with("Display limit reached"));
        let extra = json!({"type":"tool_output", "event":{"stdout":"Late output"}});
        assert!(!tool.apply_live(&extra));
        assert_eq!(details, tool.view().details);
    }

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
    fn reads_separated_by_a_turn_boundary_show_each_path_with_its_actual_line_range() {
        let mut transcript = Transcript::default();
        let raw = "17\talpha\n18\tbeta\n19\tgamma\n20\tdelta\n21\tepsilon";
        for (first, name, path, turn, number) in [
            (1, "read-a", "src/lib.rs", 7, 3),
            (4, "read-b", "src/main.rs", 8, 4),
        ] {
            for (id, entry) in (first..).zip([
                json!({"type": "assistant", "text": "", "thinking": "Read this file"}),
                json!({"type": "tool_call", "tool": "read_file", "tool_call_id": name, "args": {"path": format!("./{path}")}}),
                json!({"type": "tool_result", "tool_call_id": name, "status": "ok", "summary": format!("read {path}, lines 17-21 of 30, results truncated"), "model_content": raw, "truncated": true, "structured": {"kind": "file_content", "path": path, "start_line": 17, "line_count": 5, "total_lines": 30, "cached": true}}),
            ]) {
                let mut entry = entry.as_object().unwrap().clone();
                entry.extend([
                    ("id".into(), json!(id)),
                    ("agent".into(), json!("main")),
                    ("turn".into(), json!(turn)),
                    ("turn_number".into(), json!(number)),
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
        assert_eq!(1, rendered.matches("Turn 3").count());
        assert_eq!(1, rendered.matches("Turn 4").count());
        assert_eq!(1, rendered.matches("src/lib.rs").count());
        assert_eq!(1, rendered.matches("src/main.rs").count());
        assert!(rendered
            .contains("Read file · ok · src/lib.rs · lines 17–21 of 30 · cached · truncated"));
        assert!(rendered.contains("…") && !rendered.contains("Assistant"));
        assert!(rendered.contains("19    gamma") && !rendered.contains("epsilon"));
        assert_eq!(Some("Ctrl+O hide · /details"), view.tool_hint.as_deref());
        assert!(!rendered.contains("Ctrl+O"));
        assert_eq!(raw, transcript.tools()[0].details);

        transcript.toggle_details();
        let collapsed = transcript.project(84, 24);
        assert_eq!(
            Some("Ctrl+O show · /details"),
            collapsed.tool_hint.as_deref()
        );
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
