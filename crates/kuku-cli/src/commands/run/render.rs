use std::io::{self, IsTerminal, Write};
use std::time::Instant;

use kuku::ModelRecoveryInfo;
use kuku_client::{AgentSnapshot, AgentStatus, HistoryEntry, HistoryKind};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::cli_args::RunArgs;
use crate::commands::CliResult;
use crate::display::{Display, OutputLine};

use super::permission_ask_line;

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
    ToolOutput {
        event: Value,
    },
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
            Preview::ToolOutput { event: nested } => {
                if let Some(info) = nested_model_recovery(&nested) {
                    self.recovery(info)?;
                } else if self.stream_json && nested.get("permission").is_none() {
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
        if self.stream_json {
            if let Some(result) = tool_record(entry) {
                write_json(&result)?;
            }
        }
        Ok(())
    }

    pub(super) fn finish(
        &mut self,
        session_id: &str,
        agent: &AgentSnapshot,
        elapsed: std::time::Duration,
    ) -> CliResult {
        self.close_thinking();
        let event = match agent.status {
            AgentStatus::Idle => "completed",
            AgentStatus::Waiting => "waiting",
            AgentStatus::Stopped | AgentStatus::Interrupted => "interrupted",
            AgentStatus::Running => "running",
        };
        if self.json || self.stream_json {
            if let Some(request) = &agent.permission {
                if self.stream_json {
                    write_json(&permission_ask_line(request))?;
                }
            }
            write_json(&json!({
                "type": "session", "event": event, "session_id": session_id,
                "agent": agent.address, "status": agent.status, "turns": agent.turn,
                "response": agent.last_output, "error": agent.last_error,
                "config": agent.config, "usage": agent.usage,
                "permission": agent.permission, "question": agent.question, "duration_ms": elapsed.as_millis(),
            }))?;
        } else {
            if let Some(output) = &agent.last_output {
                if let Some(suffix) = output.strip_prefix(&self.rendered_text) {
                    print!("{suffix}");
                } else if !self.rendered_text.ends_with(output) {
                    println!("\n{output}");
                }
            }
            println!();
            eprintln!("session {session_id} agent {}: {event}", agent.address);
            if let Some(request) = &agent.permission {
                eprintln!("pending authorization {}: {}", request.id, request.summary);
            }
            if let Some(request) = &agent.question {
                eprintln!("pending question {}: {}", request.id, request.text);
            }
            if self.verbose {
                eprintln!(
                    "{}",
                    serde_json::to_string_pretty(agent).unwrap_or_default()
                );
            }
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
        | HistoryKind::Other => return None,
    };
    Some(record.with_conversation(Some(entry.agent.clone())))
}

pub(super) fn nested_model_recovery(event: &Value) -> Option<ModelRecoveryInfo> {
    serde_json::from_value(event.get("model_recovery")?.clone()).ok()
}
