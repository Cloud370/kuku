use kuku_client::{CommandExecutionFacts, CommandInvocation, CommandShell, CommandTermination};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

use crate::markup::{render_literal, wrap_numbered};
use crate::text::{cap, single_line};
use crate::theme::{
    ERROR, MUTED, SUCCESS, SURFACE_TEXT, TranscriptTone, WARNING, tool_operation_color,
};
use crate::transcript::MAX_VISUAL_ROWS;

use super::super::details::{InlineOutput, InlineOutputAction, OutputTarget};
use super::{
    MAX_DETAIL_BYTES, MAX_INVOCATION_ROWS, MAX_PREVIEW_BYTES, MAX_RESULT_ROWS, MAX_WARNING_ROWS,
    Row, Tool, ToolDetails, ToolView, arguments_json, capped, finish_card, render_excerpt,
    render_preview, render_rows,
};

#[derive(Debug, Clone)]
pub(super) struct CommandPreview {
    facts: CommandExecutionFacts,
}

impl CommandPreview {
    pub(super) fn from_result(structured: &Value) -> Option<Box<Self>> {
        if structured["kind"].as_str() != Some("command_result") {
            return None;
        }
        let facts = CommandExecutionFacts::from_structured(structured).ok()?;
        Some(Box::new(Self { facts }))
    }

    pub(super) fn label(&self) -> (&'static str, Color) {
        match self.facts.termination {
            CommandTermination::Exited => match self.facts.exit_code {
                Some(0) if self.attention() => ("ok with warnings", WARNING),
                Some(0) => ("ok", SUCCESS),
                Some(_) => ("failed", ERROR),
                None if self.facts.signal.is_some() => ("terminated", WARNING),
                None => ("exit status unavailable", WARNING),
            },
            CommandTermination::TimedOut => ("timed out", WARNING),
            CommandTermination::Cancelled => ("cancelled", WARNING),
            CommandTermination::ProgramMissing => ("program missing", ERROR),
            CommandTermination::StartFailed => ("could not start", ERROR),
        }
    }

    pub(super) fn invocation(&self) -> String {
        match &self.facts.invocation {
            CommandInvocation::Program { program, args } => {
                let args = serde_json::to_value(args).unwrap_or_default();
                render_program(program, &args)
            }
            CommandInvocation::Shell { shell, command } => {
                render_shell(shell_name(*shell), command)
            }
        }
    }

    pub(super) fn exit_code(&self) -> Option<i64> {
        self.facts.exit_code.map(i64::from)
    }

