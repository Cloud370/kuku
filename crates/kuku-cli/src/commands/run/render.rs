use std::io::{self, IsTerminal, Write};
use std::time::Instant;

use kuku::ModelRecoveryInfo;
use kuku_client::{Acknowledgement, HistoryEntry, HistoryKind, Target};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::cli_args::RunArgs;
use crate::commands::CliResult;
use crate::display::{Display, OutputLine};

use super::{RunOutcome, RunProgress, RunResult, permission_ask_line};

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Preview {
    Text {
        content: String,
    },
    Thinking {
        content: String,
    },
    ToolStart {
        id: String,
        tool: String,
        summary: String,
    },
    ToolEnd {
        id: String,
        status: String,
        summary: String,
    },
    ModelRecovery {
        #[serde(flatten)]
        info: ModelRecoveryInfo,
    },
    ToolOutput,
    Error {
        code: String,
        message: String,
    },
    Permission,
    Done,
    Cancelled,
    TurnStart,
    ModelRequest,
    #[serde(other)]
    Other,
}

pub(super) struct RunRenderer {
    display: Display,
    json: bool,
    stream_json: bool,
    verbose: bool,
    rendered_text: String,
    in_thinking: bool,
    thinking_start: Option<Instant>,
}

impl RunRenderer {
    pub(super) fn new(args: &RunArgs) -> Self {
        let reasoning = args.reasoning_effort.as_deref().unwrap_or("default");
        let display = if args.raw || !io::stdout().is_terminal() {
            Display::new_raw(args.show_thinking, reasoning)
        } else {
            Display::new(args.show_thinking, reasoning)
        };
        Self {
            display,
            json: args.json,
            stream_json: args.stream_json,
            verbose: args.verbose,
            rendered_text: String::new(),
            in_thinking: false,
            thinking_start: None,
        }
    }

    pub(super) fn streaming(&self) -> bool {
        self.stream_json
    }

    pub(super) fn needs_preview(&self) -> bool {
        !self.json
    }

    pub(super) fn event(&mut self, event: Value) -> CliResult {
        if self.json {
            return Ok(());
        }
        let Ok(preview) = serde_json::from_value(event.clone()) else {
            return Ok(());
        };
        match preview {
            Preview::Text { content: text } => {
                self.close_thinking();
                self.rendered_text.push_str(&text);
                if self.stream_json {
                    write_json(&OutputLine::text_delta(text))?;
                } else {
                    print!("{text}");
                    let _ = io::stdout().flush();
                }
            }
            Preview::Thinking { content: text } => {
                if !self.in_thinking {
                    self.in_thinking = true;
                    self.thinking_start = Some(Instant::now());
                    if !self.stream_json {
                        println!("\n{}", self.display.thinking_start());
                    }
                }
                if !self.stream_json {
                    if let Some(rendered) = self.display.thinking_line(&text) {
                        print!("{rendered}");
                    }
                }
            }
            Preview::ToolStart { id, tool, summary } => {
                self.close_thinking();
                if !self.stream_json {
                    println!("\n{}", self.display.tool_call(&tool, &summary, &id));
                }
            }
            Preview::ToolEnd {
                id,
                status,
                summary,
            } => {
                if !self.stream_json {
                    println!("{}", self.display.tool_result(&status, &summary, &id));
                }
            }
            Preview::ModelRecovery { info } => self.recovery(info)?,
            Preview::ToolOutput => {
                if self.stream_json {
                    write_json(&event)?;
                }
            }
            Preview::Error { code, message } => eprintln!("{code}: {message}"),
            Preview::Permission
            | Preview::Done
            | Preview::Cancelled
            | Preview::TurnStart
            | Preview::ModelRequest => {}
            Preview::Other => {
                if self.stream_json {
                    write_json(&event)?;
                }
            }
        }
        Ok(())
    }

    pub(super) fn committed(&mut self, entry: &HistoryEntry) -> CliResult {
        if entry.kind == HistoryKind::JobFinished {
            if self.stream_json {
                write_json(entry)?;
            } else if !self.json {
                let detail = |key| entry.details[key].as_str().unwrap_or_default();
                println!(
                    "Job {} {} · {}",
                    detail("job_id"),
                    detail("status"),
                    detail("summary")
                );
            }
        } else if self.stream_json {
            if let Some(result) = tool_record(entry) {
                write_json(&result)?;
            }
        }
        Ok(())
    }

    pub(super) fn acknowledged(
        &self,
        target: &Target,
        acknowledgement: &Acknowledgement,
    ) -> CliResult {
        if self.json || self.stream_json {
            write_json(&json!({
                "type": "session", "event": "acknowledged", "status": "acknowledged",
                "session_id": target.session_id, "agent": target.agent, "replayed": true,
                "receipt": acknowledgement.receipt, "input_status": acknowledgement.status,
            }))?;
        } else {
            let receipt = &acknowledgement.receipt;
            eprintln!(
                "command {} already committed at event {}. Session {} agent {}",
                receipt.command_id, receipt.event_id, target.session_id, target.agent,
            );
        }
        Ok(())
    }

