//! Terminal output rendering for kuku.
//!
//! Two rendering backends from the same structured event:
//! - text (concise/verbose) — human-readable terminal output
//! - JSON — structured, machine-consumable
//!
//! Style constants live at the top of this file so they can be
//! adjusted in one place.

use serde::Serialize;

// ── Style constants (one place to tune) ──

const THINKING_OPEN: &str = "\u{250c}\u{2500}\u{2500} thinking";
const THINKING_SEP: &str = "\u{2500}";
const THINKING_CLOSE: &str = "\u{2514}\u{2500}\u{2500} thinking";

const CODE_OPEN: &str = "\u{250c}\u{2500} code";
const CODE_LINE: &str = "\u{2502} ";
const CODE_CLOSE: &str = "\u{2514}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}";

const TABLE_OPEN: &str = "\u{250c}\u{2500} table";
const TABLE_CLOSE: &str = "\u{2514}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}";

const TOOL_PREFIX: &str = "\u{2699}";
const RESULT_PREFIX: &str = "  \u{21b3}";

const PERM_ASK_PREFIX: &str = "?";
const PERM_ALLOW_PREFIX: &str = "\u{2713}";
const PERM_DENY_PREFIX: &str = "\u{2717}";

const ERROR_PREFIX: &str = "!!";

const SESSION_PREFIX: &str = "--";

// ── Render mode ──

#[derive(Clone, Copy, PartialEq)]
pub enum Verbosity {
    Concise,
    Verbose,
}

pub struct Display {
    verbosity: Verbosity,
}

impl Display {
    pub fn new(verbosity: Verbosity) -> Self {
        Self { verbosity }
    }

    pub fn is_verbose(&self) -> bool {
        self.verbosity == Verbosity::Verbose
    }
}

// ── Thinking ──

impl Display {
    pub fn thinking_start(&self, tokens: u64) -> String {
        format!("{} ({}) {}", THINKING_OPEN, fmt_tokens(tokens), THINKING_SEP)
    }

    pub fn thinking_text(&self, text: &str) -> Option<String> {
        if self.is_verbose() {
            Some(text.to_string())
        } else {
            None
        }
    }

    pub fn thinking_end(&self, tokens: u64) -> String {
        format!(
            "{} \u{b7} {} tokens {}",
            THINKING_CLOSE,
            fmt_tokens(tokens),
            THINKING_SEP
        )
    }
}

