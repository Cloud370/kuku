use std::ops::Range;

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::text::{replace_tabs, shorten, terminal_text};
use crate::theme::{emphasis, MODEL, MUTED};

use super::{DetailKey, Transcript, TranscriptView};

const MAX_NOTE_ROWS: usize = 5;
const MAX_NOTE_WIDTH: usize = 80;
const GUTTER: &str = "│ ";

#[derive(Debug)]
pub(super) struct ThinkingRead {
    pub(super) key: DetailKey,
    text: String,
    rows: Vec<String>,
    width: usize,
    origin: Range<usize>,
    area: Range<usize>,
    skipped: usize,
    top: usize,
    was_following: bool,
}

impl ThinkingRead {
    pub(super) fn paint(
        &mut self,
        view: &mut TranscriptView,
        origin: Option<Range<usize>>,
        width: usize,
        height: usize,
    ) {
        if height == 0 {
            self.area = 0..0;
            return;
        }
        self.origin = origin.unwrap_or_else(|| {
            let row = self.origin.start.min(view.lines.len());
            row..row
        });
        let gutter = if width >= 4 { GUTTER } else { "" };
        let text_width = width
            .saturating_sub(gutter.width())
            .clamp(1, MAX_NOTE_WIDTH);
        if self.width != text_width {
            self.rows = wrap_notes(&self.text, text_width);
            self.width = text_width;
        }
        let count = (height / 4)
            .clamp(1, MAX_NOTE_ROWS)
            .min(self.rows.len())
            .min(height.saturating_sub(1));
        self.top = self.top.min(self.rows.len().saturating_sub(count));
        let start = self.origin.start.min(view.lines.len());
        let end = self.origin.end.min(view.lines.len());
        let after = usize::from(end < view.lines.len() && height >= 3);
        self.skipped = start.saturating_sub(height.saturating_sub(1 + count + after));
        let heading = start - self.skipped;
        self.area = heading..heading + 1 + count;

        let label = if width >= 39 {
            "Assistant   Thinking"
        } else {
            "Thinking"
        };
        let mut title = Line::from(Span::styled(shorten(label, width), emphasis(MODEL)));
        let room = width.saturating_sub(title.width());
        let range = format!("{}–{}/{}", self.top + 1, self.top + count, self.rows.len());
        let hint = if self.rows.len() > count && count > 0 {
            [
                format!("   ↑↓ {range}  ·  Esc back"),
                format!("  {range}  Esc"),
                "  Esc".into(),
            ]
            .into_iter()
            .find(|hint| hint.width() <= room)
            .unwrap_or_default()
        } else {
            shorten("   Esc back", room)
        };
        title.push_span(Span::styled(hint, Style::new().fg(MUTED)));
        let notes = self.rows.iter().skip(self.top).take(count).map(|text| {
            Line::from(vec![
                Span::styled(gutter, Style::new().fg(MODEL)),
                Span::styled(text.clone(), Style::new().fg(MUTED)),
            ])
        });
        view.lines
            .splice(start..end, std::iter::once(title).chain(notes));
        view.row_insets.splice(
            start..end,
            std::iter::once(0).chain(std::iter::repeat_n(gutter.width() as u16, count)),
        );
        view.lines.drain(..self.skipped);
        view.row_insets.drain(..self.skipped);
        view.lines.truncate(height);
        view.row_insets.truncate(height);
        view.thinking_row = None;
    }

    pub(super) fn is_heading(&self, row: usize) -> bool {
        row == self.area.start
    }

    pub(super) fn hit(&self, row: usize) -> bool {
        self.area.contains(&row)
    }

    pub(super) fn source_row(&self, row: usize) -> usize {
        if row < self.area.start {
            row + self.skipped
        } else {
            self.origin.end + row.saturating_sub(self.area.end)
        }
    }

    fn page(&self) -> usize {
        self.area.len().saturating_sub(1).max(1)
    }

    fn scroll(&mut self, amount: isize) {
        self.top = self
            .top
            .saturating_add_signed(amount)
            .min(self.rows.len().saturating_sub(self.page()));
    }
}

fn wrap_notes(text: &str, width: usize) -> Vec<String> {
    let mut rows = Vec::new();
    for paragraph in replace_tabs(&terminal_text(text)).split('\n') {
        let mut row = String::new();
        let mut used = 0;
        for word in paragraph.split_word_bounds() {
            let cells = word.width();
            if used == width && word.trim().is_empty() {
                rows.push(std::mem::take(&mut row));
                used = 0;
                continue;
            }
            if used > 0 && used + cells > width && cells <= width && !word.trim().is_empty() {
                rows.push(std::mem::take(&mut row));
                used = 0;
            }
            for grapheme in word.graphemes(true) {
                let grapheme = if grapheme.width() > width {
                    "�"
                } else {
                    grapheme
                };
                if used + grapheme.width() > width {
                    rows.push(std::mem::take(&mut row));
                    used = 0;
                }
                row.push_str(grapheme);
                used += grapheme.width();
            }
        }
        rows.push(row);
    }
    rows
}

impl Transcript {
    pub(crate) fn open_notes(&mut self, key: &DetailKey, width: usize, height: usize) -> bool {
        let Some((text, origin)) = self.notes_at(key) else {
            return false;
        };
        let text = text.to_owned();
        let was_following = !self.is_reading();
        if was_following {
            self.scroll(0, width, height);
        }
        self.notes = Some(ThinkingRead {
            key: key.clone(),
            text,
            rows: Vec::new(),
            width: 0,
            origin,
            area: 0..0,
            skipped: 0,
            top: 0,
            was_following,
        });
        true
    }

    pub(crate) fn close_notes(&mut self) -> bool {
        let Some(notes) = self.notes.take() else {
            return false;
        };
        if notes.was_following && !self.reading_has_new_content().unwrap_or(false) {
            self.show_latest();
        }
        true
    }

    pub(crate) fn note_is_open(&self, key: &DetailKey) -> bool {
        self.notes.as_ref().is_some_and(|notes| &notes.key == key)
    }

    pub(crate) fn notes_at_row(&self, row: usize) -> bool {
        self.notes.as_ref().is_some_and(|notes| notes.hit(row))
    }

    pub(crate) fn scroll_notes(&mut self, amount: isize) -> bool {
        if let Some(notes) = &mut self.notes {
            notes.scroll(amount);
            true
        } else {
            false
        }
    }

    pub(crate) fn notes_page(&self) -> Option<usize> {
        self.notes.as_ref().map(ThinkingRead::page)
    }
}

#[cfg(test)]
mod tests {
    use unicode_width::UnicodeWidthStr;

    use super::wrap_notes;

    #[test]
    fn notes_wrap_words_and_preserve_literal_unicode_and_blank_lines() {
        assert_eq!(wrap_notes("Hello world", 5), ["Hello", "world"]);
        let rows = wrap_notes("Check the answer\n\n变量 👩‍💻 AAAAAAA", 6);
        assert_eq!(&rows[..4], ["Check ", "the ", "answer", ""]);
        assert!(rows.iter().all(|row| row.width() <= 6));
        assert_eq!(rows[4..].concat(), "变量 👩‍💻 AAAAAAA");
    }
}
