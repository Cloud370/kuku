use std::borrow::Cow;

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::text::{replace_tabs, terminal_text};
use crate::theme::{ACCENT, ERROR, MUTED, SUCCESS};

mod code;
mod table;

use code::{CodeBlock, SyntaxBudget};
use table::Table;

pub(crate) fn render_markdown(text: &str, width: usize, limit: usize) -> Vec<Line<'static>> {
    let source = if text.contains("\r\n") {
        Cow::Owned(text.replace("\r\n", "\n"))
    } else {
        Cow::Borrowed(text)
    };
    let clean = replace_tabs(&terminal_text(&source));
    let mut output = Markdown {
        width: width.max(1),
        limit,
        ..Markdown::default()
    };
    let options = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
    for event in Parser::new_ext(&clean, options) {
        output.event(event);
    }
    output.finish();
    output.lines
}

pub(crate) fn render_literal(text: &str, style: Style) -> Vec<Line<'static>> {
    replace_tabs(&terminal_text(text))
        .split('\n')
        .map(|line| Line::from(Span::styled(line.to_owned(), style)))
        .collect()
}

pub(crate) fn render_diff(text: &str) -> Vec<Line<'static>> {
    replace_tabs(&terminal_text(text))
        .split('\n')
        .map(|line| {
            let style = match line.as_bytes().first() {
                Some(b'+') => Style::default().fg(SUCCESS),
                Some(b'-') => Style::default().fg(ERROR),
                Some(b'@') => Style::default().fg(ACCENT),
                _ => Style::default(),
            };
            Line::from(Span::styled(line.to_owned(), style))
        })
        .collect()
}

pub(crate) fn wrap(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    wrap_bounded(lines, width, usize::MAX)
}

pub(crate) fn wrap_bounded(
    lines: Vec<Line<'static>>,
    width: usize,
    limit: usize,
) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut wrapped = Vec::new();
    for line in lines {
        let mut parts = Vec::new();
        let mut occupied = 0;
        for span in line.spans {
            let style = line.style.patch(span.style);
            for grapheme in span.content.graphemes(true) {
                let mut cells = grapheme.width();
                let grapheme = if cells > width {
                    cells = 1;
                    "�"
                } else {
                    grapheme
                };
                if occupied + cells > width {
                    if wrapped.len() == limit {
                        return truncated(wrapped);
                    }
                    wrapped.push(Line::from(std::mem::take(&mut parts)));
                    occupied = 0;
                }
                append(&mut parts, grapheme, style);
                occupied += cells;
            }
        }
        if wrapped.len() == limit {
            return truncated(wrapped);
        }
        wrapped.push(Line::from(parts));
    }
    wrapped
}

fn truncated(mut lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    lines.push(Line::styled("…", Style::default().fg(MUTED)));
    lines
}

#[derive(Default)]
struct Markdown {
    width: usize,
    limit: usize,
    lines: Vec<Line<'static>>,
    current: Vec<Span<'static>>,
    table: Option<Table>,
    code: Option<CodeBlock>,
    syntax_budget: SyntaxBudget,
    heading: Option<HeadingLevel>,
    modifiers: Modifier,
    lists: Vec<Option<u64>>,
    quotes: usize,
    link: bool,
}

