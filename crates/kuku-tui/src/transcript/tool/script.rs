use std::borrow::Cow;
use std::collections::HashSet;

use kuku_client::{
    CommandTermination, FileCommitState, FileMutationFacts, FileMutationKind,
    WorkspaceScriptActionFacts, WorkspaceScriptActionStatus, WorkspaceScriptFacts,
    WorkspaceScriptOperation,
};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

use crate::markup::{file_source, wrap_numbered};
use crate::text::single_line;
use crate::theme::{tool_status, TranscriptTone, ERROR, MUTED, SUCCESS, SURFACE_TEXT, WARNING};

use super::command::CommandPreview;
use super::file_change::{compact_change_row, FileChange};
use super::outcome::FileRead;
use crate::transcript::details::{InlineOutput, OutputTarget, ScriptDetail};
use crate::transcript::MAX_VISUAL_ROWS;

use super::{
    body_inset, capped, finish_card, render_preview, render_rows, tool_operation_color, tool_title,
    OutputFormat, Row, Tool, ToolDetails, ToolView, MAX_DETAIL_BYTES, MAX_INSPECTION_ROWS,
    MAX_RESULT_ROWS, MAX_WARNING_ROWS,
};

#[derive(Debug, Clone)]
pub(super) struct ScriptPreview {
    facts: WorkspaceScriptFacts,
    read_only: bool,
}

impl ScriptPreview {
    pub(super) fn from_result(structured: &Value) -> Option<Box<Self>> {
        if structured["kind"].as_str() != Some("workspace_script_result") {
            return None;
        }
        let facts = WorkspaceScriptFacts::from_structured(structured).ok()?;
        let read_only = is_read_only(&facts);
        Some(Box::new(Self { facts, read_only }))
    }

    pub(super) fn label(&self, status: &str) -> Option<(&'static str, Color)> {
        let incomplete = status != "ok"
            || self.facts.timed_out
            || self.facts.cleanup_incomplete
            || self.facts.not_dispatched > 0
            || self.facts.actions.iter().any(|action| !completed(action));
        if incomplete && self.facts.actions.iter().any(completed) {
            Some(("partial", WARNING))
        } else if incomplete && status == "ok" {
            Some(("host action incomplete", WARNING))
        } else {
            None
        }
    }

    pub(super) fn read_only(&self) -> bool {
        self.read_only
    }

    pub(super) fn inspection_count(&self, operation: &str) -> u64 {
        self.facts
            .actions
            .iter()
            .filter(|action| action.tool.name() == operation && completed(action))
            .map(action_count)
            .fold(0, u64::saturating_add)
    }

    fn action_counts(&self) -> (u64, u64) {
        self.facts.actions.iter().fold(
            (0, self.facts.not_dispatched as u64),
            |(complete, total), action| {
                let count = action_count(action);
                (
                    if completed(action) {
                        complete.saturating_add(count)
                    } else {
                        complete
                    },
                    total.saturating_add(count),
                )
            },
        )
    }

    pub(super) fn summary(&self) -> String {
        let (complete, total) = self.action_counts();
        let changes = self.file_change_count();
        if total == 0 {
            return String::new();
        }
        let actions = format!("{complete}/{total} host actions completed");
        match changes {
            0 => actions,
            1 => format!("{actions} · 1 file change"),
            _ => format!("{actions} · {changes} file changes"),
        }
    }

    pub(super) fn file_change_count(&self) -> usize {
        self.facts
            .actions
            .iter()
            .filter(|action| changed_file(action).is_some())
            .count()
    }

    pub(super) fn has_changes(&self) -> bool {
        self.facts.actions.iter().any(recorded_change)
    }

    pub(super) fn change_previews(&self, width: usize, open_action: Option<usize>) -> Vec<Row> {
        self.facts
            .actions
            .iter()
            .enumerate()
            .filter(|(_, action)| recorded_change(action))
            .flat_map(|(index, action)| {
                action_rows(index, action, width, open_action == Some(index), None)
            })
            .collect()
    }

