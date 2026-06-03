use serde::Serialize;

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
    SessionStarted {
        session_id: String,
        event: String,
        tier: String,
        model: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        previous_input_tokens: Option<u64>,
    },
    #[serde(rename = "session")]
    SessionCompleted {
        session_id: String,
        event: String,
        tier: String,
        model: String,
        turns: u64,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_input_tokens: u64,
        cache_creation_input_tokens: u64,
        duration_ms: u64,
        response: String,
        usage: RunUsageSummary,
        tools: kuku::query::ToolSummary,
    },
    #[serde(rename = "session")]
    SessionInterrupted {
        session_id: String,
        event: String,
        tier: String,
        model: String,
        turns: u64,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_input_tokens: u64,
        cache_creation_input_tokens: u64,
        duration_ms: u64,
        response: Option<String>,
        usage: RunUsageSummary,
        tools: kuku::query::ToolSummary,
    },
}

impl OutputLine {
    pub fn to_json_line(&self) -> String {
        let mut line = serde_json::to_string(self).unwrap_or_default();
        line.push('\n');
        line
    }

    pub fn thinking(duration_ms: u64, text: Option<String>) -> Self {
        OutputLine::Thinking { duration_ms, text }
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

    pub fn session_started(
        session_id: String,
        tier: String,
        model: String,
        previous_input_tokens: Option<u64>,
    ) -> Self {
        OutputLine::SessionStarted {
            session_id,
            event: "started".into(),
            tier,
            model,
            previous_input_tokens,
        }
    }

    pub fn session_completed(summary: RunMetrics) -> Self {
        OutputLine::SessionCompleted {
            session_id: summary.session_id,
            event: "completed".into(),
            tier: summary.tier,
            model: summary.model,
            turns: summary.turns,
            input_tokens: summary.input_tokens,
            output_tokens: summary.output_tokens,
            cache_read_input_tokens: summary.cache_read_input_tokens,
            cache_creation_input_tokens: summary.cache_creation_input_tokens,
            duration_ms: summary.duration_ms,
            response: summary.response.unwrap_or_default(),
            usage: summary.usage,
            tools: summary.tools,
        }
    }

    pub fn session_interrupted(summary: RunMetrics) -> Self {
        OutputLine::SessionInterrupted {
            session_id: summary.session_id,
            event: "interrupted".into(),
            tier: summary.tier,
            model: summary.model,
            turns: summary.turns,
            input_tokens: summary.input_tokens,
            output_tokens: summary.output_tokens,
            cache_read_input_tokens: summary.cache_read_input_tokens,
            cache_creation_input_tokens: summary.cache_creation_input_tokens,
            duration_ms: summary.duration_ms,
            response: summary.response,
            usage: summary.usage,
            tools: summary.tools,
        }
    }
}

pub struct RunMetrics {
    pub session_id: String,
    pub tier: String,
    pub model: String,
    pub turns: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub duration_ms: u64,
    pub response: Option<String>,
    pub usage: RunUsageSummary,
    pub tools: kuku::query::ToolSummary,
}

#[derive(serde::Serialize)]
pub struct RunUsageSummary {
    pub total_input_tokens: u64,
    pub total_tokens: u64,
    pub cache_hit_rate: f64,
    pub model_requests: u64,
    pub thinking_duration_ms: u64,
}