    pub(super) fn exit_label(&self) -> &'static str {
        match self.facts.invocation {
            CommandInvocation::Program { .. } => "exit",
            CommandInvocation::Shell { .. } => "shell exit",
        }
    }

    pub(super) fn elapsed_metric(&self) -> Line<'static> {
        let ms = self.facts.duration_ms;
        if ms < 1_000 {
            Row::metric(" · ", ms.to_string(), "ms", TranscriptTone::Value)
        } else {
            Row::metric(
                " · ",
                format!("{:.1}", ms as f64 / 1_000.0),
                "s",
                TranscriptTone::Value,
            )
        }
    }

    pub(super) fn has_stderr(&self) -> bool {
        self.facts.stderr_bytes > 0
    }

    pub(super) fn signal(&self) -> Option<i32> {
        self.facts.signal
    }

    pub(super) fn body(&self) -> &str {
        let body = self.facts.output_preview.trim_end_matches(['\r', '\n']);
        if self.concise() && self.facts.stdout_bytes == 0 {
            return body.strip_prefix("stderr:\n").unwrap_or(body);
        }
        body
    }

    pub(super) fn concise(&self) -> bool {
        self.facts.termination == CommandTermination::Exited
            && self
                .facts
                .exit_code
                .is_some_and(|code| code != 0 || !(self.attention() || self.has_stderr()))
            && self.notes().is_empty()
            && self.facts.reported_tests.is_empty()
    }

    pub(super) fn summary(&self) -> String {
        self.facts
            .reported_tests
            .iter()
            .map(|report| {
                format!(
                    "{:?} reported {} passed, {} failed, {} skipped",
                    report.format, report.passed, report.failed, report.skipped,
                )
            })
            .collect::<Vec<_>>()
            .join(" · ")
    }

    pub(super) fn notes(&self) -> String {
        let mut notes = self.capture_notes();
        if self.facts.output_summary.is_some() {
            notes.push("Raw output retained");
        } else if self.facts.output_preview_truncated {
            notes.push("Output preview truncated");
        }
        notes.join("\n")
    }

    fn capture_notes(&self) -> Vec<&'static str> {
        let mut notes = Vec::new();
        if self.facts.quota_truncated {
            notes.push("Capture truncated at the byte limit");
        } else if !self.facts.capture_complete {
            notes.push("Capture incomplete");
        }
        if self.facts.cleanup_incomplete {
            notes.push("Process or pipe cleanup incomplete");
        }
        if self.facts.lossy_utf8 {
            notes.push("Display replaced invalid UTF-8");
        }
        notes
    }

    pub(super) fn format_output(&self, mut view: ToolView) -> ToolView {
        view.invocation = self.invocation();
        view.summary = self.summary();
        view.evidence = self.capture_notes().join("\n");
        if view.output_id.is_none() && !self.body().is_empty() {
            view.details = self.body().to_owned();
        }
        view
    }

    pub(super) fn inspection_rows(
        &self,
        title: &str,
        width: usize,
        output: Option<OutputTarget>,
        arguments: Option<bool>,
        inline_output: Option<&InlineOutput>,
    ) -> Vec<Row> {
        let columns = Row::detail_width(width);
        let invocation = single_line(&self.invocation());
        let mut rows = Vec::new();
        if !invocation.is_empty() && invocation != title {
            rows.extend(render_preview(
                &invocation,
                tool_operation_color("run_command"),
                columns,
                MAX_INVOCATION_ROWS,
                0,
            ));
        }
        if let Some(error) = &self.facts.start_error {
            rows.extend(render_preview(
                &single_line(error),
                ERROR,
                columns,
                MAX_WARNING_ROWS,
                0,
            ));
        }
        if let Some(open) = arguments {
            rows.push(Row::command_arguments(open, columns));
            if open {
                rows.extend(render_rows(
                    self.argument_fields(),
                    columns,
                    MAX_VISUAL_ROWS,
                    0,
                ));
            }
        }
        let body = self.body();
        let color = if self.facts.stdout_bytes == 0 && self.has_stderr() {
            WARNING
        } else {
            TranscriptTone::Text.color()
        };
        let lines = self
            .facts
            .stdout_lines
            .saturating_add(self.facts.stderr_lines);
        let line_count = usize::try_from(lines).unwrap_or(usize::MAX);
        let preview = if body.is_empty() || inline_output.is_some() {
            Vec::new()
        } else {
            let source = render_excerpt(body, |text| render_literal(text, Style::new().fg(color)));
            wrap_numbered(source, columns, MAX_RESULT_ROWS, 1, line_count)
        };
        if !preview.is_empty() || output.is_some() {
            let limited = !preview.is_empty()
                && (self.facts.output_preview_truncated
                    || self.facts.output_summary.is_some()
                    || preview.len() > MAX_RESULT_ROWS
                    || cap(body, MAX_PREVIEW_BYTES).len() < body.len());
            let heading = if limited && inline_output.is_none() {
                "Output preview"
            } else {
                "Output"
            };
            if lines > 0 {
                let unit = if lines == 1 { " line" } else { " lines" };
                let label = Row::metric(
                    format!("{heading} · "),
                    lines.to_string(),
                    unit,
                    TranscriptTone::Value,
                );
                rows.push(Row::plain(Row::fit(label, columns)));
            } else {
                rows.push(Row::caption(heading, "", MUTED, columns));
            }
        }
        if let Some(target) = output {
            let action = if inline_output.is_some() {
                InlineOutputAction::Close
            } else {
                InlineOutputAction::Open
            };
            rows.push(Row::inline_output(target, action, columns));
        }
        if let Some(output) = inline_output {
            rows.extend(output.rows(columns, line_count));
        } else {
            rows.extend(preview.into_iter().map(Row::plain));
        }
        rows.extend(render_preview(
            &self.capture_notes().join("\n"),
            WARNING,
            columns,
            MAX_WARNING_ROWS,
            0,
        ));
        rows.into_iter()
            .map(|row| row.with_detail_rule(width))
            .collect()
    }

    fn argument_fields(&self) -> Vec<Line<'static>> {
        let field = |label: &str, value: String| {
            Line::from(vec![
                Span::styled(format!("{label:<11}"), TranscriptTone::Label.style()),
                Span::styled(value, TranscriptTone::Text.style()),
            ])
        };
        let mut rows =
            match &self.facts.invocation {
                CommandInvocation::Program { program, args } => {
                    let mut rows = vec![field("Program", display_argument(program))];
                    rows.extend(args.iter().take(MAX_VISUAL_ROWS).enumerate().map(
                        |(index, arg)| {
                            field(
                                if index == 0 { "Arguments" } else { "" },
                                display_argument(arg),
                            )
                        },
                    ));
                    rows
                }
                CommandInvocation::Shell { shell, command } => vec![
                    field("Shell", shell_name(*shell).to_owned()),
                    field("Command", display_argument(command)),
                ],
            };
        rows.push(field("Directory", single_line(&self.facts.cwd)));
        rows
    }

    pub(super) fn evidence(&self) -> String {
        let mut lines = vec![format!(
            "{} ms · {} stdout bytes · {} stderr bytes",
            self.facts.duration_ms, self.facts.stdout_bytes, self.facts.stderr_bytes,
        )];
        lines.push(format!("Directory · {}", self.facts.cwd));
        if let Some(error) = &self.facts.start_error {
            lines.push(error.clone());
        }
        let notes = self.notes();
        if !notes.is_empty() {
            lines.push(notes);
        }
        for report in &self.facts.reported_tests {
            if let Some(failure) = &report.first_failure {
                lines.push(format!("First reported failure · {failure}"));
            }
            lines.extend(report.evidence.iter().cloned());
            if report.evidence_truncated {
                lines.push("Test report evidence truncated".into());
            }
        }
        lines.join("\n")
    }

    fn attention(&self) -> bool {
        !self.facts.capture_complete
            || self.facts.cleanup_incomplete
            || self
                .facts
                .reported_tests
                .iter()
                .any(|report| report.failed > 0)
    }
}