    pub(super) fn first_failed_action(&self) -> Option<&str> {
        self.facts
            .actions
            .iter()
            .find_map(|action| {
                action.result["start_error"]
                    .as_str()
                    .filter(|text| !text.is_empty())
            })
            .or_else(|| {
                self.facts
                    .actions
                    .iter()
                    .find(|action| !completed(action) && !action.summary.trim().is_empty())
                    .map(|action| action.summary.as_str())
            })
    }

    pub(super) fn evidence(&self, status: &str) -> String {
        let mut lines = vec![self.summary()];
        lines.extend(self.notes(status));
        lines.extend(self.action_details().map(|line| line.to_string()));
        if self.facts.output_ref.is_some() {
            lines.push("JSON saved".to_owned());
        }
        lines.join("\n")
    }

    pub(super) fn action_details(&self) -> impl Iterator<Item = Line<'static>> + '_ {
        self.facts.actions.iter().map(action_detail)
    }

    pub(super) fn inspection_preview(&self) -> Option<Line<'static>> {
        self.facts
            .actions
            .iter()
            .rev()
            .find(|action| {
                completed(action)
                    && match action.tool {
                        WorkspaceScriptOperation::ReadFile => action.result["path"]
                            .as_str()
                            .is_some_and(|path| !path.is_empty()),
                        WorkspaceScriptOperation::FindFiles
                        | WorkspaceScriptOperation::SearchText => !action.summary.trim().is_empty(),
                        _ => false,
                    }
            })
            .map(action_detail)
    }

    pub(super) fn outputs(&self) -> Vec<ToolView> {
        let mut seen = HashSet::new();
        let mut outputs = self
            .facts
            .actions
            .iter()
            .enumerate()
            .filter_map(|(index, action)| {
                if let Some(view) = file_change_view(action) {
                    return Some(view);
                }
                if action.tool == WorkspaceScriptOperation::ReadOutput {
                    return None;
                }
                let view = script_action_output(index, action)?;
                seen.insert(view.output_id.as_ref()?.clone())
                    .then_some(view)
            })
            .collect::<Vec<_>>();
        if let Some(output) = &self.facts.output_ref {
            outputs.push(ToolView {
                name: "Script return".into(),
                summary: format!("Saved JSON · {} bytes", output.bytes),
                output_id: Some(output.id.clone()),
                ..ToolView::default()
            });
        } else if let Some(value) = self.pretty_value() {
            outputs.push(ToolView {
                name: "Script return".into(),
                summary: "JSON".into(),
                details: value,
                format: OutputFormat::Json,
                inline_output_complete: true,
                ..ToolView::default()
            });
        }
        outputs
    }

    fn notes(&self, status: &str) -> Vec<String> {
        let mut notes = Vec::new();
        if self.facts.timed_out {
            notes.push("Script timed out".to_owned());
        } else if status == "cancelled" {
            notes.push("Script cancelled".to_owned());
        }
        if self.facts.cleanup_incomplete {
            notes.push("Cleanup incomplete · Some effects may be unknown".to_owned());
        }
        if self.facts.not_dispatched > 0 {
            notes.push(format!(
                "{} host actions stopped before dispatch",
                self.facts.not_dispatched
            ));
        }
        notes
    }

    fn pretty_value(&self) -> Option<String> {
        (!self.facts.value.is_null()).then(|| {
            serde_json::to_string_pretty(&self.facts.value).expect("JSON value serializes")
        })
    }
}

