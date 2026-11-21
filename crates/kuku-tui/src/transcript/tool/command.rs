use kuku_client::{CommandExecutionFacts, CommandInvocation, CommandShell, CommandTermination};
use ratatui::style::Color;
use ratatui::text::Line;
use serde_json::Value;

use crate::text::single_line;
use crate::theme::{CardFrame, ERROR, SUCCESS, SURFACE_TEXT, WARNING};

use super::{arguments_json, capped, Row, Tool, MAX_DETAIL_BYTES};

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

    pub(super) fn elapsed(&self) -> String {
        let ms = self.facts.duration_ms;
        if ms < 1_000 {
            format!("{ms}ms")
        } else {
            format!("{:.1}s", ms as f64 / 1_000.0)
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
        let mut notes = Vec::new();
        if self.facts.quota_truncated {
            notes.push("Capture truncated at the byte limit");
        } else if !self.facts.capture_complete {
            notes.push("Capture incomplete");
        }
        if self.facts.cleanup_incomplete {
            notes.push("Process or pipe cleanup incomplete");
        }
        if self.facts.output_preview_truncated {
            notes.push("Output preview truncated");
        }
        if self.facts.lossy_utf8 {
            notes.push("Display replaced invalid UTF-8");
        }
        notes.join("\n")
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
    pub(super) fn command_heading(&self, frame: &CardFrame) -> Row {
        let title = if self.start_summary.is_empty() {
            "Run command"
        } else {
            &self.start_summary
        };
        Row::plain(frame.title(&single_line(title), SURFACE_TEXT))
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

pub(super) fn finish(
    frame: CardFrame,
    mut rows: Vec<Row>,
    (status, color): (String, Color),
) -> Vec<Row> {
    for row in rows.iter_mut().skip(1) {
        row.line = frame.line(std::mem::take(&mut row.line));
        row.inset = frame.content_inset() as u16;
    }
    Row::pad_framed_body(&mut rows, &frame);
    rows.push(Row::plain(frame.status_foot(&status, color)));
    rows.push(Row::plain(Line::default()));
    rows
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