fn fmt_tokens(n: u64) -> String {
    if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

// ── Tool call / result ──

impl Display {
    pub fn tool_call(&self, tool: &str, summary: &str, tool_call_id: &str) -> String {
        if self.is_verbose() {
            format!("{} {}  id={}", TOOL_PREFIX, tool, tool_call_id)
        } else {
            format!("{} {} {}", TOOL_PREFIX, tool, summary)
        }
    }

    pub fn tool_result(&self, status: &str, summary: &str, tool_call_id: &str) -> String {
        if self.is_verbose() {
            format!(
                "{} {} \u{b7} {}  id={}",
                RESULT_PREFIX, status, summary, tool_call_id
            )
        } else {
            format!("{} {} \u{b7} {}", RESULT_PREFIX, status, summary)
        }
    }

    pub fn tool_result_output(&self, output: &str) -> Option<String> {
        if self.is_verbose() {
            Some(output.to_string())
        } else {
            None
        }
    }
}

// ── Permission ──

impl Display {
    pub fn permission_ask(&self, tool: &str, summary: &str) -> String {
        format!("{} {} \u{b7} {}  (y/n)?", PERM_ASK_PREFIX, tool, summary)
    }

    pub fn permission_decision(&self, decision: &str, tool: &str, rule: &str) -> String {
        let prefix = if decision == "allow" {
            PERM_ALLOW_PREFIX
        } else {
            PERM_DENY_PREFIX
        };
        format!("{} {} \u{b7} {} \u{b7} {}", prefix, decision, tool, rule)
    }
}

// ── Error, session, code block, table ──

impl Display {
    pub fn error(&self, source: &str, kind: &str, message: &str) -> String {
        format!(
            "{} {} \u{b7} {} \u{b7} {}",
            ERROR_PREFIX, source, kind, message
        )
    }

    pub fn error_detail(&self, detail: &str) -> Option<String> {
        if self.is_verbose() {
            Some(detail.to_string())
        } else {
            None
        }
    }

    pub fn session_start(&self, session_id: &str, model: &str, effort: &str) -> String {
        format!(
            "{} session: {} \u{b7} {} \u{b7} {} {}",
            SESSION_PREFIX, session_id, model, effort, SESSION_PREFIX
        )
    }

    pub fn session_completed(
        &self,
        session_id: &str,
        turns: u64,
        tokens: u64,
        duration_ms: u64,
    ) -> String {
        format!(
            "{} completed: {} \u{b7} {} turns \u{b7} {} tokens \u{b7} {}s {}",
            SESSION_PREFIX,
            session_id,
            turns,
            fmt_tokens(tokens),
            duration_ms / 1000,
            SESSION_PREFIX
        )
    }

    pub fn session_interrupted(&self, session_id: &str, turns: u64) -> String {
        format!(
            "{} interrupted: {} \u{b7} {} turns {}",
            SESSION_PREFIX, session_id, turns, SESSION_PREFIX
        )
    }

    pub fn code_block_open(&self, language: Option<&str>) -> String {
        match language {
            Some(lang) => format!("{} {}", CODE_OPEN, lang),
            None => CODE_OPEN.to_string(),
        }
    }

    pub fn code_line(&self, line: &str) -> String {
        format!("{}{}", CODE_LINE, line)
    }

    pub fn code_block_close(&self) -> String {
        CODE_CLOSE.to_string()
    }

    pub fn table_open(&self) -> String {
        TABLE_OPEN.to_string()
    }

    pub fn table_row(&self, cells: &[&str], widths: &[usize]) -> String {
        let padded: Vec<String> = cells
            .iter()
            .zip(widths.iter())
            .map(|(c, w)| format!(" {:<w$} ", c, w = w))
            .collect();
        format!("{}{}", CODE_LINE, padded.join("\u{2502}"))
    }

    pub fn table_separator(&self, widths: &[usize]) -> String {
        let parts: Vec<String> = widths
            .iter()
            .map(|w| format!("{:\u{2500}>w$}", "", w = w + 2))
            .collect();
        format!("{}\u{2502}{}\u{2502}", CODE_LINE, parts.join("\u{253c}"))
    }

    pub fn table_close(&self) -> String {
        TABLE_CLOSE.to_string()
    }
}

// ── Event rendering (ported from old view/) ──

use kuku::event::{EventPayload, StoredEvent};

pub fn render_event_brief(event: &StoredEvent, verbose: bool) -> String {
    let mut line = format!("evt:{} | {}", event.id, event_type_name(&event.payload));
    let details = event_details(&event.payload, verbose);
    if !details.is_empty() {
        line.push_str(" | ");
        line.push_str(&details);
    }
    line
}

fn event_type_name(payload: &EventPayload) -> &'static str {
    match payload {
        EventPayload::SessionMeta { .. } => "session.meta",
        EventPayload::TurnStart { .. } => "turn.start",
        EventPayload::UserInput { .. } => "user.input",
        EventPayload::ModelRequest { .. } => "model.request",
        EventPayload::ModelResponse { .. } => "model.response",
        EventPayload::ModelError { .. } => "model.error",
        EventPayload::ToolCall { .. } => "tool.call",
        EventPayload::PermissionRequest { .. } => "permission.request",
        EventPayload::PermissionDecision { .. } => "permission.decision",
        EventPayload::ToolResult { .. } => "tool.result",
        EventPayload::PolicyLoaded { .. } => "policy.loaded",
        EventPayload::TurnEnd { .. } => "turn.end",
    }
}