impl Tool {
    fn script_source_rows(&self, width: usize, open: Option<ScriptDetail>) -> Vec<Row> {
        if self.script_source.is_empty() {
            return Vec::new();
        }
        const INPUT_PREVIEW_ROWS: usize = 3;
        let limit = if open == Some(ScriptDetail::Source) {
            MAX_VISUAL_ROWS
        } else {
            INPUT_PREVIEW_ROWS
        };
        let end = self
            .script_source
            .split_inclusive('\n')
            .take(limit + 1)
            .map(str::len)
            .sum();
        let columns = Row::detail_width(width);
        let lines = file_source(&self.script_source[..end], "javascript");
        let content = if open == Some(ScriptDetail::Source) {
            render_rows(lines, columns, limit, 0).collect()
        } else {
            let mut lines = lines.into_iter();
            let mut preview = lines
                .by_ref()
                .take(limit)
                .map(|line| Row::plain(Row::fit(line, columns)))
                .collect::<Vec<_>>();
            if lines.next().is_some() {
                preview.push(Row::caption("…", "", MUTED, columns));
            }
            preview
        };
        let language = Line::styled("JavaScript", TranscriptTone::Type.style());
        script_section(ScriptDetail::Source, language, content, width)
    }

    pub(in crate::transcript) fn script_action_description(
        &self,
        index: usize,
    ) -> Option<(&'static str, &str)> {
        let action = self
            .state
            .outcome()?
            .script
            .as_ref()?
            .facts
            .actions
            .get(index)?;
        if action.tool == WorkspaceScriptOperation::RunCommand {
            Some(("command", &action.summary))
        } else {
            Some(("file change", action.result["path"].as_str()?))
        }
    }

    pub(in crate::transcript) fn script_action_output(&self, index: usize) -> Option<ToolView> {
        let action = self
            .state
            .outcome()?
            .script
            .as_ref()?
            .facts
            .actions
            .get(index)?;
        script_action_output(index, action)
    }

    pub(super) fn render_script(
        &self,
        width: usize,
        details: ToolDetails,
        open: Option<ScriptDetail>,
        inline_output: Option<&InlineOutput>,
    ) -> Vec<Row> {
        let mut rows = vec![Row::activity_title(
            "Script",
            tool_operation_color("run_js"),
            &single_line(&self.request_title),
            width,
        )];
        let columns = Row::detail_width(width);
        let inspect = details == ToolDetails::Inspection;
        let outcome = self.state.outcome();
        let mut body = Vec::new();
        if let ToolDetails::ActivityPreview(count) = details {
            if count > 1 {
                let (mut status, _) = self.status_line();
                status
                    .extend(Row::metric(" ×", count.to_string(), "", TranscriptTone::Value).spans);
                body.extend(Row::styled_status(
                    &single_line(self.activity_diagnostic()),
                    status,
                    MUTED,
                    columns,
                ));
            }
        }
        if inspect {
            body.extend(Row::styled_status(
                "Status",
                self.status_line().0,
                MUTED,
                columns,
            ));
        }
        if let Some(outcome) = outcome {
            if let Some(script) = &outcome.script {
                if inspect {
                    body.extend(self.render_script_details(
                        script,
                        &outcome.status,
                        columns,
                        open,
                        inline_output,
                    ));
                } else {
                    body.extend(
                        script.change_previews(columns, open.and_then(ScriptDetail::action)),
                    );
                }
            } else if inspect {
                body.extend(render_preview(
                    self.result(),
                    SURFACE_TEXT,
                    columns,
                    MAX_WARNING_ROWS,
                    0,
                ));
                body.extend(self.script_source_rows(columns, open));
            }
        } else if inspect {
            body.extend(self.script_source_rows(columns, open));
        }
        if !inspect && body.is_empty() {
            body = self.compact_script_rows(columns);
        }
        rows.extend(body.into_iter().map(|row| row.with_detail_rule(width)));
        finish_card(rows)
    }

