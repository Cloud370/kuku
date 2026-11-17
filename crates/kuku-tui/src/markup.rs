use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::text::{replace_tabs, single_line, terminal_text};

pub(crate) const ACCENT: Color = Color::Cyan;
pub(crate) const MUTED: Color = Color::DarkGray;

pub(crate) fn render_markdown(text: &str) -> Vec<Line<'static>> {
    let clean = replace_tabs(&terminal_text(text));
    let mut output = Markdown::default();
    for event in Parser::new_ext(&clean, Options::ENABLE_STRIKETHROUGH) {
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
            let color = match line.as_bytes().first() {
                Some(b'+') => Color::Green,
                Some(b'-') => Color::Red,
                Some(b'@') => ACCENT,
                _ => Color::Reset,
            };
            Line::from(Span::styled(line.to_owned(), Style::default().fg(color)))
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
                append(&mut parts, grapheme, span.style);
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
    lines: Vec<Line<'static>>,
    current: Vec<Span<'static>>,
    modifiers: Modifier,
    lists: Vec<Option<u64>>,
    quotes: usize,
    code: bool,
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
                self.finish();
                self.modifiers.insert(Modifier::BOLD);
                if matches!(level, HeadingLevel::H1 | HeadingLevel::H2) {
                    self.push("▰ ", Style::default().fg(ACCENT));
                }
            }
            Tag::BlockQuote(_) => {
                self.finish();
                self.quotes += 1;
            }
            Tag::CodeBlock(kind) => {
                self.finish();
                let label = match kind {
                    CodeBlockKind::Fenced(language) if !language.is_empty() => {
                        single_line(&language)
                    }
                    CodeBlockKind::Indented | CodeBlockKind::Fenced(_) => "Code".into(),
                };
                self.push(&label, Style::default().fg(ACCENT));
                self.finish();
                self.code = true;
            }
            Tag::List(start) => {
                self.finish();
                self.lists.push(start);
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
            | Tag::Table(_)
            | Tag::TableHead
            | Tag::TableRow
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
                self.modifiers.remove(Modifier::BOLD);
                self.paragraph();
            }
            TagEnd::BlockQuote(_) => {
                self.finish();
                self.quotes = self.quotes.saturating_sub(1);
                self.paragraph();
            }
            TagEnd::CodeBlock => {
                self.finish();
                self.code = false;
                self.paragraph();
            }
            TagEnd::List(_) => {
                self.lists.pop();
                self.paragraph();
            }
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
            | TagEnd::Table
            | TagEnd::TableHead
            | TagEnd::TableRow
            | TagEnd::TableCell
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
        let style = if self.code {
            Style::default().fg(Color::Gray)
        } else if self.link {
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
        if self.current.is_empty() && (self.quotes > 0 || self.code) {
            let indentation = "│ ".repeat(self.quotes + usize::from(self.code));
            append(&mut self.current, &indentation, Style::default().fg(MUTED));
        }
        append(&mut self.current, &terminal_text(text), style);
    }

    fn finish(&mut self) {
        if !self.current.is_empty() {
            self.new_line();
        }
    }

    fn new_line(&mut self) {
        self.lines
            .push(Line::from(std::mem::take(&mut self.current)));
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
