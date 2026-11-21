use std::borrow::Cow;
use std::sync::LazyLock;

use pulldown_cmark::CodeBlockKind;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxSet};

use crate::text::single_line;
use crate::theme::{
    code_surface, CardFrame, ACCENT, CODE_COMMENT, CODE_KEYWORD, CODE_LITERAL, CODE_NAME,
    CODE_STRING, CODE_TEXT, ERROR,
};

use super::{append, document, wrap_bounded};

const MAX_SYNTAX_BYTES: usize = 8 * 1024;
const MAX_SYNTAX_LINES: usize = 128;
const MAX_SYNTAX_LINE_BYTES: usize = 1024;

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
        let frame = CardFrame::new(width);
        let limit = limit.saturating_add(1);
        let mut output = match self.language.as_deref() {
            Some(language) => vec![frame.title(language, ACCENT)],
            None if self.text.is_empty() => return Vec::new(),
            None if frame.bordered() => vec![frame.border('╭', '╮')],
            None => Vec::new(),
        };
        if output.len() == limit {
            return output;
        }
        output.extend(frame.top_padding_line().filter(|_| !self.text.is_empty()));
        let mut highlighter = self
            .language
            .as_deref()
            .filter(|_| !self.text.is_empty())
            .and_then(Highlighter::new);

        for line in self.text.split_inclusive('\n') {
            let remaining = limit - output.len();
            if remaining == 0 {
                return output;
            }
            let line = source_line(&mut highlighter, budget, line);
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
        if frame.bordered() && output.len() < limit {
            output.push(frame.border('╰', '╯'));
        }
        output
    }
}

fn source_line(
    highlighter: &mut Option<Highlighter>,
    budget: &mut SyntaxBudget,
    line: &str,
) -> Line<'static> {
    if let Some(parser) = highlighter {
        if budget.take(line.len()) {
            if let Some(spans) = parser.line(line) {
                return Line::from(spans);
            }
        }
        *highlighter = None;
    }
    Line::styled(
        line.strip_suffix('\n').unwrap_or(line).to_owned(),
        code_surface(),
    )
}

pub(crate) fn file_source(text: &str, language: &str) -> Vec<Line<'static>> {
    let clean = crate::text::replace_tabs(&crate::text::terminal_text(text));
    let mut highlighter = Highlighter::new(language);
    let mut budget = SyntaxBudget::default();
    clean
        .split_inclusive('\n')
        .map(|line| source_line(&mut highlighter, &mut budget, line))
        .collect()
}

#[derive(Debug, Clone, Copy)]
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
