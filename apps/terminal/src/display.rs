//! Terminal output rendering for kuku.
//!
//! Two rendering backends from the same structured event:
//! - text — human-readable terminal output
//! - JSON — structured, machine-consumable
//!
//! Style constants live at the top of this file so they can be
//! adjusted in one place.

use std::time::Duration;

use kuku::event::{EventPayload, StoredEvent};
use serde::Serialize;

// ── Style constants (one place to tune) ──

const THINKING_OPEN: &str = "\u{250c}\u{2500}\u{2500} thinking";
const THINKING_SEP: &str = "\u{2500}";
const THINKING_CLOSE: &str = "\u{2514}\u{2500}\u{2500} thinking";

const CODE_OPEN: &str = "\u{250c}\u{2500} code";
const CODE_LINE: &str = "\u{2502} ";
const CODE_CLOSE: &str = "\u{2514}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}";

const TABLE_OPEN: &str = "\u{250c}\u{2500} table";
const TABLE_CLOSE: &str =
    "\u{2514}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}";

const TOOL_PREFIX: &str = "\u{2699}";
const RESULT_PREFIX: &str = "  \u{21b3}";

const PERM_ASK_PREFIX: &str = "?";
const PERM_ALLOW_PREFIX: &str = "\u{2713}";
const PERM_DENY_PREFIX: &str = "\u{2717}";

const ERROR_PREFIX: &str = "!!";

const SESSION_PREFIX: &str = "--";

// ── Display ──

/// Controls whether thinking content is rendered.
#[derive(Clone, Copy, PartialEq)]
pub struct Display {
    show_thinking: bool,
}

impl Display {
    /// Create a new display.
    pub fn new(show_thinking: bool) -> Self {
        Self { show_thinking }
    }

    /// Render a thinking block start line.
    pub fn thinking_start(&self) -> String {
        format!("{} {}", THINKING_OPEN, THINKING_SEP)
    }

    /// Return thinking text only when show_thinking is enabled.
    pub fn thinking_text(&self, text: &str) -> Option<String> {
        if self.show_thinking {
            Some(text.to_string())
        } else {
            None
        }
    }

    /// Render a thinking block close line with duration.
    pub fn thinking_end(&self, duration: Duration) -> String {
        let secs = duration.as_secs_f64();
        format!("{} \u{b7} {:.1}s {}", THINKING_CLOSE, secs, THINKING_SEP)
    }

    /// Render a tool call line.
    pub fn tool_call(&self, tool: &str, summary: &str, _tool_call_id: &str) -> String {
        format!("{} {} {}", TOOL_PREFIX, tool, summary)
    }

    /// Render a tool result line.
    pub fn tool_result(&self, status: &str, summary: &str, _tool_call_id: &str) -> String {
        format!("{} {} \u{b7} {}", RESULT_PREFIX, status, summary)
    }

    /// Render a permission ask prompt.
    pub fn permission_ask(&self, tool: &str, summary: &str) -> String {
        format!("{} {} \u{b7} {}  (y/n)?", PERM_ASK_PREFIX, tool, summary)
    }

    /// Render a permission decision line.
    pub fn permission_decision(&self, decision: &str, tool: &str, rule: &str) -> String {
        let prefix = if decision == "allow" {
            PERM_ALLOW_PREFIX
        } else {
            PERM_DENY_PREFIX
        };
        format!("{} {} \u{b7} {} \u{b7} {}", prefix, decision, tool, rule)
    }

    /// Render an error line.
    pub fn error(&self, source: &str, kind: &str, message: &str) -> String {
        format!(
            "{} {} \u{b7} {} \u{b7} {}",
            ERROR_PREFIX, source, kind, message
        )
    }

    /// Render a session start line with tier and model.
    pub fn session_start(&self, session_id: &str, tier: &str, model: &str) -> String {
        format!(
            "{} session: {} \u{b7} {} \u{b7} {} {}",
            SESSION_PREFIX, session_id, tier, model, SESSION_PREFIX
        )
    }