    pub(super) fn finish(
        &mut self,
        progress: &RunProgress,
        agent: &str,
        report: &RunResult,
        outcome: RunOutcome,
        elapsed: std::time::Duration,
    ) -> CliResult {
        self.close_thinking();
        let response = report.response.as_deref();
        if self.json || self.stream_json {
            if let Some(request) = &report.permission {
                if self.stream_json {
                    write_json(&permission_ask_line(request))?;
                }
            }
            #[derive(Serialize)]
            struct FinalRecord<'a> {
                #[serde(flatten)]
                session: Value,
                #[serde(flatten)]
                result: &'a RunResult,
                #[serde(skip_serializing_if = "Option::is_none")]
                connection: Option<&'a str>,
                #[serde(skip_serializing_if = "Option::is_none")]
                model: Option<&'a str>,
            }
            let route = report.last_model_route.as_ref();
            write_json(&FinalRecord {
                session: terminal_record(progress, agent, outcome, elapsed),
                result: report,
                connection: route.map(|route| route.connection.as_str()),
                model: route.map(|route| route.model.as_str()),
            })?;
        } else {
            if let Some(output) = response {
                if let Some(suffix) = output.strip_prefix(&self.rendered_text) {
                    print!("{suffix}");
                } else if !self.rendered_text.ends_with(output) {
                    println!("\n{output}");
                }
            }
            println!();
            let session_id = progress.session_id.as_deref().unwrap_or_default();
            eprintln!("session {session_id} agent {agent}: {}", outcome.as_str());
            if let Some(request) = &report.permission {
                eprintln!("pending authorization {}: {}", request.id, request.summary);
            }
            if let Some(request) = &report.question {
                eprintln!("pending question {}: {}", request.id, request.text);
            }
            if self.verbose {
                eprintln!(
                    "{}",
                    serde_json::to_string_pretty(report).unwrap_or_default()
                );
            }
        }
        Ok(())
    }

    pub(super) fn finish_error(
        &mut self,
        progress: &RunProgress,
        agent: &str,
        error: &(dyn std::error::Error + 'static),
        elapsed: std::time::Duration,
    ) -> CliResult {
        self.close_thinking();
        if self.json || self.stream_json {
            let outcome = error
                .downcast_ref::<RunOutcome>()
                .copied()
                .unwrap_or(RunOutcome::Failed);
            let mut result = terminal_record(progress, agent, outcome, elapsed);
            if outcome == RunOutcome::InputWithdrawn {
                result["input_status"] = json!("withdrawn");
            } else if outcome == RunOutcome::Failed {
                result["error"] = json!(error.to_string());
            }
            write_json(&result)?;
        }
        Ok(())
    }

    fn close_thinking(&mut self) {
        if self.in_thinking {
            self.in_thinking = false;
            let elapsed = self
                .thinking_start
                .take()
                .map(|start| start.elapsed())
                .unwrap_or_default();
            if !self.json && !self.stream_json {
                println!("{}", self.display.thinking_end(elapsed));
            }
        }
    }

    fn recovery(&mut self, info: kuku::query::ModelRecoveryInfo) -> CliResult {
        self.close_thinking();
        self.rendered_text.clear();
        if self.stream_json {
            write_json(&OutputLine::model_recovery(info))?;
        } else {
            println!("{}", self.display.model_recovery(&info));
        }
        Ok(())
    }
}

fn terminal_record(
    progress: &RunProgress,
    agent: &str,
    outcome: RunOutcome,
    elapsed: std::time::Duration,
) -> Value {
    let status = outcome.as_str();
    json!({
        "type": "session", "event": status, "status": status,
        "session_id": progress.session_id, "agent": agent, "duration_ms": elapsed.as_millis(),
        "receipt": progress.acknowledgement.as_ref().map(|ack| &ack.receipt),
        "replayed": progress.acknowledgement.as_ref().is_some_and(|ack| ack.replayed),
    })
}

pub(super) fn write_json(record: &impl Serialize) -> CliResult {
    let mut line = serde_json::to_vec(record)?;
    line.push(b'\n');
    let mut stdout = io::stdout().lock();
    stdout.write_all(&line)?;
    stdout.flush()?;
    Ok(())
}

pub(super) fn tool_record(entry: &HistoryEntry) -> Option<OutputLine> {
    let text = |key| entry.details.get(key).and_then(Value::as_str);
    let id = text("tool_call_id")?.to_owned();
    let record = match entry.kind {
        HistoryKind::ToolCall => {
            let name = text("tool")?;
            let args = entry.details.get("args").cloned().unwrap_or_default();
            let summary = args["brief"].as_str().unwrap_or(name).to_owned();
            OutputLine::tool_call(name.to_owned(), id, summary, args)
        }
        HistoryKind::ToolResult => OutputLine::tool_result(
            id,
            text("status")?.to_owned(),
            text("summary").unwrap_or_default().to_owned(),
            text("model_content").map(str::to_owned),
            entry
                .details
                .get("truncated")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        )
        .with_structured(
            entry
                .details
                .get("structured")
                .filter(|value| !value.is_null())
                .cloned(),
        ),
        HistoryKind::User
        | HistoryKind::Assistant
        | HistoryKind::Question
        | HistoryKind::Permission
        | HistoryKind::QuestionReply
        | HistoryKind::PermissionReply
        | HistoryKind::Error
        | HistoryKind::Status
        | HistoryKind::TurnSummary
        | HistoryKind::JobFinished
        | HistoryKind::Retry
        | HistoryKind::Compaction
        | HistoryKind::Other => return None,
    };
    Some(record.with_conversation(Some(entry.agent.clone())))
}
