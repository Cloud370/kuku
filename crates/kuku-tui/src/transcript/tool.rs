use std::borrow::Cow;

use kuku_client::{HistoryEntry, HistoryKind, Receipt};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use serde_json::{Map, Value};
use unicode_width::UnicodeWidthStr;

use crate::markup::{render_diff, render_literal};
use crate::text::{cap, shorten, single_line};
use crate::theme::{emphasis, panel, ACCENT, ERROR, MODEL, MUTED, SUCCESS, SURFACE_TEXT, WARNING};

use super::agent::{AgentCard, AgentRequest};
use super::{append_bounded, body_inset, capped, render_rows, Row, ToolDetails, ToolView};

mod command;
mod file_change;
mod session;

use command::CommandPreview;
use file_change::FileChange;
use session::{is_session_read, query_target, Observation};

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
const MAX_CHANGE_ROWS: usize = 4;
const MAX_OBSERVATION_ROWS: usize = 2;

#[derive(Debug, Clone, Default)]
pub(super) struct Tool {
    name: String,
    invocation: String,
    arguments: String,
    start_summary: String,
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
        Self {
            name,
            invocation,
            arguments,
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
        (matches!(
            self.name.as_str(),
            "read_file" | "find_files" | "search_text" | "list_agents" | "query_session"
        ) && outcome.status == "ok"
            && outcome.exit_code.is_none_or(|code| code == 0)
            && !outcome.attention
            && !self.observation().is_some_and(|value| value.attention))
        .then_some(self.name.as_str())
    }

    pub(super) fn checks_delegation(&self, sender: &str, recipient: &str) -> bool {
        self.group_name() == Some("list_agents")
            && self
                .observation()
                .is_some_and(|item| item.checks_delegation(sender, recipient))
    }