fn event_details(payload: &EventPayload, verbose: bool) -> String {
    match payload {
        EventPayload::UserInput { text, .. } => text.chars().take(60).collect(),
        EventPayload::ModelResponse {
            text,
            stop_reason,
            ..
        } => {
            let preview: String = text.chars().take(60).collect();
            format!("{preview}  stop={stop_reason}")
        }
        EventPayload::ToolCall {
            tool,
            args,
            tool_call_id,
            ..
        } => {
            let path_or_cmd = args
                .get("path")
                .or_else(|| args.get("command"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if verbose {
                format!("{tool} {path_or_cmd}  id={tool_call_id}")
            } else {
                format!("{tool} {path_or_cmd}")
            }
        }
        EventPayload::ToolResult {
            tool_call_id,
            status,
            summary,
            ..
        } => {
            if verbose {
                format!("{status}  {summary}  id={tool_call_id}")
            } else {
                format!("{status}  {summary}")
            }
        }
        EventPayload::PermissionDecision {
            decision, rule, ..
        } => {
            format!("{decision}  {rule}")
        }
        _ => String::new(),
    }
}

pub fn derive_final_output(events: &[StoredEvent]) -> Option<String> {
    events.iter().rev().find_map(|event| match &event.payload {
        EventPayload::ModelResponse {
            stop_reason, text, ..
        } if stop_reason == "end_turn" => Some(text.clone()),
        _ => None,
    })
}

// ── JSON output types (stable schema) ──

#[derive(Serialize)]
#[serde(tag = "type")]
pub enum OutputLine {
    #[serde(rename = "thinking")]
    Thinking {
        tokens: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "code_block")]
    CodeBlock {
        #[serde(skip_serializing_if = "Option::is_none")]
        language: Option<String>,
        content: String,
    },
    #[serde(rename = "table")]
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        align: Option<Vec<String>>,
    },
    #[serde(rename = "tool_call")]
    ToolCall {
        tool: String,
        tool_call_id: String,
        summary: String,
        args: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_call_id: String,
        status: String,
        summary: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        output: Option<String>,
        truncated: bool,
    },
    #[serde(rename = "permission_ask")]
    PermissionAsk {
        request_id: String,
        tool: String,
        risk: String,
        summary: String,
    },
    #[serde(rename = "permission_decision")]
    PermissionDecision {
        request_id: String,
        tool: String,
        decision: String,
        rule: String,
    },
    #[serde(rename = "error")]
    Error {
        source: String,
        kind: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    #[serde(rename = "session")]
    Session {
        session_id: String,
        event: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        turns: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tokens: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
    },
}

impl OutputLine {
    pub fn to_json_line(&self) -> String {
        let mut line = serde_json::to_string(self).unwrap_or_default();
        line.push('\n');
        line
    }

    pub fn thinking(tokens: u64, text: Option<String>) -> Self {
        OutputLine::Thinking { tokens, text }
    }

    pub fn text_delta(text: String) -> Self {
        OutputLine::TextDelta { text }
    }

    pub fn code_block(language: Option<String>, content: String) -> Self {
        OutputLine::CodeBlock { language, content }
    }

    pub fn table(headers: Vec<String>, rows: Vec<Vec<String>>, align: Option<Vec<String>>) -> Self {
        OutputLine::Table {
            headers,
            rows,
            align,
        }
    }

    pub fn tool_call(
        tool: String,
        tool_call_id: String,
        summary: String,
        args: serde_json::Value,
    ) -> Self {
        OutputLine::ToolCall {
            tool,
            tool_call_id,
            summary,
            args,
        }
    }

    pub fn tool_result(
        tool_call_id: String,
        status: String,
        summary: String,
        output: Option<String>,
        truncated: bool,
    ) -> Self {
        OutputLine::ToolResult {
            tool_call_id,
            status,
            summary,
            output,
            truncated,
        }
    }

    pub fn permission_ask(request_id: String, tool: String, risk: String, summary: String) -> Self {
        OutputLine::PermissionAsk {
            request_id,
            tool,
            risk,
            summary,
        }
    }

    pub fn permission_decision(
        request_id: String,
        tool: String,
        decision: String,
        rule: String,
    ) -> Self {
        OutputLine::PermissionDecision {
            request_id,
            tool,
            decision,
            rule,
        }
    }

    pub fn error(source: String, kind: String, message: String, detail: Option<String>) -> Self {
        OutputLine::Error {
            source,
            kind,
            message,
            detail,
        }
    }

    pub fn session_started(session_id: String, model: String, effort: String) -> Self {
        OutputLine::Session {
            session_id,
            event: "started".into(),
            model: Some(model),
            effort: Some(effort),
            turns: None,
            tokens: None,
            duration_ms: None,
        }
    }

    pub fn session_completed(
        session_id: String,
        turns: u64,
        tokens: u64,
        duration_ms: u64,
    ) -> Self {
        OutputLine::Session {
            session_id,
            event: "completed".into(),
            model: None,
            effort: None,
            turns: Some(turns),
            tokens: Some(tokens),
            duration_ms: Some(duration_ms),
        }
    }

    pub fn session_interrupted(session_id: String, turns: u64) -> Self {
        OutputLine::Session {
            session_id,
            event: "interrupted".into(),
            model: None,
            effort: None,
            turns: Some(turns),
            tokens: None,
            duration_ms: None,
        }
    }
}
