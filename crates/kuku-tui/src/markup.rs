use std::borrow::Cow;

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::text::{replace_tabs, shorten, terminal_text};
use crate::theme::{append_line_count, ACCENT, ERROR, MARKDOWN_EMPHASIS, MUTED, SUCCESS};

mod code;
mod document;
mod inline;
mod links;
pub(crate) use links::{LinkDestination, LinkLayout};
mod stream;
pub(crate) use stream::MarkdownStream;
mod table;

pub(crate) use code::file_source;
use code::{CodeBlock, SyntaxBudget};
use table::Table;

const OPTIONS: Options = Options::ENABLE_STRIKETHROUGH
    .union(Options::ENABLE_TABLES)
    .union(Options::ENABLE_TASKLISTS);

pub(crate) fn render_markdown(text: &str, width: usize, limit: usize) -> Vec<Line<'static>> {
    parse_markdown(text, width, limit, None).lines
}

pub(crate) fn linked_markdown(
    text: &str,
    width: usize,
    limit: usize,
) -> (Vec<Line<'static>>, LinkLayout) {
    let parsed = parse_markdown(text, width, limit, Some(LinkLayout::default()));
    (parsed.lines, parsed.link_layout.unwrap_or_default())
}

fn parse_markdown(
    text: &str,
    width: usize,
    limit: usize,
    link_layout: Option<LinkLayout>,
) -> Markdown {
    let clean = clean_markdown(text);
    let mut output = Markdown {
        width: width.max(1),
        limit,
        preview_documents: true,
        link_layout,
        ..Markdown::default()
    };
    output.parse(&document::repair_fences(&clean));
    output
}

fn clean_markdown(text: &str) -> String {
    let source = if text.contains("\r\n") {
        Cow::Owned(text.replace("\r\n", "\n"))
    } else {
        Cow::Borrowed(text)
    };
    replace_tabs(&terminal_text(&source))
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
    wrap_with_indent(lines, width, limit, "")
}

pub(crate) fn number_gutter(digits: usize) -> usize {
    digits + usize::from(digits != 0)
}

pub(crate) fn number_line(mut line: Line<'static>, number: usize, digits: usize) -> Line<'static> {
    if digits > 0 {
        line.spans.insert(
            0,
            Span::styled(format!("{number:>digits$} "), Style::new().fg(MUTED)),
        );
    }
    line
}

pub(crate) fn wrap_numbered(
    lines: Vec<Line<'static>>,
    width: usize,
    limit: usize,
    first: usize,
    total: usize,
) -> Vec<Line<'static>> {
    let end = first.saturating_add(lines.len().saturating_sub(1));
    let digits = end.max(total).to_string().len();
    if width < digits + 3 {
        return wrap_bounded(lines, width, limit);
    }
    let numbered = lines
        .into_iter()
        .enumerate()
        .map(|(index, line)| number_line(line, first + index, digits))
        .collect();
    wrap_with_indent(numbered, width, limit, &" ".repeat(number_gutter(digits)))
}

pub(crate) fn wrap_with_indent(
    lines: Vec<Line<'static>>,
    width: usize,
    limit: usize,
    indent: &str,
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
                    if !indent.is_empty() && indent.width() + cells <= width {
                        append(&mut parts, indent, Style::default().fg(MUTED));
                        occupied = indent.width();
                    }
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
    preview_documents: bool,
    lines: Vec<Line<'static>>,
    current: Vec<Span<'static>>,
    table: Option<Table>,
    code: Option<CodeBlock>,
    syntax_budget: SyntaxBudget,
    heading: Option<HeadingLevel>,
    modifiers: Modifier,
    emphasis_depth: usize,
    lists: Vec<Option<u64>>,
    quotes: usize,
    link: Option<Color>,
    link_layout: Option<LinkLayout>,
    script_position: Option<inline::Position>,
}

impl Markdown {
    fn parse(&mut self, source: &str) {
        for event in Parser::new_ext(source, OPTIONS) {
            if self.lines.len() > self.limit {
                break;
            }
            self.event(event);
        }
        self.finish_parse();
    }

    fn finish_parse(&mut self) {
        self.finish();
        self.lines.truncate(self.limit.saturating_add(1));
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) | Event::Html(text) => self.text(&text),
            Event::InlineHtml(tag) => self.inline_html(&tag),
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
            Tag::Emphasis => self.emphasis_depth += 1,
            Tag::Strong => self.modifiers.insert(Modifier::BOLD),
            Tag::Strikethrough => self.modifiers.insert(Modifier::CROSSED_OUT),
            Tag::Link { dest_url, .. } => {
                self.link = crate::browser::is_http_url(&dest_url).then(|| {
                    self.link_layout
                        .as_mut()
                        .map_or(ACCENT, |layout| layout.register(&dest_url))
                });
            }
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
            TagEnd::Emphasis => self.emphasis_depth -= 1,
            TagEnd::Strong => self.modifiers.remove(Modifier::BOLD),
            TagEnd::Strikethrough => self.modifiers.remove(Modifier::CROSSED_OUT),
            TagEnd::Link => self.link = None,
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
        let converted = self.script_position.map(|position| position.convert(text));
        let text = converted.as_deref().unwrap_or(text);
        let mut style = Style::default().add_modifier(self.modifiers);
        if self.emphasis_depth > 0 {
            style = style.fg(MARKDOWN_EMPHASIS);
        }
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
        let style = self.link.map_or(style, |color| {
            style.fg(color).add_modifier(Modifier::UNDERLINED)
        });
        let style = match self.heading {
            Some(_) => style.add_modifier(Modifier::BOLD),
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
        } else if (!self.lists.is_empty() || self.quotes > 0) && line.width() > self.width {
            let (_, limit, inset) = self.block_area();
            self.lines.extend(wrap_with_indent(
                vec![line],
                self.width,
                limit.saturating_add(1),
                &inset,
            ));
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
        let rows = match code.markdown() {
            Some(source) if self.preview_documents => self.render_document(source, width, limit),
            Some(_) | None => code.render(width, limit, &mut self.syntax_budget),
        };
        self.append_block(rows, &inset);
    }

    fn render_document(&mut self, source: &str, width: usize, limit: usize) -> Vec<Line<'static>> {
        let full_label = "Markdown preview · Messages";
        let compact_label = "Markdown · Messages";
        let label = if width >= full_label.width() {
            full_label
        } else if width >= compact_label.width() {
            compact_label
        } else {
            "Markdown"
        };
        let mut title = Line::styled(shorten(label, width), Style::new().fg(MUTED));
        append_line_count(&mut title, width, source.lines().count());
        let mut rows = vec![title];
        if limit > 0 {
            let mut document = Markdown {
                width,
                limit: limit - 1,
                syntax_budget: std::mem::take(&mut self.syntax_budget),
                link_layout: self.link_layout.take(),
                ..Markdown::default()
            };
            document.parse(source);
            self.syntax_budget = document.syntax_budget;
            self.link_layout = document.link_layout;
            rows.extend(document.lines);
        }
        rows
    }

    fn block_area(&self) -> (usize, usize, String) {
        let quotes = self.quotes.min(self.width.saturating_sub(2) / 2);
        let mut inset = "│ ".repeat(quotes);
        if let Some(counter) = self.lists.last() {
            let marker_width =
                counter.map_or(2, |next| next.saturating_sub(1).to_string().len() + 2);
            let indent = 2 * (self.lists.len() - 1) + marker_width;
            inset.push_str(&" ".repeat(indent.min(self.width.saturating_sub(inset.width() + 2))));
        }
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
mod tests;
