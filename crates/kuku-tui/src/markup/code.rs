use std::borrow::Cow;
use std::sync::LazyLock;

use pulldown_cmark::CodeBlockKind;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxSet};
use unicode_width::UnicodeWidthStr;

use crate::text::{shorten, single_line};
use crate::theme::{
    code_surface, ACCENT, CODE_COMMENT, CODE_KEYWORD, CODE_LITERAL, CODE_NAME, CODE_STRING,
    CODE_TEXT, ERROR, MUTED,
};

use super::{append, document, wrap_bounded};

const MAX_SYNTAX_BYTES: usize = 8 * 1024;
const MAX_SYNTAX_LINES: usize = 128;
const MAX_SYNTAX_LINE_BYTES: usize = 1024;
const MIN_FRAME_WIDTH: usize = 4;
const PADDED_FRAME_WIDTH: usize = 8;

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static SCOPES: LazyLock<Vec<(Scope, Color)>> = LazyLock::new(|| {
    [
        ("comment", CODE_COMMENT),
        ("string", CODE_STRING),
        ("keyword", CODE_KEYWORD),
        ("storage", CODE_KEYWORD),
        ("constant", CODE_LITERAL),
        ("entity.name", CODE_NAME),
        ("support", CODE_NAME),
        ("invalid", ERROR),
    ]
    .into_iter()
    .map(|(name, color)| (Scope::new(name).expect("valid built-in scope"), color))
    .collect()
});

pub(super) struct CodeBlock {
    language: Option<String>,
    text: String,
}

impl CodeBlock {
    pub(super) fn new(kind: CodeBlockKind<'_>) -> Self {
        let language = match kind {
            CodeBlockKind::Fenced(info) => info.split_whitespace().next().map(single_line),
            CodeBlockKind::Indented => None,
        };
        Self {
            language,
            text: String::new(),
        }
    }

    pub(super) fn push(&mut self, text: &str) {
        self.text.push_str(text);
    }

    pub(super) fn markdown(&self) -> Option<&str> {
        document::is_markdown(self.language.as_deref()?).then_some(self.text.as_str())
    }

    pub(super) fn render(
        self,
        width: usize,
        limit: usize,
        budget: &mut SyntaxBudget,
    ) -> Vec<Line<'static>> {
        let frame = Frame::new(width);
        let limit = limit.saturating_add(1);
        let mut output = vec![frame.label(self.language.as_deref().unwrap_or("Code"))];
        if limit == 1 {
            return output;
        }
        let mut highlighter = self.language.as_deref().and_then(Highlighter::new);

        for line in self.text.split_inclusive('\n') {
            let remaining = limit - output.len();
            if remaining == 0 {
                return output;
            }
            let spans = match &mut highlighter {
                Some(highlighter) if budget.take(line.len()) => highlighter.line(line),
                Some(_) | None => None,
            };
            let line = if let Some(spans) = spans {
                Line::from(spans)
            } else {
                highlighter = None;
                Line::styled(
                    line.strip_suffix('\n').unwrap_or(line).to_owned(),
                    code_surface(),
                )
            };
            output.extend(
                wrap_bounded(vec![line], frame.content_width(), remaining)
                    .into_iter()
                    .take(remaining)
                    .map(|line| frame.line(line)),
            );
        }
        if self.text.is_empty() && output.len() < limit {
            output.push(frame.line(Line::default()));
        }
        if frame.bordered && output.len() < limit {
            output.push(frame.bottom());
        }
        output
    }
}

pub(super) struct SyntaxBudget {
    bytes: usize,
    lines: usize,
}

impl Default for SyntaxBudget {
    fn default() -> Self {
        Self {
            bytes: MAX_SYNTAX_BYTES,
            lines: MAX_SYNTAX_LINES,
        }
    }
}

impl SyntaxBudget {
    fn take(&mut self, bytes: usize) -> bool {
        if bytes > self.bytes.min(MAX_SYNTAX_LINE_BYTES) || self.lines == 0 {
            return false;
        }
        self.bytes -= bytes;
        self.lines -= 1;
        true
    }
}

struct Highlighter {
    parse: ParseState,
    scopes: ScopeStack,
}

impl Highlighter {
    fn new(language: &str) -> Option<Self> {
        let normalized = language.to_ascii_lowercase();
        let token = match normalized.as_str() {
            "shell" => "sh",
            "text" | "plaintext" | "txt" => return None,
            _ => &normalized,
        };
        let syntax = SYNTAXES.find_syntax_by_token(token)?;
        Some(Self {
            parse: ParseState::new(syntax),
            scopes: ScopeStack::new(),
        })
    }

    fn line(&mut self, line: &str) -> Option<Vec<Span<'static>>> {
        let content = line.strip_suffix('\n').unwrap_or(line);
        let source = if content.len() == line.len() {
            Cow::Owned(format!("{line}\n"))
        } else {
            Cow::Borrowed(line)
        };
        let mut spans = Vec::new();
        let mut start = 0;
        for (end, op) in self.parse.parse_line(&source, &SYNTAXES).ok()? {
            let end = end.min(content.len());
            if start < end {
                append(&mut spans, content.get(start..end)?, self.style());
            }
            self.scopes.apply(&op).ok()?;
            start = end;
        }
        if start < content.len() {
            append(&mut spans, content.get(start..)?, self.style());
        }
        Some(spans)
    }

    fn style(&self) -> Style {
        let color = self.scopes.scopes.iter().rev().find_map(|&actual| {
            SCOPES
                .iter()
                .find_map(|&(prefix, color)| prefix.is_prefix_of(actual).then_some(color))
        });
        code_surface().fg(color.unwrap_or(CODE_TEXT))
    }
}

struct Frame {
    width: usize,
    bordered: bool,
    padding: usize,
}

impl Frame {
    fn new(width: usize) -> Self {
        Self {
            width,
            bordered: width >= MIN_FRAME_WIDTH,
            padding: usize::from(width >= PADDED_FRAME_WIDTH),
        }
    }

    fn content_width(&self) -> usize {
        self.width - 2 * (usize::from(self.bordered) + self.padding)
    }

    fn label(&self, label: &str) -> Line<'static> {
        let style = code_surface().fg(ACCENT).add_modifier(Modifier::BOLD);
        if !self.bordered {
            return Line::styled(shorten(label, self.width), style);
        }
        let (prefix, tail) = if self.padding > 0 {
            ("╭─ ", " ")
        } else {
            ("╭", "")
        };
        let room = self.width - prefix.width() - tail.width() - 1;
        let label = shorten(label, room);
        let fill = "─".repeat(room - label.width());
        Line::from(vec![
            Span::styled(prefix, edge()),
            Span::styled(label, style),
            Span::styled(format!("{tail}{fill}╮"), edge()),
        ])
    }

    fn line(&self, line: Line<'static>) -> Line<'static> {
        let room = self.content_width().saturating_sub(line.width());
        let mut spans = Vec::with_capacity(line.spans.len() + 4);
        if self.bordered {
            append(&mut spans, "│", edge());
        }
        if self.padding != 0 {
            append(&mut spans, " ", code_surface());
        }
        spans.extend(line.spans);
        if room + self.padding != 0 {
            append(&mut spans, &" ".repeat(room + self.padding), code_surface());
        }
        if self.bordered {
            append(&mut spans, "│", edge());
        }
        Line::from(spans)
    }

    fn bottom(&self) -> Line<'static> {
        Line::styled(format!("╰{}╯", "─".repeat(self.width - 2)), edge())
    }
}

fn edge() -> Style {
    code_surface().fg(MUTED)
}