impl Markdown {
    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => self.text(&text),
            Event::Code(text) | Event::InlineMath(text) => {
                self.push(&text, Style::default().fg(ACCENT));
            }
            Event::DisplayMath(text) => self.text(&text),
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => self.new_line(),
            Event::Rule => {
                self.finish();
                self.push("────", Style::default().fg(MUTED));
                self.paragraph();
            }
            Event::TaskListMarker(checked) => {
                self.push(if checked { "[x] " } else { "[ ] " }, Style::default());
            }
            Event::FootnoteReference(text) => self.text(&text),
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Heading { level, .. } => {
                self.paragraph();
                self.heading = Some(level);
            }
            Tag::BlockQuote(_) => {
                self.finish();
                self.quotes += 1;
            }
            Tag::CodeBlock(kind) => {
                self.paragraph();
                self.code = Some(CodeBlock::new(kind));
            }
            Tag::List(start) => {
                self.finish();
                self.lists.push(start);
            }
            Tag::Table(alignments) => {
                self.paragraph();
                self.table = Some(Table::new(alignments));
            }
            Tag::TableHead | Tag::TableRow => {
                self.table.as_mut().expect("table row").start_row();
            }
            Tag::Item => self.list_marker(),
            Tag::Emphasis => self.modifiers.insert(Modifier::ITALIC),
            Tag::Strong => self.modifiers.insert(Modifier::BOLD),
            Tag::Strikethrough => self.modifiers.insert(Modifier::CROSSED_OUT),
            Tag::Link { .. } => self.link = true,
            Tag::Paragraph
            | Tag::HtmlBlock
            | Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::TableCell
            | Tag::Superscript
            | Tag::Subscript
            | Tag::Image { .. }
            | Tag::MetadataBlock(_) => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_) => {
                self.heading = None;
                self.paragraph();
            }
            TagEnd::BlockQuote(_) => {
                self.finish();
                self.quotes = self.quotes.saturating_sub(1);
                self.paragraph();
            }
            TagEnd::CodeBlock => self.finish_code(),
            TagEnd::List(_) => {
                self.lists.pop();
                self.paragraph();
            }
            TagEnd::TableCell => {
                self.new_line();
                self.table.as_mut().expect("table cell").finish_cell();
            }
            TagEnd::Table => self.finish_table(),
            TagEnd::Item => self.finish(),
            TagEnd::Emphasis => self.modifiers.remove(Modifier::ITALIC),
            TagEnd::Strong => self.modifiers.remove(Modifier::BOLD),
            TagEnd::Strikethrough => self.modifiers.remove(Modifier::CROSSED_OUT),
            TagEnd::Link => self.link = false,
            TagEnd::Paragraph | TagEnd::HtmlBlock => self.paragraph(),
            TagEnd::FootnoteDefinition
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::TableHead
            | TagEnd::TableRow
            | TagEnd::Superscript
            | TagEnd::Subscript
            | TagEnd::Image
            | TagEnd::MetadataBlock(_) => {}
        }
    }

    fn list_marker(&mut self) {
        self.finish();
        let marker = if let Some(Some(number)) = self.lists.last_mut() {
            let marker = format!("{number}. ");
            *number = number.saturating_add(1);
            marker
        } else {
            "• ".to_owned()
        };
        let indent = "  ".repeat(self.lists.len().saturating_sub(1));
        self.push(&format!("{indent}{marker}"), Style::default().fg(ACCENT));
    }

    fn text(&mut self, text: &str) {
        if let Some(code) = &mut self.code {
            code.push(text);
            return;
        }
        let style = if self.link {
            Style::default()
                .fg(ACCENT)
                .add_modifier(Modifier::UNDERLINED | self.modifiers)
        } else {
            Style::default().add_modifier(self.modifiers)
        };
        let mut lines = text.split('\n');
        self.push(lines.next().unwrap_or(""), style);
        for line in lines {
            self.new_line();
            self.push(line, style);
        }
    }

    fn push(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        if self.table.is_none() && self.current.is_empty() && self.quotes > 0 {
            let indentation = "│ ".repeat(self.quotes);
            append(&mut self.current, &indentation, Style::default().fg(MUTED));
        }
        let style = match self.heading {
            Some(HeadingLevel::H1 | HeadingLevel::H2) => style.add_modifier(Modifier::BOLD),
            Some(HeadingLevel::H3 | HeadingLevel::H4 | HeadingLevel::H5 | HeadingLevel::H6) => {
                style.add_modifier(Modifier::ITALIC)
            }
            None => style,
        };
        append(&mut self.current, &terminal_text(text), style);
    }

    fn finish(&mut self) {
        if !self.current.is_empty() {
            self.new_line();
        }
    }

    fn new_line(&mut self) {
        let line = Line::from(std::mem::take(&mut self.current));
        if let Some(table) = &mut self.table {
            table.push_cell_line(line);
        } else {
            self.lines.push(line);
        }
    }

    fn finish_table(&mut self) {
        let table = self.table.take().expect("table end");
        let (width, limit, inset) = self.block_area();
        self.append_block(table.render(width, limit), &inset);
    }

    fn finish_code(&mut self) {
        let code = self.code.take().expect("code block end");
        let (width, limit, inset) = self.block_area();
        let rows = code.render(width, limit, &mut self.syntax_budget);
        self.append_block(rows, &inset);
    }

    fn block_area(&self) -> (usize, usize, String) {
        let quotes = self.quotes.min(self.width.saturating_sub(2) / 2);
        let inset = "│ ".repeat(quotes);
        let width = self.width - inset.width();
        let limit = self.limit.saturating_sub(self.lines.len());
        (width, limit, inset)
    }

    fn append_block(&mut self, rows: Vec<Line<'static>>, inset: &str) {
        self.lines.extend(rows.into_iter().map(|mut row| {
            if !inset.is_empty() && !row.spans.is_empty() {
                row.spans.insert(
                    0,
                    Span::styled(inset.to_owned(), Style::default().fg(MUTED)),
                );
            }
            row
        }));
        self.paragraph();
    }

    fn paragraph(&mut self) {
        self.finish();
        if self.lines.last().is_some_and(|line| !line.spans.is_empty()) {
            self.new_line();
        }
    }
}

