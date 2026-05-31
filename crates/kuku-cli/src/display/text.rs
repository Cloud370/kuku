use std::time::Duration;

const THINKING_OPEN: &str = "\u{250c}\u{2500} thinking";
const THINKING_SEP: &str = "\u{2500}";
const THINKING_CLOSE: &str = "\u{2514}\u{2500} thinking";

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    Pretty,
    Raw,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Display {
    mode: RenderMode,
    think_level: &'static str,
    show_thinking: bool,
    line_start: bool,
}

impl Display {
    pub fn new(show_thinking: bool, think_level: &'static str) -> Self {
        Self {
            mode: RenderMode::Pretty,
            think_level,
            show_thinking,
            line_start: true,
        }
    }

    pub fn new_raw(show_thinking: bool, think_level: &'static str) -> Self {
        Self {
            mode: RenderMode::Raw,
            think_level,
            show_thinking,
            line_start: true,
        }
    }

    pub fn thinking_start(&self) -> String {
        match self.mode {
            RenderMode::Pretty => {
                format!("{} [{}] {}", THINKING_OPEN, self.think_level, THINKING_SEP)
            }
            RenderMode::Raw => format!("thinking [{}]", self.think_level),
        }
    }

    pub fn thinking_text(&self, text: &str) -> Option<String> {
        if self.show_thinking {
            Some(text.to_string())
        } else {
            None
        }
    }

    pub fn thinking_line(&mut self, text: &str) -> Option<String> {
        if !self.show_thinking {
            return None;
        }
        let prefix = match self.mode {
            RenderMode::Pretty => "\u{2502} ",
            RenderMode::Raw => "thinking ",
        };
        let mut output = String::new();
        for ch in text.chars() {
            if self.line_start {
                output.push_str(prefix);
                self.line_start = false;
            }
            output.push(ch);
            if ch == '\n' {
                self.line_start = true;
            }
        }
        Some(output)
    }

    pub fn thinking_end(&mut self, duration: Duration) -> String {
        let secs = duration.as_secs_f64();
        let result = match self.mode {
            RenderMode::Pretty => {
                format!("\n{} \u{b7} {:.1}s {}", THINKING_CLOSE, secs, THINKING_SEP)
            }
            RenderMode::Raw => {
                format!("\nthinking end \u{b7} {:.1}s", secs)
            }
        };
        self.line_start = true;
        result
    }

    pub fn tool_call(&self, tool: &str, summary: &str, _tool_call_id: &str) -> String {
        match self.mode {
            RenderMode::Pretty => {
                if tool == "agent" {
                    format!("{} agent \u{b7} {}", TOOL_PREFIX, summary)
                } else {
                    format!("{} {} \u{b7} {}", TOOL_PREFIX, tool, summary)
                }
            }
            RenderMode::Raw => format!("call {} \u{b7} {}", tool, summary),
        }
    }

    pub fn tool_running(&self) -> String {
        match self.mode {
            RenderMode::Pretty => "  ...".to_string(),
            RenderMode::Raw => "run ...".to_string(),
        }
    }

    pub fn tool_result(&self, status: &str, summary: &str, _tool_call_id: &str) -> String {
        match self.mode {
            RenderMode::Pretty => format!("{} {} \u{b7} {}", RESULT_PREFIX, status, summary),
            RenderMode::Raw => format!("result {} \u{b7} {}", status, summary),
        }
    }

    pub fn permission_ask(&self, tool: &str, summary: &str) -> String {
        match self.mode {
            RenderMode::Pretty => {
                format!("{} {} \u{b7} {}  (Y/n)?", PERM_ASK_PREFIX, tool, summary)
            }
            RenderMode::Raw => format!("ask {} \u{b7} {}", tool, summary),
        }
    }

    pub fn permission_decision(&self, decision: &str, tool: &str, rule: &str) -> String {
        match self.mode {
            RenderMode::Pretty => {
                let prefix = if decision == "allow" {
                    PERM_ALLOW_PREFIX
                } else {
                    PERM_DENY_PREFIX
                };
                format!("{} {} \u{b7} {} \u{b7} {}", prefix, decision, tool, rule)
            }
            RenderMode::Raw => format!("{} {} \u{b7} {}", decision, tool, rule),
        }
    }

    pub fn error(&self, source: &str, kind: &str, message: &str) -> String {
        match self.mode {
            RenderMode::Pretty => format!(
                "{} {} \u{b7} {} \u{b7} {}",
                ERROR_PREFIX, source, kind, message
            ),
            RenderMode::Raw => format!("error {} \u{b7} {} \u{b7} {}", source, kind, message),
        }
    }

    pub fn session_start(&self, session_id: &str, tier: &str, model: &str) -> String {
        format!(
            "{} session: {} \u{b7} {} \u{b7} {} {}",
            SESSION_PREFIX, session_id, tier, model, SESSION_PREFIX
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn session_completed(
        &self,
        session_id: &str,
        turns: u64,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_input_tokens: u64,
        cache_creation_input_tokens: u64,
        duration: Duration,
    ) -> String {
        let secs = duration.as_secs();
        let mut cache_parts = Vec::new();
        if cache_read_input_tokens > 0 {
            cache_parts.push(format!("read {}", fmt_tokens(cache_read_input_tokens)));
        }
        if cache_creation_input_tokens > 0 {
            cache_parts.push(format!("write {}", fmt_tokens(cache_creation_input_tokens)));
        }
        let cache_part = if cache_parts.is_empty() {
            String::new()
        } else {
            format!(" \u{b7} cache {}", cache_parts.join(" / "))
        };
        format!(
            "{} completed: {} \u{b7} {} turns \u{b7} in {} \u{b7} out {}{} \u{b7} {}s {}",
            SESSION_PREFIX,
            session_id,
            turns,
            fmt_tokens(input_tokens),
            fmt_tokens(output_tokens),
            cache_part,
            secs,
            SESSION_PREFIX
        )
    }

    pub fn session_interrupted(&self, session_id: &str, turns: u64) -> String {
        format!(
            "{} interrupted: {} \u{b7} {} turns {}",
            SESSION_PREFIX, session_id, turns, SESSION_PREFIX
        )
    }

    pub fn context_previous(&self, tokens: u64) -> String {
        format!(
            "{} context: {} tokens (previous) {}",
            SESSION_PREFIX,
            fmt_tokens(tokens),
            SESSION_PREFIX
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

pub(crate) fn fmt_tokens(n: u64) -> String {
    if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}
