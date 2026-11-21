use pulldown_cmark::{Event, Parser};
use ratatui::text::Line;

use super::{clean_markdown, document, LinkLayout, Markdown, SyntaxBudget, OPTIONS};

#[derive(Debug, Default)]
pub(crate) struct MarkdownStream {
    stable_bytes: usize,
    lines: usize,
    syntax_budget: SyntaxBudget,
}

pub(crate) struct MarkdownTail {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) links: LinkLayout,
    pub(crate) stable_lines: usize,
}

impl MarkdownStream {
    pub(crate) fn render_tail(&mut self, text: &str, width: usize, limit: usize) -> MarkdownTail {
        let tail = &text[self.stable_bytes..];
        let clean = clean_markdown(tail);
        let source = document::repair_fences(&clean);
        let mut markdown = Markdown {
            width: width.max(1),
            limit: limit.saturating_sub(self.lines),
            preview_documents: true,
            link_layout: Some(LinkLayout::default()),
            syntax_budget: self.syntax_budget,
            ..Markdown::default()
        };
        let mut stable_lines = 0;
        if source != tail {
            markdown.parse(&source);
        } else if let Some(prefix) = parse_tail(&mut markdown, &source) {
            self.stable_bytes += prefix.stable_bytes;
            self.lines += prefix.lines;
            self.syntax_budget = prefix.syntax_budget;
            stable_lines = prefix.lines;
        }
        MarkdownTail {
            lines: markdown.lines,
            links: markdown.link_layout.unwrap_or_default(),
            stable_lines,
        }
    }

    #[cfg(test)]
    pub(crate) fn stable_bytes(&self) -> usize {
        self.stable_bytes
    }
}

fn parse_tail(markdown: &mut Markdown, source: &str) -> Option<MarkdownStream> {
    let barrier = cache_barrier(source);
    let mut depth = 0usize;
    let mut boundary = None;
    let mut block_end = 0;
    for (event, range) in Parser::new_ext(source, OPTIONS).into_offset_iter() {
        if markdown.lines.len() > markdown.limit {
            break;
        }
        if barrier > 0 && depth == 0 && matches!(event, Event::Start(_) | Event::Rule) {
            // NOTE: Wait for a complete following top-level line. A blank line alone may
            // still belong to a loose list, a table preview, or a fenced block.
            let line_start = source[..range.start]
                .rfind('\n')
                .map_or(0, |index| index + 1);
            if block_end > 0
                && line_start >= block_end
                && line_start <= barrier
                && source[range.start..].contains('\n')
                && markdown.current.is_empty()
                && markdown.script_position.is_none()
                && markdown
                    .lines
                    .last()
                    .is_some_and(|line| line.spans.is_empty())
            {
                boundary = Some(MarkdownStream {
                    stable_bytes: line_start,
                    lines: markdown.lines.len(),
                    syntax_budget: markdown.syntax_budget,
                });
            }
        }
        match &event {
            Event::Start(_) => depth += 1,
            Event::End(_) => {
                depth -= 1;
                if depth == 0 {
                    block_end = range.end;
                }
            }
            Event::Rule => block_end = range.end,
            _ => {}
        }
        markdown.event(event);
    }
    markdown.finish_parse();
    boundary
}

fn cache_barrier(source: &str) -> usize {
    // NOTE: A reference definition may change an earlier bracket expression. Nested
    // Markdown document fences may acquire a different closing delimiter later.
    let bracket = source.find('[').unwrap_or(source.len());
    let mut offset = 0;
    for line in source[..bracket].split_inclusive('\n') {
        if document::is_document_opening(line.trim_end_matches('\n')) {
            return offset;
        }
        offset += line.len();
    }
    bracket
}
