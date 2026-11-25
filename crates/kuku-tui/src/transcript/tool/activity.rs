use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::text::single_line;
use crate::theme::{CardFrame, ERROR, MUTED, TranscriptTone, WARNING};

use super::{InlineOutput, Row, ScriptDetail, State, Tool, ToolDetails, tool_operation_color};

impl Tool {
    pub(in crate::transcript) fn activity_has_changes(&self) -> bool {
        self.file_change().is_some()
            || self
                .state
                .outcome()
                .and_then(|outcome| outcome.script.as_ref())
                .is_some_and(|script| script.has_changes())
    }

    pub(in crate::transcript) fn render(
        &self,
        width: usize,
        details: ToolDetails,
        in_group: bool,
        script_detail: Option<ScriptDetail>,
        inline_output: Option<&InlineOutput>,
    ) -> Vec<Row> {
        if in_group {
            return self.render_body(width, details, true, script_detail, inline_output);
        }
        let frame = CardFrame::timeline(width);
        Row::card(
            self.render_body(
                frame.content_width(),
                details,
                false,
                script_detail,
                inline_output,
            ),
            &frame,
        )
    }

    pub(in crate::transcript) fn workspace_activity(&self) -> bool {
        !matches!(self.name(), "list_agents" | "query_session" | "ask_user")
            && self.agent_card().is_none()
    }

    pub(in crate::transcript) fn activity_changes(&self) -> usize {
        if self.file_change().is_some() {
            1
        } else {
            self.state
                .outcome()
                .and_then(|outcome| outcome.script.as_ref())
                .map_or(0, |script| script.file_change_count())
        }
    }

    pub(in crate::transcript) fn activity_running(&self) -> bool {
        matches!(self.state, State::Running)
    }

    pub(in crate::transcript) fn activity_needs_attention(&self) -> bool {
        matches!(self.state.label().1, ERROR | WARNING)
    }

    pub(in crate::transcript) fn activity_result_missing(&self) -> bool {
        matches!(self.state, State::Requested)
    }

    pub(in crate::transcript) fn activity_repeat_key(&self) -> Option<(&str, &str, &str)> {
        if matches!(self.state, State::Requested) {
            return Some((self.name(), "requested", ""));
        }
        let (status, color) = self.state.label();
        let reason = self.activity_diagnostic();
        (matches!(color, ERROR | WARNING) && !reason.is_empty()).then_some((
            self.name(),
            status,
            reason,
        ))
    }

    pub(super) fn activity_diagnostic(&self) -> &str {
        if let Some(reason) = self
            .state
            .outcome()
            .and_then(|outcome| outcome.script.as_ref())
            .and_then(|script| script.first_failed_action())
        {
            return reason;
        }
        self.result()
            .lines()
            .find(|line| {
                !line.trim().is_empty() && !line.starts_with("Host action results observed:")
            })
            .unwrap_or_else(|| self.summary())
    }

    pub(super) fn render_activity(&self, width: usize, count: usize) -> Vec<Row> {
        if let Some(change) = self.file_change() {
            return self.render_change(change, width, ToolDetails::Collapsed);
        }
        let (mut status, mut color) = self.status_line();
        if self.activity_result_missing() {
            status = Line::from(Span::styled("result not loaded", Style::new().fg(MUTED)));
            color = MUTED;
        }
        let operation = match self.name() {
            "read_file" => "Read",
            "write_file" => "Write",
            "edit_file" => "Edit",
            "remove_file" => "Remove",
            "run_command" => "Run",
            "find_files" => "Find",
            "search_text" => "Search",
            _ => "Tool",
        };
        let diagnostic = if matches!(color, ERROR | WARNING) {
            self.activity_diagnostic()
        } else {
            ""
        };
        let title = if self.name() == "run_command" {
            self.command_title().into()
        } else {
            self.title()
        };
        let target = self.invocation();
        let subject = if count > 1 {
            if diagnostic.is_empty() {
                title.into_owned()
            } else {
                diagnostic.to_owned()
            }
        } else if matches!(
            self.name(),
            "read_file" | "write_file" | "edit_file" | "remove_file"
        ) && !target.is_empty()
        {
            let reason = diagnostic
                .strip_prefix("invalid argument: ")
                .unwrap_or(diagnostic);
            if reason.is_empty() {
                target.to_owned()
            } else if let Some(prefix) = reason.strip_suffix(target) {
                prefix
                    .strip_suffix(": ")
                    .map_or_else(|| reason.to_owned(), |cause| format!("{target} · {cause}"))
            } else {
                format!("{target} · {reason}")
            }
        } else if diagnostic.is_empty() || diagnostic == title {
            title.into_owned()
        } else {
            format!("{title} · {diagnostic}")
        };
        if count > 1 {
            status.extend(Row::metric(" ×", count.to_string(), "", TranscriptTone::Value).spans);
        }
        let mut rows = vec![Row::activity_columns(
            operation,
            tool_operation_color(self.name()),
            &single_line(&subject),
            status,
            width,
        )];
        if self.name() == "run_command" {
            rows.extend(self.background_handoff_rows(width));
        }
        rows
    }
}