    fn compact_script_rows(&self, width: usize) -> Vec<Row> {
        let (mut status, color) = self.status_line();
        let script = self
            .state
            .outcome()
            .and_then(|outcome| outcome.script.as_deref());
        if let Some(script) = script {
            let (complete, total) = script.action_counts();
            if total != 0 {
                status.extend(
                    Row::metric(
                        " · ",
                        format!("{complete}/{total}"),
                        " actions completed",
                        TranscriptTone::Value,
                    )
                    .spans,
                );
            }
        }
        let mut rows = vec![Row::caption("Status", "", MUTED, width).with_summary(status, width)];
        if let Some(script) = script {
            let actions = &script.facts.actions;
            if let Some(action) = actions
                .iter()
                .find(|action| !completed(action))
                .or_else(|| actions.first())
            {
                if let Some(command) = CommandPreview::from_result(&action.result) {
                    let invocation = single_line(&command.invocation());
                    rows.extend(
                        command_action_heading(&command, &invocation, width)
                            .into_iter()
                            .take(1),
                    );
                } else {
                    rows.push(Row::plain(Row::fit(action_detail(action), width)));
                }
            }
            let output = if script.facts.output_ref.is_some() {
                Some(Line::styled("Saved JSON", TranscriptTone::Type.style()))
            } else {
                (!script.facts.value.is_null()).then(|| value_summary(&script.facts.value))
            };
            if let Some(output) = output {
                rows.push(Row::caption("Output", "", MUTED, width).with_summary(output, width));
            }
        }
        if rows.len() == 1 {
            let diagnostic = self.activity_diagnostic();
            if matches!(color, ERROR | WARNING) && !diagnostic.trim().is_empty() {
                rows.extend(render_preview(diagnostic, color, width, 1, 0));
            } else if !self.script_source.is_empty() {
                let lines = self.script_source.lines().count();
                let summary = Row::metric(
                    "JavaScript · ",
                    lines.to_string(),
                    if lines == 1 { " line" } else { " lines" },
                    TranscriptTone::Value,
                );
                rows.push(Row::caption("Input", "", MUTED, width).with_summary(summary, width));
            }
        }
        rows
    }

    fn render_script_details(
        &self,
        script: &ScriptPreview,
        status: &str,
        width: usize,
        open: Option<ScriptDetail>,
        inline_output: Option<&InlineOutput>,
    ) -> Vec<Row> {
        let open_action = open.and_then(ScriptDetail::action);
        let mut rows = Vec::new();
        for note in script.notes(status) {
            rows.extend(render_preview(&note, WARNING, width, 1, 0));
        }
        if status != "ok" {
            let reason = self
                .result()
                .lines()
                .find(|line| {
                    !line.trim().is_empty() && !line.starts_with("Host action results observed:")
                })
                .unwrap_or_else(|| self.summary());
            rows.extend(render_preview(reason, WARNING, width, MAX_WARNING_ROWS, 0));
        }
        rows.extend(self.script_source_rows(width, open));
        let (complete, total) = script.action_counts();
        if total != 0 {
            let mut heading = Line::styled("Actions", TranscriptTone::Text.emphasized());
            heading.extend(
                Row::metric(
                    " · ",
                    format!("{complete}/{total}"),
                    " completed",
                    TranscriptTone::Value,
                )
                .spans,
            );
            let changes = script.file_change_count();
            if changes > 0 {
                let unit = if changes == 1 {
                    " file change"
                } else {
                    " file changes"
                };
                heading.extend(
                    Row::metric(" · ", changes.to_string(), unit, TranscriptTone::Value).spans,
                );
            }
            rows.push(Row::plain(Row::fit(heading, width)));
            let columns = Row::detail_width(width);
            let mut actions = script.change_previews(columns, open_action);
            let other_actions = || {
                script
                    .facts
                    .actions
                    .iter()
                    .enumerate()
                    .filter(|(_, action)| !recorded_change(action))
            };
            for (index, action) in other_actions()
                .filter(|(_, action)| important(action))
                .chain(other_actions().filter(|(_, action)| !important(action)))
                .take(MAX_INSPECTION_ROWS)
            {
                actions.extend(action_rows(
                    index,
                    action,
                    columns,
                    open_action == Some(index),
                    inline_output
                        .filter(|output| output.target == OutputTarget::ScriptAction(index)),
                ));
            }
            rows.extend(actions.into_iter().map(|row| row.with_detail_rule(width)));
        }
        rows.extend(script.return_rows(width, open == Some(ScriptDetail::Return)));
        if script.facts.output_ref.is_some() || script.facts.actions.iter().any(has_output) {
            rows.push(Row::open_output(OutputTarget::Script, width));
        }
        rows
    }
}