impl Tool {
    pub(super) fn render_short_command(
        &self,
        command: &CommandPreview,
        width: usize,
        details: ToolDetails,
    ) -> Vec<Row> {
        let mut rows = self.command_heading(width);
        if details != ToolDetails::Collapsed {
            rows.extend(render_preview(command.body(), SURFACE_TEXT, width, 3, 0));
        }
        finish_card(rows)
    }

    pub(super) fn render_command_inspection(
        &self,
        command: &CommandPreview,
        width: usize,
        in_group: bool,
        arguments_open: bool,
        inline_output: Option<&InlineOutput>,
    ) -> Vec<Row> {
        let mut rows = if in_group {
            self.render_activity(width, 1)
        } else {
            self.command_heading(width)
        };
        let title = single_line(self.command_title());
        let output = self.view().has_full_output().then_some(OutputTarget::Tool);
        rows.extend(command.inspection_rows(
            &title,
            width,
            output,
            Some(arguments_open),
            inline_output,
        ));
        finish_card(rows)
    }

    pub(in crate::transcript) fn format_output(&self, mut view: ToolView) -> ToolView {
        let Some(command) = self
            .state
            .outcome()
            .and_then(|outcome| outcome.command_preview.as_deref())
        else {
            return view;
        };
        view.name = single_line(self.command_title());
        command.format_output(view)
    }

    pub(super) fn command_title(&self) -> &str {
        if !self.request_title.is_empty() {
            return &self.request_title;
        }
        let invocation = self.invocation();
        if !invocation.is_empty() && invocation != self.arguments {
            invocation
        } else if !self.start_summary.is_empty() {
            &self.start_summary
        } else {
            "Run command"
        }
    }

    pub(super) fn command_heading(&self, width: usize) -> Vec<Row> {
        let (status, _) = self.status_line();
        Row::styled_status(
            &single_line(self.command_title()),
            status,
            tool_operation_color("run_command"),
            width,
        )
    }
}

fn display_argument(argument: &str) -> String {
    if argument.is_empty()
        || argument
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control() || matches!(ch, '"' | '\\'))
    {
        format!("{argument:?}")
    } else {
        argument.to_owned()
    }
}

pub(super) fn invocation(args: &Value) -> String {
    if let Some(program) = args["program"].as_str() {
        render_program(program, &args["args"])
    } else if let (Some(shell), Some(command)) = (args["shell"].as_str(), args["command"].as_str())
    {
        render_shell(shell, command)
    } else {
        String::new()
    }
}

pub(super) fn exit_metric(label: &str, code: i64) -> Line<'static> {
    let tone = if code == 0 {
        TranscriptTone::Success
    } else {
        TranscriptTone::Error
    };
    Row::metric(format!(" · {label} "), code.to_string(), "", tone)
}

fn render_program(program: &str, args: &Value) -> String {
    let program = capped(Some(program), MAX_DETAIL_BYTES);
    if !args.as_array().is_some_and(|args| !args.is_empty()) {
        return program;
    }
    capped(
        Some(&format!("{program} {}", arguments_json(args))),
        MAX_DETAIL_BYTES,
    )
}

fn render_shell(shell: &str, command: &str) -> String {
    let shell = single_line(&capped(Some(shell), MAX_DETAIL_BYTES / 2));
    let command = capped(Some(command), MAX_DETAIL_BYTES / 2);
    format!("{shell} · {command}")
}

fn shell_name(shell: CommandShell) -> &'static str {
    match shell {
        CommandShell::Sh => "sh",
        CommandShell::Bash => "bash",
        CommandShell::Zsh => "zsh",
        CommandShell::Cmd => "cmd",
        CommandShell::Powershell => "powershell",
        CommandShell::Pwsh => "pwsh",
    }
}
