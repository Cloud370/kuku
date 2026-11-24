use std::borrow::Cow;

use kuku_client::{HistoryEntry, HistoryKind};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use serde_json::{Map, Value};
use unicode_width::UnicodeWidthStr;

use crate::markup::{render_diff, render_literal};
use crate::text::{cap, shorten, single_line};
use crate::theme::{
    ACCENT, MODEL, MUTED, SURFACE_TEXT, TranscriptTone, WARNING, emphasis, tool_operation_color,
    tool_status,
};

use super::agent::{AgentCard, AgentRequest};
use super::details::{InlineOutput, ScriptDetail};
use super::{Row, ToolDetails, append_bounded, body_inset, capped, render_rows};

mod activity;
mod command;
mod file_change;
mod inspection;
mod outcome;
mod script;
mod session;

use file_change::FileChange;
use file_change::OutputFormat;
use outcome::{Outcome, State};
use session::{Observation, is_session_read, query_target};

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
pub(crate) struct ToolView {
    pub(crate) name: String,
    pub(crate) summary: String,
    pub(crate) invocation: String,
    pub(crate) details: String,
    pub(crate) diff: String,
    pub(crate) output_id: Option<String>,
    pub(crate) output_range: Option<std::ops::Range<u64>>,
    pub(crate) evidence: String,
    pub(crate) inline_output_complete: bool,
    pub(crate) format: OutputFormat,
}

#[derive(Debug, Clone, Default)]
pub(super) struct Tool {
    name: String,
    invocation: String,
    arguments: String,
    script_source: String,
    script_source_lines: usize,
    request_title: String,
    start_summary: String,
    progress: String,
    state: State,
    agent_request: Option<AgentRequest>,
}

impl Tool {
    pub(super) fn positioned_at_result(&self) -> bool {
        matches!(&self.state, State::Recorded(outcome) if outcome.receipt.is_none())
    }