impl ScriptPreview {
    fn return_rows(&self, width: usize, open: bool) -> Vec<Row> {
        let value = &self.facts.value;
        if value.is_null() {
            return Vec::new();
        }
        let columns = Row::detail_width(width);
        let content = if open {
            let text = self.pretty_value().expect("non-null return value");
            wrap_numbered(file_source(&text, "json"), columns, MAX_VISUAL_ROWS, 1, 0)
                .into_iter()
                .map(Row::plain)
                .collect()
        } else {
            return_preview(value, columns)
        };
        script_section(ScriptDetail::Return, value_summary(value), content, width)
    }
}

fn script_section(
    detail: ScriptDetail,
    summary: Line<'static>,
    content: Vec<Row>,
    width: usize,
) -> Vec<Row> {
    let mut heading = Row::linked_label(detail.label(), width).with_summary(summary, width);
    heading.script_detail = Some(detail);
    std::iter::once(heading)
        .chain(content.into_iter().map(|row| row.with_detail_rule(width)))
        .collect()
}

fn return_preview(value: &Value, width: usize) -> Vec<Row> {
    const FIELDS: usize = 3;
    let Some(fields) = value.as_object() else {
        return Vec::new();
    };
    let mut rows = fields
        .iter()
        .take(FIELDS)
        .map(|(key, value)| {
            let name = if key.is_empty() {
                r#""""#.into()
            } else {
                single_line(key)
            };
            Row::caption(&name, "", TranscriptTone::Field.color(), width)
                .with_summary(value_summary(value), width)
        })
        .collect::<Vec<_>>();
    if fields.len() > FIELDS {
        let count = fields.len() - FIELDS;
        let remaining = if count == 1 {
            " more field"
        } else {
            " more fields"
        };
        rows.push(Row::plain(Row::fit(
            Row::metric("", count.to_string(), remaining, TranscriptTone::Value),
            width,
        )));
    }
    rows
}

fn value_summary(value: &Value) -> Line<'static> {
    let count = |label, count: usize, singular, plural| {
        let unit = if count == 1 { singular } else { plural };
        Row::metric(label, count.to_string(), unit, TranscriptTone::Value)
    };
    let kind = TranscriptTone::Type.style();
    match value {
        Value::Object(fields) => count("", fields.len(), " field", " fields"),
        Value::Array(items) => count("", items.len(), " item", " items"),
        Value::String(text) if text.is_empty() => Line::styled("Empty text", kind),
        Value::String(text) => {
            let mut line = Line::styled("Text", kind);
            line.extend(count(" · ", text.lines().count(), " line", " lines").spans);
            line
        }
        Value::Null => Line::styled("null", kind),
        Value::Bool(flag) => Line::styled(flag.to_string(), TranscriptTone::Value.style()),
        Value::Number(number) => Line::styled(number.to_string(), TranscriptTone::Value.style()),
    }
}

fn action_change(action: &WorkspaceScriptActionFacts) -> Option<(&str, FileChange)> {
    let path = action.result["path"].as_str()?;
    let change = FileChange::from_result(&action.result, action.truncated)?;
    (action.status == WorkspaceScriptActionStatus::Ok || change.unconfirmed)
        .then_some((path, change))
}