    /// Render a session completed line with separate in/out tokens.
    pub fn session_completed(
        &self,
        session_id: &str,
        turns: u64,
        input_tokens: u64,
        output_tokens: u64,
        duration: Duration,
    ) -> String {
        let secs = duration.as_secs();
        format!(
            "{} completed: {} \u{b7} {} turns \u{b7} in {} \u{b7} out {} \u{b7} {}s {}",
            SESSION_PREFIX,
            session_id,
            turns,
            fmt_tokens(input_tokens),
            fmt_tokens(output_tokens),
            secs,
            SESSION_PREFIX
        )
    }

    /// Render a session interrupted line.
    pub fn session_interrupted(&self, session_id: &str, turns: u64) -> String {
        format!(
            "{} interrupted: {} \u{b7} {} turns {}",
            SESSION_PREFIX, session_id, turns, SESSION_PREFIX
        )
    }

    /// Render a context continuation line.
    pub fn context_previous(&self, tokens: u64) -> String {
        format!(
            "{} context: {} tokens (previous) {}",
            SESSION_PREFIX,
            fmt_tokens(tokens),
            SESSION_PREFIX
        )
    }

    /// Render a code block opening line.
    pub fn code_block_open(&self, language: Option<&str>) -> String {
        match language {
            Some(lang) => format!("{} {}", CODE_OPEN, lang),
            None => CODE_OPEN.to_string(),
        }
    }

    /// Render a single code line with prefix.
    pub fn code_line(&self, line: &str) -> String {
        format!("{}{}", CODE_LINE, line)
    }

    /// Render a code block closing line.
    pub fn code_block_close(&self) -> String {
        CODE_CLOSE.to_string()
    }

    /// Render a table opening line.
    pub fn table_open(&self) -> String {
        TABLE_OPEN.to_string()
    }

    /// Render a table row with padded cells.
    pub fn table_row(&self, cells: &[&str], widths: &[usize]) -> String {
        let padded: Vec<String> = cells
            .iter()
            .zip(widths.iter())
            .map(|(c, w)| format!(" {:<w$} ", c, w = w))
            .collect();
        format!("{}{}", CODE_LINE, padded.join("\u{2502}"))
    }

    /// Render a table separator line.
    pub fn table_separator(&self, widths: &[usize]) -> String {
        let parts: Vec<String> = widths
            .iter()
            .map(|w| format!("{:\u{2500}>w$}", "", w = w + 2))
            .collect();
        format!("{}\u{2502}{}\u{2502}", CODE_LINE, parts.join("\u{253c}"))
    }

    /// Render a table closing line.
    pub fn table_close(&self) -> String {
        TABLE_CLOSE.to_string()
    }
}

fn fmt_tokens(n: u64) -> String {
    if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

// ── Event rendering (ported from old view/) ──

/// Format a stored event as a single summary line.
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
            text, stop_reason, ..
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
        EventPayload::PermissionDecision { decision, rule, .. } => {
            format!("{decision}  {rule}")
        }
        _ => String::new(),
    }
}

/// Extract the final assistant response from completed session events.
pub fn derive_final_output(events: &[StoredEvent]) -> Option<String> {
    events.iter().rev().find_map(|event| match &event.payload {
        EventPayload::ModelResponse {
            stop_reason, text, ..
        } if stop_reason == "end_turn" => Some(text.clone()),
        _ => None,
    })
}

// ── JSON output types (stable schema) ──

/// Structured JSON output line matching the display-spec schema.
#[derive(Serialize)]
#[serde(tag = "type")]
pub enum OutputLine {
    #[serde(rename = "thinking")]
    Thinking {
        duration_ms: u64,
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
        tier: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        turns: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        input_tokens: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        output_tokens: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        previous_input_tokens: Option<u64>,
    },
}

impl OutputLine {
    /// Serialize to a single JSON line with trailing newline.
    pub fn to_json_line(&self) -> String {
        let mut line = serde_json::to_string(self).unwrap_or_default();
        line.push('\n');
        line
    }

    /// Create a thinking output line.
    pub fn thinking(duration_ms: u64, text: Option<String>) -> Self {
        OutputLine::Thinking { duration_ms, text }
    }

    /// Create a text delta output line.
    pub fn text_delta(text: String) -> Self {
        OutputLine::TextDelta { text }
    }