fn append(parts: &mut Vec<Span<'static>>, text: &str, style: Style) {
    if let Some(last) = parts.last_mut().filter(|last| last.style == style) {
        last.content.to_mut().push_str(text);
    } else {
        parts.push(Span::styled(text.to_owned(), style));
    }
}

#[cfg(test)]
mod tests {
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    use unicode_width::UnicodeWidthStr;

    use super::{render_markdown, wrap_bounded};
    use crate::theme::{
        ACCENT, CODE_BACKGROUND, CODE_COMMENT, CODE_KEYWORD, CODE_STRING, CODE_TEXT,
    };

    const RICH_TABLE: &str = "| 项目 | Result | Cost |\n\
                              | :--- | :---: | ---: |\n\
                              | 中文 | `a\\|b` | 7 |\n\
                              | 一 | x\\|y | 100 |";
    const LIMIT: usize = 50;

    fn rows(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect()
    }

    fn column_edges(line: &str) -> Vec<usize> {
        line.match_indices('│')
            .map(|(index, _)| line[..index].width())
            .collect()
    }

    #[test]
    fn wrapped_lines_keep_role_emphasis_and_span_colors() {
        let line = Line::from(vec![Span::styled("甲乙", Style::new().fg(ACCENT))])
            .style(Style::new().add_modifier(Modifier::BOLD));
        for row in super::wrap(vec![line], 2) {
            assert!(row.spans[0].style.add_modifier.contains(Modifier::BOLD));
            assert_eq!(row.spans[0].style.fg, Some(ACCENT));
        }
    }

    #[test]
    fn gfm_table_keeps_unicode_inline_code_and_escaped_pipes_in_aligned_columns() {
        let source = format!(
            "Before.\r\n\r\n{}\r\n\r\nAfter.",
            RICH_TABLE.replace('\n', "\r\n")
        );
        let lines = render_markdown(&source, 40, LIMIT);
        let plain = rows(&lines);
        let body: Vec<_> = plain.iter().filter(|line| line.starts_with('│')).collect();
        assert_eq!(3, body.len());
        assert_eq!(vec![0, 7, 16, 23], column_edges(body[0]));
        assert!(body
            .iter()
            .all(|line| column_edges(line) == column_edges(body[0])));
        assert_eq!("│ 中文 │  a|b   │    7 │", body[1]);
        assert_eq!("│ 一   │  x|y   │  100 │", body[2]);
        assert!(lines
            .iter()
            .flat_map(|line| &line.spans)
            .any(|span| { span.content == "a|b" && span.style.fg == Some(ACCENT) }));
        assert!(lines.iter().flat_map(|line| &line.spans).any(|span| {
            span.content == "项目" && span.style.add_modifier.contains(Modifier::BOLD)
        }));
        assert_eq!("Before.", plain.first().unwrap());
        assert!(plain.iter().any(|line| line == "After."));
    }