fn recorded_change(action: &WorkspaceScriptActionFacts) -> bool {
    matches!(
        action.tool,
        WorkspaceScriptOperation::EditFile
            | WorkspaceScriptOperation::WriteFile
            | WorkspaceScriptOperation::RemoveFile
    ) && (action.status == WorkspaceScriptActionStatus::Ok
        || action.result["commit_state"] == "published_unconfirmed")
}

fn action_rows(
    index: usize,
    action: &WorkspaceScriptActionFacts,
    width: usize,
    open: bool,
    inline_output: Option<&InlineOutput>,
) -> Vec<Row> {
    if let Some(command) = CommandPreview::from_result(&action.result) {
        let invocation = single_line(&command.invocation());
        let mut rows = command_action_heading(&command, &invocation, width);
        rows[0].script_detail = Some(ScriptDetail::Action(index));
        if open {
            let output = has_output(action).then_some(OutputTarget::ScriptAction(index));
            rows.extend(command.inspection_rows(&invocation, width, output, None, inline_output));
        }
        return rows;
    }
    let Some((path, change)) = action_change(action) else {
        let mut rows = render_rows(vec![action_detail(action)], width, 1, 0).collect::<Vec<_>>();
        if saved_read(action).is_some() {
            if let Some(row) = rows.first_mut() {
                if let Some(label) = row.line.spans.first_mut() {
                    label.style = label.style.underlined();
                }
                row.output = Some(OutputTarget::ScriptAction(index));
            }
        }
        return rows;
    };
    let mut row = compact_change_row(&change, path, width);
    row.script_detail = Some(ScriptDetail::Action(index));
    let mut rows = vec![row];
    if open {
        let preview = change.expanded_rows(width);
        if preview.is_empty() && change.tool() != "remove_file" {
            let indent = body_inset(width);
            rows.push(
                Row::caption(
                    "Content preview unavailable",
                    "",
                    MUTED,
                    width.saturating_sub(usize::from(indent)),
                )
                .indented(indent),
            );
        } else {
            rows.extend(preview);
        }
    } else {
        rows.extend(change.compact_preview(width));
    }
    rows
}

fn command_action_heading(command: &CommandPreview, invocation: &str, width: usize) -> Vec<Row> {
    let (state, color) = command.label();
    let mut status = Line::from(Span::styled(state, tool_status(color)));
    status.extend(command.elapsed_metric().spans);
    Row::styled_status(
        &format!("Run {invocation}"),
        status,
        tool_operation_color("run_command"),
        width,
    )
}

fn saved_read(action: &WorkspaceScriptActionFacts) -> Option<(&str, std::ops::Range<u64>)> {
    if action.tool != WorkspaceScriptOperation::ReadOutput
        || action.status != WorkspaceScriptActionStatus::Ok
    {
        return None;
    }
    let output_id = action.result["output_id"]
        .as_str()
        .filter(|id| !id.is_empty())?;
    let start = action.result["offset"].as_u64()?;
    let end = action.result["next_offset"].as_u64()?;
    (start < end).then_some((output_id, start..end))
}

fn script_action_output(index: usize, action: &WorkspaceScriptActionFacts) -> Option<ToolView> {
    let (output_id, output_range) = if let Some((id, range)) = saved_read(action) {
        (id, Some(range))
    } else {
        (action.result["output_ref"]["id"].as_str()?, None)
    };
    let command = CommandPreview::from_result(&action.result);
    let view = ToolView {
        name: format!(
            "Script action {} · {}",
            index + 1,
            tool_title(action.tool.name())
        ),
        summary: action_detail(action).to_string(),
        output_id: Some(output_id.to_owned()),
        output_range,
        ..ToolView::default()
    };
    Some(match command {
        Some(command) => command.format_output(view),
        None => view,
    })
}