    pub(super) fn agent_listing(&self) -> Option<(&str, &str)> {
        let observation = self
            .observation()
            .filter(|value| value.is_for("list_agents"))?;
        Some((&observation.caption, &observation.detail))
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
        if let Some(caption) = self.session_caption() {
            let detail = self.session_detail();
            let summary = join_details([caption, detail.as_str()]);
            return (title.into_owned(), shorten(&summary, 110));
        }
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
            let target = self.session_caption().map_or_else(
                || caption.unwrap_or_else(|| single_line(cap(self.invocation(), MAX_LABEL_BYTES))),
                |summary| join_details([self.invocation.as_str(), summary]),
            );
            let columns = width.saturating_sub(usize::from(heading_inset));
            let mut label = self.title().into_owned();
            if !target.is_empty() {
                label.push_str(&format!(" · {target}"));
            }
            let caption = shorten(&label, columns);
            let rows = render_rows(
                vec![Line::styled(caption, emphasis(ACCENT))],
                width,
                1,
                heading_inset,
            )
            .collect();
            return finish_card(rows);
        }
        if let Some(change) = self.file_change() {
            return self.render_change(change, width, inset, details);
        }
        if !inspecting {
            if let Some(caption) = self.session_caption() {
                let mut rows = self.render_header(width, heading_inset, inset, Some(caption));
                let color = if self.observation().is_some_and(|value| value.attention) {
                    WARNING
                } else {
                    SURFACE_TEXT
                };
                rows.extend(render_preview(
                    &self.session_detail(),
                    color,
                    width,
                    MAX_OBSERVATION_ROWS,
                    inset,
                ));
                return finish_card(rows);
            }
        }
        if self.name == "list_agents"
            && !self.invocation.is_empty()
            && self.state.outcome().is_none()
            && !inspecting
        {
            let label = format!("Waiting for {}", single_line(&self.invocation));
            return finish_card(
                render_rows(
                    vec![Line::styled(
                        shorten(&label, width.saturating_sub(usize::from(heading_inset))),
                        emphasis(MODEL),
                    )],
                    width,
                    1,
                    heading_inset,
                )
                .collect(),
            );
        }
        let outcome = self.state.outcome();
        let command = outcome.and_then(|outcome| outcome.command_preview.as_ref());
        let concise = !inspecting
            && command.is_some_and(|preview| preview.complete && preview.notes.is_empty())
            && outcome.is_some_and(|outcome| {
                outcome.status == "ok"
                    && outcome
                        .exit_code
                        .is_some_and(|code| code != 0 || !outcome.attention)
            });
        let mut rows = self.render_header(width, heading_inset, inset, caption.as_deref());
        if !inspecting && self.is_missing_read() {
            rows.extend(render_preview(
                self.result(),
                ERROR,
                width,
                MAX_SUMMARY_ROWS,
                inset,
            ));
            return finish_card(rows);
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
        if !concise && caption.is_none() && (!expanded || summary != invocation) {
            let limit = if expanded { MAX_SUMMARY_ROWS } else { 1 };
            rows.extend(render_preview(summary, SURFACE_TEXT, width, limit, inset));
        }
        if inspecting {
            self.render_arguments(&mut rows, width, inset);
            if self.session_caption().is_some() && !self.result().is_empty() {
                rows.extend(render_preview(
                    "Raw result · /tools",
                    MUTED,
                    width,
                    1,
                    inset,
                ));
            }
        }
        if expanded {
            let result = if let Some(diff) = self.diff() {
                render_excerpt(diff, render_diff)
            } else {
                let text = command.map_or_else(
                    || self.result(),
                    |preview| preview.body(self.result(), concise),
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
            if !concise && outcome.is_some_and(|outcome| outcome.output_id.is_some()) {
                rows.extend(render_preview(
                    "Saved output · /tools",
                    ACCENT,
                    width,
                    2,
                    inset,
                ));
            }
        }
        finish_card(rows)
    }

    fn render_arguments(&self, rows: &mut Vec<Row>, width: usize, inset: u16) {
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
            return;
        }
        let label = if name.is_empty() {
            "Requested arguments".to_owned()
        } else {
            format!("Requested arguments · {name}")
        };
        rows.extend(render_preview(&label, MUTED, width, 2, inset));
        rows.extend(render_preview(
            &self.arguments,
            SURFACE_TEXT,
            width,
            MAX_ARGUMENT_ROWS,
            inset,
        ));
    }

    fn render_change(
        &self,
        change: &FileChange,
        width: usize,
        inset: u16,
        details: ToolDetails,
    ) -> Vec<Row> {
        let frame = crate::theme::CardFrame::new(width);
        let path = self.invocation();
        let title = join_details([
            self.title().as_ref(),
            self.state.label().0,
            single_line(path).as_str(),
        ]);
        let mut rows = vec![Row::plain(frame.title(&title, ACCENT))];
        let mut clipped = false;
        if details != ToolDetails::Collapsed {
            let limit = if details == ToolDetails::Inspection {
                MAX_INSPECTION_ROWS
            } else {
                MAX_CHANGE_ROWS
            };
            let lines = change.lines(path, frame.content_width(), limit);
            clipped = lines.len() > limit;
            rows.extend(
                lines
                    .into_iter()
                    .take(limit)
                    .map(|line| Row::plain(frame.line(line))),
            );
        }
        let footer = if clipped {
            format!("{} · …", change.stats)
        } else {
            change.stats.clone()
        };
        rows.push(Row::plain(frame.foot(&footer)));
        if details == ToolDetails::Inspection {
            self.render_arguments(&mut rows, width, inset);
        }
        rows.push(Row::plain(Line::default()));
        rows
    }

    fn file_change(&self) -> Option<&FileChange> {
        let State::Recorded(outcome) = &self.state else {
            return None;
        };
        outcome
            .change
            .as_ref()
            .filter(|value| value.is_for(&self.name))
    }

    fn observation(&self) -> Option<&Observation> {
        self.state
            .outcome()?
            .observation
            .as_ref()
            .filter(|value| self.name.is_empty() || value.is_for(&self.name))
    }

    fn session_caption(&self) -> Option<&str> {
        let outcome = self
            .state
            .outcome()
            .filter(|outcome| outcome.status == "ok")?;
        if let Some(observation) = self.observation() {
            return Some(&observation.caption);
        }
        if !is_session_read(&self.name) {
            return None;
        }
        let fallback = if outcome.content.is_empty() {
            "Result details unavailable"
        } else {
            "Result available · /tools"
        };
        Some(fallback)
    }

    fn session_detail(&self) -> String {
        let detail = self.observation().map_or("", |value| value.detail.as_str());
        if self.name == "list_agents" {
            detail.to_owned()
        } else {
            join_details([self.invocation.as_str(), detail])
        }
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
                SURFACE_TEXT,
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
        if self.invocation.is_empty()
            || matches!(
                self.name.as_str(),
                "run_command" | "read_file" | "write_file" | "edit_file"
            )
        {
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
        self.file_change()
            .and_then(FileChange::diff)
            .or_else(|| (!outcome.diff.is_empty()).then_some(outcome.diff.as_str()))
    }
}

pub(super) fn style_card(rows: &mut [Row]) {
    for row in rows {
        row.line.style = panel().patch(row.line.style);
    }
}

fn finish_card(mut rows: Vec<Row>) -> Vec<Row> {
    style_card(&mut rows);
    rows.push(Row::plain(Line::default()));
    rows
}

fn join_details<const N: usize>(parts: [&str; N]) -> String {
    parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
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
    observation: Option<Observation>,
    change: Option<FileChange>,
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
        let summary = get("summary").or(fallback_summary);
        let succeeded = get("status") == Some("ok");
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
            summary: capped(summary, MAX_DETAIL_BYTES),
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
            command_preview: CommandPreview::from_result(content, structured, output_id, truncated),
            file_read: FileRead::from_result(structured, truncated),
            receipt: succeeded.then(|| agent_receipt(structured)).flatten(),
            observation: succeeded
                .then(|| {
                    Observation::from_result(
                        structured,
                        summary.unwrap_or_default(),
                        content.unwrap_or_default(),
                        truncated,
                    )
                })
                .flatten(),
            change: succeeded
                .then(|| FileChange::from_result(structured, truncated))
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

fn invocation(tool: &str, args: &Value) -> String {
    if tool == "query_session" {
        return query_target(args);
    }
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
        "list_agents" => Some("wait_for"),
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
mod tests;