    #[test]
    fn constrained_grid_wraps_long_cells_with_stable_column_edges() {
        let source = "| Key | Value |\n| --- | --- |\n| Id | `abcdefghijklmnopqrstuvwx` |";
        let width = 24;
        let plain = rows(&render_markdown(source, width, LIMIT));
        let body: Vec<_> = plain.iter().filter(|line| line.starts_with('│')).collect();
        assert_eq!(3, body.len());
        assert!(body.iter().all(|line| column_edges(line) == vec![0, 6, 23]));
        assert!(plain.iter().all(|line| line.width() <= width));
        let value: String = body[1..]
            .iter()
            .map(|line| line.split('│').nth(2).unwrap().trim())
            .collect();
        assert_eq!("abcdefghijklmnopqrstuvwx", value);
    }

    #[test]
    fn narrow_table_keeps_fields_attached_to_each_logical_row() {
        let source =
            "| Id | 列 | Note |\n| --- | --- | --- |\n| one | 中 | `a\\|b` |\n| two | 文 | ok |";
        let width = 12;
        let plain = rows(&render_markdown(source, width, LIMIT));
        assert_eq!(
            [
                "Id: one",
                "列: 中",
                "Note: a|b",
                "",
                "Id: two",
                "列: 文",
                "Note: ok",
                ""
            ],
            plain.as_slice()
        );
        assert!(plain.iter().all(|line| line.width() <= width));
    }

    #[test]
    fn quoted_table_stays_within_width_and_respects_the_row_limit() {
        let source = format!("> {}", RICH_TABLE.replace('\n', "\n> "));
        let width = 26;
        let limit = 5;
        let plain = rows(&wrap_bounded(
            render_markdown(&source, width, limit),
            width,
            limit,
        ));
        assert_eq!(limit + 1, plain.len());
        assert!(plain.first().unwrap().starts_with("│ ┌"));
        assert_eq!("…", plain.last().unwrap());
        assert!(plain.iter().all(|line| line.width() <= width));
    }

    #[test]
    fn arriving_delimiters_reflow_partial_markdown_without_losing_text() {
        let width = 34;
        let mut source = String::from("Adjacent **marker");
        assert_eq!(
            "Adjacent **marker",
            rows(&render_markdown(&source, width, LIMIT))[0]
        );

        source.push_str("**\n\n| 列 | 值 |\n");
        let before_table = rows(&render_markdown(&source, width, LIMIT));
        assert!(before_table.iter().any(|line| line == "Adjacent marker"));
        assert!(before_table.iter().any(|line| line == "| 列 | 值 |"));

        source.push_str("| --- | --- |\n| 中文 | `a\\|b` |\n\n```rust\nlet 值");
        let before_fence = rows(&render_markdown(&source, width, LIMIT));
        assert!(before_fence.iter().any(|line| line.starts_with('┌')));
        assert!(before_fence.iter().any(|line| line.contains("中文 │ a|b")));
        assert!(before_fence
            .iter()
            .any(|line| line.starts_with("│ let 值 ")));
        assert!(before_fence.iter().any(|line| line.starts_with('╰')));

        source.push_str(" = 1;\n```\n\nFinished.");
        let complete = rows(&render_markdown(&source, width, LIMIT));
        assert_eq!(
            1,
            complete
                .iter()
                .filter(|line| line.starts_with("│ let 值 = 1; "))
                .count()
        );
        assert_eq!(
            1,
            complete.iter().filter(|line| *line == "Finished.").count()
        );
    }

    #[test]
    fn pipe_text_and_fenced_code_keep_their_markdown_meaning() {
        let source = "| 项目 | 值 | | --- | --- | | A | B |";
        let plain = rows(&render_markdown(source, 60, LIMIT));
        assert_eq!([source, ""], plain.as_slice());

        let fenced = format!("```text\n{RICH_TABLE}\n```\n\nOrdinary\nparagraph.");
        let plain = rows(&render_markdown(&fenced, 60, LIMIT));
        assert!(plain
            .iter()
            .any(|line| line.starts_with("│ | 项目 | Result | Cost | ")));
        assert!(plain.iter().any(|line| line == "Ordinary paragraph."));
        assert!(plain.iter().all(|line| !line.starts_with('┌')));
    }