fn file_change_view(action: &WorkspaceScriptActionFacts) -> Option<ToolView> {
    let (path, change) = action_change(action)?;
    let summary = if change.unconfirmed {
        format!("{} · Durability unconfirmed", change.stats())
    } else {
        change.stats()
    };
    Some(ToolView {
        name: format!("{} {}", change.verb(), single_line(path)),
        invocation: path.to_owned(),
        summary,
        format: OutputFormat::Change(change),
        ..ToolView::default()
    })
}

fn is_read_only(facts: &WorkspaceScriptFacts) -> bool {
    !facts.timed_out
        && !facts.cleanup_incomplete
        && facts.not_dispatched == 0
        && !facts.actions.is_empty()
        && facts.actions.iter().all(|action| {
            completed(action)
                && matches!(
                    action.tool,
                    WorkspaceScriptOperation::ReadFile
                        | WorkspaceScriptOperation::FindFiles
                        | WorkspaceScriptOperation::SearchText
                        | WorkspaceScriptOperation::StatFile
                        | WorkspaceScriptOperation::ReadOutput
                        | WorkspaceScriptOperation::SearchOutput
                        | WorkspaceScriptOperation::ListValues
                )
        })
}

fn important(action: &WorkspaceScriptActionFacts) -> bool {
    !completed(action) || changed_file(action).is_some() || has_output(action)
}

fn has_output(action: &WorkspaceScriptActionFacts) -> bool {
    action.result["output_ref"]["id"].is_string()
}

fn action_count(action: &WorkspaceScriptActionFacts) -> u64 {
    if action.tool == WorkspaceScriptOperation::ReadOutput {
        action.result["pages"].as_u64().unwrap_or(1)
    } else {
        1
    }
}

fn completed(action: &WorkspaceScriptActionFacts) -> bool {
    if action.status != WorkspaceScriptActionStatus::Ok {
        return false;
    }
    if let Some(termination) = termination(action) {
        termination == CommandTermination::Exited && action.result["exit_code"].as_i64() == Some(0)
    } else {
        true
    }
}

