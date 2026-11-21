use std::borrow::Cow;
use std::collections::HashSet;

use kuku_client::{
    CommandTermination, FileCommitState, FileMutationFacts, FileMutationKind,
    WorkspaceScriptActionFacts, WorkspaceScriptActionStatus, WorkspaceScriptFacts,
    WorkspaceScriptOperation,
};
use ratatui::style::Color;
use serde_json::Value;

use crate::text::single_line;
use crate::theme::{ACCENT, MUTED, SURFACE_TEXT, WARNING};

use super::command::CommandPreview;
use super::{
    arguments_json, capped, finish_card, render_preview, tool_title, Row, Tool, ToolDetails,
    ToolView, MAX_DETAIL_BYTES, MAX_INSPECTION_ROWS, MAX_RESULT_ROWS, MAX_WARNING_ROWS,
};

#[derive(Debug, Clone)]
pub(super) struct ScriptPreview {
    facts: WorkspaceScriptFacts,
}

impl ScriptPreview {
    pub(super) fn from_result(structured: &Value) -> Option<Box<Self>> {
        if structured["kind"].as_str() != Some("workspace_script_result") {
            return None;
        }
        let facts = WorkspaceScriptFacts::from_structured(structured).ok()?;
        Some(Box::new(Self { facts }))
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

    pub(super) fn summary(&self) -> String {
        let actions = &self.facts.actions;
        let complete = actions
            .iter()
            .filter(|action| completed(action))
            .map(action_count)
            .fold(0, u64::saturating_add);
        let changes = actions
            .iter()
            .filter(|action| changed_file(action).is_some())
            .count();
        let total = actions
            .iter()
            .map(action_count)
            .fold(self.facts.not_dispatched as u64, u64::saturating_add);
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

    pub(super) fn evidence(&self, status: &str) -> String {
        let mut lines = vec![self.summary()];
        lines.extend(self.notes(status));
        lines.extend(self.facts.actions.iter().map(action_detail));
        let value = self.returned_value();
        if !value.is_empty() {
            lines.push(value);
        }
        lines.join("\n")
    }

    pub(super) fn outputs(&self) -> Vec<ToolView> {
        let mut seen = HashSet::new();
        self.facts
            .actions
            .iter()
            .enumerate()
            .filter_map(|(index, action)| {
                let output_id = action.result["output_ref"]["id"].as_str()?;
                if !seen.insert(output_id) {
                    return None;
                }
                let name = format!(
                    "Script action {} · {}",
                    index + 1,
                    tool_title(action.tool.name())
                );
                let command = CommandPreview::from_result(&action.result);
                let invocation = command
                    .as_ref()
                    .map_or_else(String::new, |command| command.invocation());
                let evidence = command
                    .as_ref()
                    .map_or_else(String::new, |command| command.evidence());
                Some(ToolView {
                    name,
                    summary: action_detail(action),
                    invocation,
                    evidence,
                    output_id: Some(output_id.to_owned()),
                    ..ToolView::default()
                })
            })
            .collect()
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

    fn returned_value(&self) -> String {
        if self.facts.value.is_null() {
            String::new()
        } else {
            format!("Script returned {}", arguments_json(&self.facts.value))
        }
    }
}

impl Tool {
    pub(super) fn render_script(
        &self,
        script: &ScriptPreview,
        status: &str,
        width: usize,
        inset: u16,
        details: ToolDetails,
    ) -> Vec<Row> {
        let summary = script.summary();
        let mut rows = self.render_header(
            width,
            inset,
            Some(&summary)
                .filter(|summary| !summary.is_empty())
                .map(String::as_str),
        );
        for note in script.notes(status) {
            rows.extend(render_preview(&note, WARNING, width, 1, inset));
        }
        if status != "ok" && !self.summary().is_empty() {
            rows.extend(render_preview(
                self.summary(),
                WARNING,
                width,
                MAX_WARNING_ROWS,
                inset,
            ));
        }
        if details == ToolDetails::Collapsed {
            return finish_card(rows);
        }
        if details == ToolDetails::Inspection {
            self.render_arguments(&mut rows, width, inset);
        }
        let limit = if details == ToolDetails::Inspection {
            MAX_INSPECTION_ROWS
        } else {
            MAX_RESULT_ROWS
        };
        let actions = &script.facts.actions;
        let evidence = actions
            .iter()
            .filter(|action| important(action))
            .chain(actions.iter().filter(|action| !important(action)))
            .take(limit)
            .map(action_detail)
            .collect::<Vec<_>>()
            .join("\n");
        rows.extend(render_preview(&evidence, SURFACE_TEXT, width, limit, inset));
        rows.extend(render_preview(
            &script.returned_value(),
            MUTED,
            width,
            1,
            inset,
        ));
        if !actions.is_empty() {
            let hint = if actions.iter().any(has_output) {
                "Actions and saved output · Messages"
            } else {
                "Actions · Messages"
            };
            rows.extend(render_preview(hint, ACCENT, width, 1, inset));
        }
        finish_card(rows)
    }
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

fn action_detail(action: &WorkspaceScriptActionFacts) -> String {
    if let Some(change) = changed_file(action) {
        let verb = match change.kind {
            FileMutationKind::FileEdit => "Edited",
            FileMutationKind::FileWrite => "Wrote",
            FileMutationKind::FileRemove => "Removed",
        };
        let warning = if change.commit_state == FileCommitState::PublishedUnconfirmed {
            " · durability unconfirmed"
        } else {
            ""
        };
        return format!("{verb} {}{warning}", single_line(&change.path));
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
    let state = state.as_ref();
    let truncation = if action.result["capture_complete"] == false {
        " · capture incomplete"
    } else if action.truncated {
        " · evidence truncated"
    } else {
        ""
    };
    if summary.is_empty() {
        format!("{name} · {state}{truncation}")
    } else {
        format!("{name} · {state}{truncation} · {summary}")
    }
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
