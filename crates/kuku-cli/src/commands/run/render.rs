use std::io::{self, IsTerminal, Write};
use std::time::Instant;

use kuku::{ModelRecoveryInfo, ToolKind};
use kuku_client::{AgentSnapshot, AgentStatus};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::cli_args::RunArgs;
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
        kind: ToolKind,
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
    tool_conversations: std::collections::HashMap<String, String>,
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
            tool_conversations: std::collections::HashMap::new(),
        }
    }

    pub(super) fn needs_preview(&self) -> bool {
        !self.json
    }

    pub(super) fn event(&mut self, event: Value) {
        if self.json {
            return;
        }
        let Ok(preview) = serde_json::from_value(event.clone()) else {
            return;
        };
        match preview {
            Preview::Text { content: text } => {
                self.close_thinking();
                self.rendered_text.push_str(&text);
                if self.stream_json {
                    println!("{}", OutputLine::text_delta(text).to_json_line());
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
            Preview::ToolStart {
                id,
                tool,
                summary,
                kind,
            } => {
                self.close_thinking();
                if let Some(conversation) = conversation_for_tool_kind(&kind) {
                    self.tool_conversations.insert(id.clone(), conversation);
                }
                if self.stream_json {
                    println!(
                        "{}",
                        tool_call_line(tool, id, summary, &kind).to_json_line()
                    );
                } else {
                    println!("\n{}", self.display.tool_call(&tool, &summary, &id));
                }
            }
            Preview::ToolEnd {
                id,
                status,
                summary,
            } => {
                if self.stream_json {
                    let conversation = self.tool_conversations.remove(&id);
                    println!(
                        "{}",
                        tool_result_line(id, status, summary, conversation).to_json_line()
                    );
                } else {
                    println!("{}", self.display.tool_result(&status, &summary, &id));
                }
            }
            Preview::ModelRecovery { info } => self.recovery(info),
            Preview::ToolOutput { event: nested } => {
                if let Some(info) = nested_model_recovery(&nested) {
                    self.recovery(info);
                } else if self.stream_json && nested.get("permission").is_none() {
                    println!("{event}");
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
                    println!("{event}");
                }
            }
        }
    }

    pub(super) fn finish(
        &mut self,
        session_id: &str,
        agent: &AgentSnapshot,
        elapsed: std::time::Duration,
    ) {
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
                    println!("{}", permission_ask_line(request).to_json_line());
                }
            }
            println!(
                "{}",
                json!({
                    "type": "session", "event": event, "session_id": session_id,
                    "agent": agent.address, "status": agent.status, "turns": agent.turn,
                    "response": agent.last_output, "error": agent.last_error,
                    "config": agent.config, "usage": agent.usage,
                    "permission": agent.permission, "question": agent.question, "duration_ms": elapsed.as_millis(),
                })
            );
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

    fn recovery(&mut self, info: kuku::query::ModelRecoveryInfo) {
        self.close_thinking();
        self.rendered_text.clear();
        if self.stream_json {
            println!("{}", OutputLine::model_recovery(info).to_json_line());
        } else {
            println!("{}", self.display.model_recovery(&info));
        }
    }
}

pub(super) fn conversation_for_tool_kind(kind: &ToolKind) -> Option<String> {
    match kind {
        ToolKind::Agent { conversation, .. } => Some(conversation.as_str().into()),
        _ => None,
    }
}

pub(super) fn tool_call_line(
    tool: String,
    id: String,
    summary: String,
    kind: &ToolKind,
) -> OutputLine {
    OutputLine::tool_call(tool, id, summary, Value::Null)
        .with_conversation(conversation_for_tool_kind(kind))
}

pub(super) fn tool_result_line(
    id: String,
    status: String,
    summary: String,
    conversation: Option<String>,
) -> OutputLine {
    OutputLine::tool_result(id, status, summary, None, false).with_conversation(conversation)
}

pub(super) fn nested_model_recovery(event: &Value) -> Option<ModelRecoveryInfo> {
    serde_json::from_value(event.get("model_recovery")?.clone()).ok()
}