fn action_detail(action: &WorkspaceScriptActionFacts) -> Line<'static> {
    let operation = Style::new().fg(tool_operation_color(action.tool.name()));
    let muted = TranscriptTone::Label.style();
    if action.tool == WorkspaceScriptOperation::ReadFile
        && action.status == WorkspaceScriptActionStatus::Ok
    {
        if let (Some(path), Some(read)) = (
            action.result["path"]
                .as_str()
                .filter(|path| !path.is_empty()),
            FileRead::from_result(&action.result, action.truncated),
        ) {
            let mut line = Line::from(vec![
                Span::styled("Read ", operation),
                Span::raw(single_line(&capped(Some(path), MAX_DETAIL_BYTES))),
                Span::styled(" · ", muted),
            ]);
            line.extend(read.caption_line().spans);
            return line;
        }
    }
    if action.tool == WorkspaceScriptOperation::FindFiles
        && action.status == WorkspaceScriptActionStatus::Ok
    {
        let facts = &action.result;
        if let (Some(path), Some(entries)) = (facts["path"].as_str(), facts["entry_count"].as_u64())
        {
            let mut line = Line::from(vec![
                Span::styled("Find ", operation),
                Span::styled(
                    single_line(&capped(Some(path), MAX_DETAIL_BYTES)),
                    TranscriptTone::Text.style(),
                ),
            ]);
            if let Some(pattern) = facts["pattern"]
                .as_str()
                .filter(|pattern| !pattern.is_empty())
            {
                line.push_span(Span::styled(
                    format!(
                        " · {}",
                        single_line(&capped(Some(pattern), MAX_DETAIL_BYTES))
                    ),
                    muted,
                ));
            }
            let unit = if entries == 1 { " entry" } else { " entries" };
            line.extend(Row::metric(" · ", entries.to_string(), unit, TranscriptTone::Value).spans);
            if action.truncated {
                line.push_span(Span::styled(
                    " · listing shortened",
                    TranscriptTone::Warning.style(),
                ));
            }
            return line;
        }
    }
    if let Some(change) = changed_file(action) {
        let verb = match change.kind {
            FileMutationKind::FileEdit => "Edited",
            FileMutationKind::FileWrite => "Wrote",
            FileMutationKind::FileRemove => "Removed",
        };
        let mut line = Line::from(vec![
            Span::styled(format!("{verb} "), operation),
            Span::raw(single_line(&change.path)),
        ]);
        if change.commit_state == FileCommitState::PublishedUnconfirmed {
            line.push_span(Span::styled(" · ", muted));
            line.push_span(Span::styled(
                "durability unconfirmed",
                Style::new().fg(WARNING),
            ));
        }
        return line;
    }
    let name = tool_title(action.tool.name());
    let termination = termination(action);
    let state = termination.map_or_else(
        || status_label(action.status).into(),
        |termination| command_state(termination, &action.result),
    );
    let limit = MAX_DETAIL_BYTES / MAX_RESULT_ROWS;
    let summary = if termination.is_some() {
        action.result["start_error"].as_str().unwrap_or_default()
    } else {
        &action.summary
    };
    let summary = single_line(&capped(Some(summary), limit));
    let status_color = match (action.status, termination) {
        (
            WorkspaceScriptActionStatus::Cancelled
            | WorkspaceScriptActionStatus::Blocked
            | WorkspaceScriptActionStatus::Unknown,
            _,
        )
        | (_, Some(CommandTermination::Cancelled | CommandTermination::TimedOut)) => WARNING,
        (WorkspaceScriptActionStatus::Error, _) => ERROR,
        _ if completed(action) => SUCCESS,
        _ => ERROR,
    };
    let mut line = Line::from(vec![
        Span::styled(name, operation),
        Span::styled(" · ", muted),
        Span::styled(state, tool_status(status_color)),
    ]);
    let truncation = if action.result["capture_complete"] == false {
        Some("capture incomplete")
    } else if action.truncated {
        Some("evidence truncated")
    } else {
        None
    };
    if let Some(truncation) = truncation {
        line.push_span(Span::styled(" · ", muted));
        line.push_span(Span::styled(truncation, Style::new().fg(WARNING)));
    }
    if !summary.is_empty() {
        line.push_span(Span::styled(" · ", muted));
        line.push_span(Span::raw(summary));
    }
    line
}

fn changed_file(action: &WorkspaceScriptActionFacts) -> Option<FileMutationFacts> {
    if !matches!(
        action.result["kind"].as_str(),
        Some("file_edit" | "file_write" | "file_remove")
    ) {
        return None;
    }
    FileMutationFacts::from_structured(&action.result).ok()
}

fn termination(action: &WorkspaceScriptActionFacts) -> Option<CommandTermination> {
    serde_json::from_value(action.result.get("termination")?.clone()).ok()
}

fn command_state(termination: CommandTermination, result: &Value) -> Cow<'static, str> {
    match termination {
        CommandTermination::Exited => result["exit_code"]
            .as_i64()
            .map(|code| format!("exit {code}"))
            .or_else(|| {
                result["signal"]
                    .as_i64()
                    .map(|signal| format!("signal {signal}"))
            })
            .map_or(Cow::Borrowed("exit status unavailable"), Cow::Owned),
        CommandTermination::ProgramMissing => "program missing".into(),
        CommandTermination::StartFailed => "could not start".into(),
        CommandTermination::TimedOut => "timed out".into(),
        CommandTermination::Cancelled => "cancelled".into(),
    }
}

fn status_label(status: WorkspaceScriptActionStatus) -> &'static str {
    match status {
        WorkspaceScriptActionStatus::Ok => "completed",
        WorkspaceScriptActionStatus::Error => "error",
        WorkspaceScriptActionStatus::Blocked => "blocked",
        WorkspaceScriptActionStatus::Cancelled => "cancelled",
        WorkspaceScriptActionStatus::Unknown => "effects unknown",
    }
}