    pub(super) fn from_entry(entry: &HistoryEntry) -> Self {
        let name = capped(
            entry
                .details
                .get("tool")
                .and_then(Value::as_str)
                .or_else(|| entry.details.get("structured")?["tool"].as_str()),
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
        let script_code = if name == "run_js" {
            args["code"].as_str()
        } else {
            None
        };
        let script_source = capped(script_code, MAX_DETAIL_BYTES);
        let arguments = if script_source.is_empty() {
            arguments_json(args)
        } else {
            let other_args = args
                .as_object()
                .into_iter()
                .flatten()
                .filter(|(name, _)| name.as_str() != "code")
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect();
            arguments_json(&Value::Object(other_args))
        };
        let agent_request = AgentRequest::from_call(&name, args);
        let request_title = if matches!(name.as_str(), "run_command" | "run_js") {
            capped(args["title"].as_str().map(str::trim), MAX_LABEL_BYTES)
        } else {
            String::new()
        };
        Self {
            name,
            invocation,
            arguments,
            script_source,
            script_source_lines: script_code.map_or(0, |code| code.lines().count()),
            request_title,
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
        if !incoming.start_summary.is_empty() {
            self.start_summary = incoming.start_summary;
        }
        if !incoming.request_title.is_empty() {
            self.request_title = incoming.request_title;
        }
        if !incoming.arguments.is_empty() {
            self.arguments = incoming.arguments;
        }
        if !incoming.script_source.is_empty() {
            self.script_source = incoming.script_source;
            self.script_source_lines = incoming.script_source_lines;
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
                if self.start_summary.is_empty() {
                    self.start_summary = capped(
                        event["summary"]
                            .as_str()
                            .map(str::trim)
                            .filter(|summary| *summary != self.name),
                        MAX_DETAIL_BYTES,
                    );
                }
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
            name: self.name().to_owned(),
            summary: self.summary().to_owned(),
            invocation: self.invocation().to_owned(),
            details: self.result().to_owned(),
            diff: self.diff().unwrap_or_default().to_owned(),
            output_id: outcome.and_then(|outcome| outcome.output_id.clone()),
            output_range: None,
            evidence: outcome.map_or_else(String::new, Outcome::evidence),
            inline_output_complete: outcome.is_some_and(|outcome| outcome.inline_output_complete),
            format: OutputFormat::Text,
        }
    }

    pub(super) fn output_views(&self) -> Vec<ToolView> {
        self.state
            .outcome()
            .and_then(|outcome| outcome.script.as_ref())
            .map_or_else(Vec::new, |script| script.outputs())
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
        let model = receipt
            .and(outcome)
            .and_then(|outcome| outcome.agent_model.as_deref());
        Some(request.card(receipt, model, status, error))
    }

    pub(super) fn group_name(&self) -> Option<&str> {
        let State::Recorded(outcome) = &self.state else {
            return None;
        };
        if outcome.status != "ok"
            || outcome.exit_code.is_some_and(|code| code != 0)
            || outcome.attention
            || self.observation().is_some_and(|value| value.attention)
        {
            return None;
        }
        let name = self.name();
        match name {
            "read_file" | "find_files" | "search_text" | "list_agents" | "query_session"
            | "stat_file" | "read_output" | "search_output" | "list_values" => Some(name),
            "run_js"
                if outcome
                    .script
                    .as_ref()
                    .is_some_and(|script| script.read_only()) =>
            {
                Some(name)
            }
            _ => None,
        }
    }

    pub(super) fn inspection_count(&self, operation: &str) -> u64 {
        if let Some(script) = self
            .state
            .outcome()
            .and_then(|outcome| outcome.script.as_ref())
        {
            return script.inspection_count(operation);
        }
        u64::from(self.group_name() == Some(operation))
    }

    pub(super) fn checks_delegation(&self, sender: &str, recipient: &str) -> bool {
        self.group_name() == Some("list_agents")
            && self
                .observation()
                .is_some_and(|item| item.checks_delegation(sender, recipient))
    }

    pub(super) fn agent_listing(&self) -> Option<String> {
        let observation = self
            .observation()
            .filter(|value| value.is_for("list_agents"))?;
        Some(if let Some(agent) = observation.single_agent() {
            format!("Agent · {agent}")
        } else if observation.detail.is_empty() {
            observation.caption.clone()
        } else {
            format!("{}\n{}", observation.caption, observation.detail)
        })
    }

    pub(super) fn inspection_result(&self) -> Option<String> {
        let result = match self.name() {
            "read_file" => self.file_caption()?,
            "find_files" | "search_text" => {
                single_line(cap(self.search_summary()?, MAX_LABEL_BYTES))
            }
            "query_session" => self.session_caption()?.to_owned(),
            _ => return None,
        };
        Some(format!("{} · {result}", self.title()))
    }

    fn search_summary(&self) -> Option<&str> {
        let summary = self.summary();
        (matches!(self.name.as_str(), "find_files" | "search_text") && !summary.trim().is_empty())
            .then_some(summary)
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
        let status = self.status_line().0.to_string();
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

    fn render_body(
        &self,
        width: usize,
        details: ToolDetails,
        in_group: bool,
        script_detail: Option<ScriptDetail>,
        inline_output: Option<&InlineOutput>,
    ) -> Vec<Row> {
        if self.name() == "run_js" {
            return self.render_script(width, details, script_detail, inline_output);
        }
        if let ToolDetails::ActivityPreview(count) = details {
            return self.render_activity(width, count);
        }
        if details == ToolDetails::GroupMember
            && self.workspace_activity()
            && self.group_name().is_none()
        {
            return self.render_activity(width, 1);
        }
        if let Some(command) = self
            .state
            .outcome()
            .and_then(|outcome| outcome.command_preview.as_deref())
        {
            if matches!(
                details,
                ToolDetails::Inspection | ToolDetails::InspectionWithArguments
            ) {
                return self.render_command_inspection(
                    command,
                    width,
                    in_group,
                    details == ToolDetails::InspectionWithArguments,
                    inline_output,
                );
            }
            if command.concise() && command.body().lines().take(4).count() <= 3 {
                return self.render_short_command(command, width, details);
            }
        }
        if self.name() == "read_file"
            && self
                .state
                .outcome()
                .is_some_and(|outcome| outcome.status != "ok")
        {
            return self.render_failed_read(width, details);
        }
        if details != ToolDetails::Inspection {
            if let Some(rows) = self.render_agent_check(width) {
                return rows;
            }
        }
        if in_group && self.group_name() == Some("read_file") {
            if let Some(rows) = self.render_read_details(width, details == ToolDetails::Inspection)
            {
                return rows;
            }
        }
        self.render_content(width, details)
    }

    fn render_agent_check(&self, width: usize) -> Option<Vec<Row>> {
        let (summary, color) = match self.state.outcome() {
            None if self.name == "list_agents" && !self.invocation.is_empty() => (
                format!("{} · Waiting", single_line(&self.invocation)),
                MODEL,
            ),
            Some(outcome) if outcome.status == "ok" => {
                let result = self.observation()?;
                let color = if result.attention { WARNING } else { MUTED };
                (result.single_agent()?.to_owned(), color)
            }
            _ => return None,
        };
        let columns = width;
        let caption = "Agent check · ";
        let line = Line::from(vec![
            Span::styled(shorten(caption, columns), Style::new().fg(MUTED)),
            Span::styled(
                shorten(&summary, columns.saturating_sub(caption.width())),
                Style::new().fg(color),
            ),
        ]);
        Some(vec![Row::plain(line)])
    }

    fn render_content(&self, width: usize, details: ToolDetails) -> Vec<Row> {
        let inspecting = details == ToolDetails::Inspection;
        let expanded = inspecting || details == ToolDetails::Expanded;
        let inset = 0;
        let caption = self.file_caption();
        if details == ToolDetails::GroupMember {
            let target = self.session_caption().map_or_else(
                || caption.unwrap_or_else(|| single_line(cap(self.invocation(), MAX_LABEL_BYTES))),
                |summary| join_details([self.invocation.as_str(), summary]),
            );
            let action = self.title();
            let summary = self
                .search_summary()
                .filter(|summary| *summary != target)
                .map_or_else(String::new, |value| {
                    single_line(cap(value, MAX_LABEL_BYTES))
                });
            let heading = if target.is_empty() {
                Row::caption(&action, &summary, ACCENT, width)
            } else {
                Row::action(&format!("{action} · "), &target, &summary, ACCENT, width)
            };
            return vec![heading];
        }
        if let Some(change) = self.file_change() {
            return self.render_change(change, width, details);
        }
        if !inspecting {
            if let Some(caption) = self.session_caption() {
                let mut rows = self.render_header(width, inset, Some(caption));
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
        let outcome = self.state.outcome();
        let command = outcome.and_then(|outcome| outcome.command_preview.as_ref());
        let is_command = self.name == "run_command" || command.is_some();
        let concise = !inspecting && command.is_some_and(|preview| preview.concise());
        let mut rows = if is_command {
            self.command_heading(width)
        } else if let Some(read) = outcome
            .and_then(|outcome| outcome.file_read.as_ref())
            .filter(|_| outcome.is_some_and(|outcome| outcome.status == "ok"))
        {
            vec![Row::read(
                &single_line(self.invocation()),
                read.caption_line(),
                width,
            )]
        } else {
            self.render_header(width, inset, caption.as_deref())
        };
        let invocation = self.invocation();
        let command_summary = command.map(|preview| preview.summary());
        let summary = command_summary
            .as_deref()
            .filter(|summary| !summary.is_empty())
            .unwrap_or_else(|| self.summary());
        if expanded
            && caption.is_none()
            && (!is_command || inspecting || !self.request_title.is_empty())
            && (!inspecting || invocation != self.arguments)
        {
            if is_command && !inspecting && !invocation.is_empty() {
                let command = shorten(&single_line(cap(invocation, MAX_PREVIEW_BYTES)), width);
                rows.push(Row::plain(Line::styled(command, Style::new().fg(MUTED))));
            } else {
                rows.extend(render_preview(
                    invocation,
                    ACCENT,
                    width,
                    MAX_INVOCATION_ROWS,
                    inset,
                ));
            }
        }
        if !concise
            && caption.is_none()
            && (!expanded || summary != invocation)
            && (!is_command
                || summary != self.command_title()
                    && (outcome.is_some() || self.command_title() != invocation))
        {
            let limit = if expanded { MAX_SUMMARY_ROWS } else { 1 };
            rows.extend(render_preview(summary, SURFACE_TEXT, width, limit, inset));
        }
        if inspecting {
            self.render_arguments(&mut rows, width, inset);
            if self.session_caption().is_some() && !self.result().is_empty() {
                rows.extend(render_preview("Raw result", MUTED, width, 1, inset));
            }
        }
        if expanded {
            let result = if let Some(diff) = self.diff() {
                render_excerpt(diff, render_diff)
            } else {
                let text = command.map_or_else(|| self.result(), |preview| preview.body());
                if text.is_empty() || text == summary || (!is_command && text == invocation) {
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
            if !concise && outcome.is_some_and(|outcome| outcome.output_id.is_some()) {
                rows.push(Row::open_output(super::details::OutputTarget::Tool, width));
            }
        }
        if let Some(preview) = command {
            rows.extend(render_preview(
                &preview.notes(),
                WARNING,
                width,
                MAX_WARNING_ROWS,
                inset,
            ));
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

    pub(super) fn compact_file_change(&self) -> bool {
        self.state
            .outcome()
            .is_some_and(|outcome| outcome.status == "ok" && !outcome.attention)
            && self.file_change().is_some_and(|change| !change.unconfirmed)
    }

    fn file_change(&self) -> Option<&FileChange> {
        let State::Recorded(outcome) = &self.state else {
            return None;
        };
        outcome
            .change
            .as_ref()
            .filter(|value| self.name.is_empty() || value.tool() == self.name)
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
            "Result available"
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
        if self.name() == "run_js" && !self.request_title.is_empty() {
            Cow::Owned(single_line(&self.request_title))
        } else {
            tool_title(self.name())
        }
    }

    pub(super) fn name(&self) -> &str {
        if !self.name.is_empty() {
            return &self.name;
        }
        let Some(outcome) = self.state.outcome() else {
            return "";
        };
        if outcome.command_preview.is_some() {
            "run_command"
        } else if outcome.script.is_some() {
            "run_js"
        } else if let Some(change) = &outcome.change {
            change.tool()
        } else if outcome.file_read.is_some() {
            "read_file"
        } else {
            ""
        }
    }

    fn status_line(&self) -> (Line<'static>, Color) {
        let (status, color) = self.state.label();
        let mut status = Line::from(Span::styled(
            shorten(&single_line(status), 24),
            tool_status(color),
        ));
        if let Some(outcome) = self.state.outcome() {
            let command = outcome.command_preview.as_deref();
            if let Some(exit) = outcome.exit_code {
                let label = command.map_or("exit", command::CommandPreview::exit_label);
                status.extend(command::exit_metric(label, exit).spans);
            } else if let Some(signal) = command.and_then(command::CommandPreview::signal) {
                status.extend(
                    Row::metric(
                        " · signal ",
                        signal.to_string(),
                        "",
                        TranscriptTone::Warning,
                    )
                    .spans,
                );
            }
            if let Some(command) = command {
                if command.has_stderr() {
                    status.push_span(Span::styled(" · stderr", Style::new().fg(MUTED)));
                }
                status.extend(command.elapsed_metric().spans);
            }
        }
        (status, color)
    }

    fn render_header(&self, width: usize, detail_inset: u16, caption: Option<&str>) -> Vec<Row> {
        let (status, _) = self.status_line();
        let title = self.title();
        let style = emphasis(tool_operation_color(self.name()));
        let mut title = Line::styled(shorten(&title, width), style);
        let status_inline = title.width() + status.width() + 3 <= width;
        if status_inline {
            title.push_span(Span::styled(" · ", Style::new().fg(MUTED)));
            title.extend(status.spans.iter().cloned());
        }
        let separate_caption =
            caption.filter(|caption| !status_inline || title.width() + caption.width() + 3 > width);
        if let Some(caption) = caption.filter(|_| separate_caption.is_none()) {
            title.push_span(Span::styled(format!(" · {caption}"), Style::new()));
        }
        let mut lines = vec![title];
        if !status_inline {
            lines.push(status);
        }
        let mut rows: Vec<_> = render_rows(lines, width, 4, 0).collect();
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
                "run_command" | "read_file" | "write_file" | "edit_file" | "remove_file"
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

fn finish_card(mut rows: Vec<Row>) -> Vec<Row> {
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
        "remove_file" => "Remove file",
        "run_command" => "Run command",
        "run_js" => "Run JavaScript",
        "read_output" => "Read saved output",
        "search_output" => "Search saved output",
        "create_output" => "Start output",
        "append_output" => "Append output",
        "finish_output" => "Save output",
        "save_value" => "Save value",
        "list_values" => "List saved values",
        "stat_file" => "Inspect file",
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

fn invocation(tool: &str, args: &Value) -> String {
    if tool == "query_session" {
        return query_target(args);
    }
    if tool == "run_command" {
        return command::invocation(args);
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
        "edit_file" | "read_file" | "write_file" | "remove_file" => Some("path"),
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