    #[test]
    fn fence_labels_and_highlights_lexical_scopes_across_lines() {
        let source = concat!(
            "```rust linenos\n",
            "let greeting = \"let 中文\";\n",
            "/* let 42\n",
            "   let still a comment */\n",
            "let answer = 42;\n",
            "```",
        );
        let lines = render_markdown(source, 48, LIMIT);
        let plain = rows(&lines);
        assert!(plain[0].starts_with("╭─ rust "));
        assert!(!plain[0].contains("linenos"));
        assert!(plain[3].starts_with("│    let still a comment */"));
        assert!(plain[5].starts_with('╰'));
        assert!(lines[..6].iter().all(|line| line.width() == 48));
        assert!(lines[..6].iter().all(|line| line
            .spans
            .iter()
            .all(|span| { line.style.patch(span.style).bg == Some(CODE_BACKGROUND) })));
        for (row, part, color) in [
            (1, "let", CODE_KEYWORD),
            (1, "let 中文", CODE_STRING),
            (2, "let 42", CODE_COMMENT),
            (3, "let still a comment", CODE_COMMENT),
            (4, "let", CODE_KEYWORD),
        ] {
            assert!(
                lines[row]
                    .spans
                    .iter()
                    .any(|span| { span.content.contains(part) && span.style.fg == Some(color) }),
                "missing color for {part:?} in {:?}",
                lines[row]
            );
        }
    }

    #[test]
    fn unknown_fences_and_long_lines_keep_plain_code_and_independent_scopes() {
        let long = "/".repeat(1100);
        let source = format!(
            concat!(
                "```unknown_lang\nlet unknown = 3;\n```\n\n",
                "```rust\n{}\nlet after_long = 4;\n```\n\n",
                "```python\nreturn 5\n```",
            ),
            long
        );
        let lines = render_markdown(&source, 80, LIMIT);
        assert!(lines[0].to_string().starts_with("╭─ unknown_lang "));
        for word in ["let unknown", "let after_long"] {
            assert!(lines
                .iter()
                .flat_map(|line| &line.spans)
                .any(|span| { span.content.contains(word) && span.style.fg == Some(CODE_TEXT) }));
        }
        assert!(lines
            .iter()
            .flat_map(|line| &line.spans)
            .any(|span| { span.content == "return" && span.style.fg == Some(CODE_KEYWORD) }));
    }

    #[test]
    fn quoted_code_wraps_original_unicode_graphemes_at_narrow_widths() {
        let source = "> ```text\n>  中文👩‍💻𠮷字\n> ```";
        for width in 4..17 {
            let lines = render_markdown(source, width, LIMIT);
            let plain = rows(&lines);
            assert!(lines.iter().all(|line| line.width() <= width));
            assert!(plain.iter().all(|line| !line.contains('�')));
            let body: String = plain
                .iter()
                .skip(1)
                .take_while(|line| !line.is_empty() && !line.contains('╰'))
                .flat_map(|line| line.chars())
                .filter(|ch| *ch != '│' && !ch.is_whitespace())
                .collect();
            assert_eq!("中文👩‍💻𠮷字", body, "width {width}");
        }
    }

    #[test]
    fn headings_use_spacing_and_typography() {
        let lines = render_markdown("# First *part*\n\n### Detail\n\nBody", 30, LIMIT);
        assert_eq!(
            ["First part", "", "Detail", "", "Body", ""],
            rows(&lines).as_slice()
        );
        assert!(lines[0]
            .spans
            .iter()
            .all(|span| span.style.add_modifier.contains(Modifier::BOLD)));
        assert!(lines[2].spans[0]
            .style
            .add_modifier
            .contains(Modifier::ITALIC));
        assert!(lines[4].spans[0].style.add_modifier.is_empty());
    }
}