    /// Create a code block output line.
    pub fn code_block(language: Option<String>, content: String) -> Self {
        OutputLine::CodeBlock { language, content }
    }

    /// Create a table output line.
    pub fn table(headers: Vec<String>, rows: Vec<Vec<String>>, align: Option<Vec<String>>) -> Self {
        OutputLine::Table {
            headers,
            rows,
            align,
        }
    }

    /// Create a tool call output line.
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

    /// Create a tool result output line.
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

    /// Create a permission ask output line.
    pub fn permission_ask(request_id: String, tool: String, risk: String, summary: String) -> Self {
        OutputLine::PermissionAsk {
            request_id,
            tool,
            risk,
            summary,
        }
    }

    /// Create a permission decision output line.
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

    /// Create an error output line.
    pub fn error(source: String, kind: String, message: String, detail: Option<String>) -> Self {
        OutputLine::Error {
            source,
            kind,
            message,
            detail,
        }
    }

    /// Create a session started output line.
    pub fn session_started(
        session_id: String,
        tier: String,
        model: String,
        previous_input_tokens: Option<u64>,
    ) -> Self {
        OutputLine::Session {
            session_id,
            event: "started".into(),
            tier: Some(tier),
            model: Some(model),
            turns: None,
            input_tokens: None,
            output_tokens: None,
            duration_ms: None,
            previous_input_tokens,
        }
    }

    /// Create a session completed output line.
    pub fn session_completed(
        session_id: String,
        tier: String,
        model: String,
        turns: u64,
        input_tokens: u64,
        output_tokens: u64,
        duration_ms: u64,
    ) -> Self {
        OutputLine::Session {
            session_id,
            event: "completed".into(),
            tier: Some(tier),
            model: Some(model),
            turns: Some(turns),
            input_tokens: Some(input_tokens),
            output_tokens: Some(output_tokens),
            duration_ms: Some(duration_ms),
            previous_input_tokens: None,
        }
    }

    /// Create a session interrupted output line.
    pub fn session_interrupted(session_id: String, turns: u64) -> Self {
        OutputLine::Session {
            session_id,
            event: "interrupted".into(),
            tier: None,
            model: None,
            turns: Some(turns),
            input_tokens: None,
            output_tokens: None,
            duration_ms: None,
            previous_input_tokens: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_start_contains_tier_and_model() {
        let d = Display::new(false);
        let line = d.session_start("s_001", "strong", "claude-sonnet-4-6");
        assert!(line.contains("strong"), "should contain tier");
        assert!(line.contains("claude-sonnet-4-6"), "should contain model");
        assert!(line.contains("s_001"), "should contain session id");
    }

    #[test]
    fn session_completed_shows_in_out_tokens() {
        let d = Display::new(false);
        let line = d.session_completed("s_001", 2, 35000, 7000, Duration::from_secs(18));
        assert!(
            line.contains("in 35.0k"),
            "should show input tokens: {line}"
        );
        assert!(
            line.contains("out 7.0k"),
            "should show output tokens: {line}"
        );
        assert!(line.contains("2 turns"), "should show turn count");
        assert!(line.contains("18s"), "should show duration");
    }

    #[test]
    fn thinking_start_has_no_duration() {
        let d = Display::new(false);
        let line = d.thinking_start();
        assert!(line.contains("thinking"), "should contain thinking label");
        assert!(!line.contains("s"), "should not contain duration");
    }

    #[test]
    fn thinking_end_shows_duration() {
        let d = Display::new(false);
        let line = d.thinking_end(Duration::from_millis(12500));
        assert!(line.contains("12.5s"), "should show duration");
    }

    #[test]
    fn thinking_text_shown_when_enabled() {
        let d = Display::new(true);
        assert_eq!(d.thinking_text("reasoning"), Some("reasoning".to_string()));
    }

    #[test]
    fn thinking_text_hidden_when_disabled() {
        let d = Display::new(false);
        assert_eq!(d.thinking_text("reasoning"), None);
    }

    #[test]
    fn context_previous_shows_tokens() {
        let d = Display::new(false);
        let line = d.context_previous(35000);
        assert!(line.contains("35.0k"), "should show token count: {line}");
        assert!(
            line.contains("previous"),
            "should indicate previous context"
        );
    }
}
