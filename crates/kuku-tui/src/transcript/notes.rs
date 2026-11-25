use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::text::{replace_tabs, shorten, terminal_text};
use crate::theme::{CardFrame, MODEL, MUTED, emphasis};

use super::{AnchorLine, DetailKey, Key, Row, Transcript};

const MAX_NOTE_WIDTH: usize = 80;

#[derive(Debug)]
pub(super) struct ThinkingRead {
    pub(super) key: DetailKey,
    text: String,
    rows: Vec<Row>,
    width: usize,
    was_following: bool,
    in_timeline: bool,
}

impl ThinkingRead {
    pub(super) fn fit(&mut self, width: usize) {
        let width = if self.in_timeline {
            CardFrame::timeline(width).content_width()
        } else {
            width
        };
        if width == self.width {
            return;
        }
        self.width = width;
        let indent = if self.in_timeline {
            super::body_inset(width)
        } else {
            0
        };
        let label = if self.in_timeline || width < 35 {
            "Thinking ▾"
        } else {
            "Assistant   Thinking ▾"
        };
        let mut title = Line::from(vec![
            Span::raw(" ".repeat(usize::from(indent))),
            Span::styled(
                shorten(label, width.saturating_sub(usize::from(indent))),
                emphasis(MODEL),
            ),
        ]);
        let room = width.saturating_sub(title.width());
        if room >= "   Esc back".len() {
            title.push_span(Span::styled("   Esc back", Style::new().fg(MUTED)));
        }

        let gutter = if width >= 4 {
            format!("{}│ ", " ".repeat(usize::from(indent)))
        } else {
            String::new()
        };
        let text_width = width
            .saturating_sub(gutter.width())
            .clamp(1, MAX_NOTE_WIDTH);
        self.rows = std::iter::once(Row::plain(title))
            .chain(
                wrap_notes(&self.text, text_width)
                    .into_iter()
                    .map(|text| Row {
                        line: Line::from(vec![
                            Span::styled(gutter.clone(), Style::new().fg(MODEL)),
                            Span::styled(text, Style::new().fg(MUTED)),
                        ]),
                        inset: gutter.width() as u16,
                        right_inset: 0,
                        links: Vec::new(),
                        script_detail: None,
                        output: None,
                        command_arguments: None,
                        inline_output: None,
                        turn_summary: false,
                    }),
            )
            .collect();
    }

    pub(super) fn rows_for(&self, key: &Key) -> &[Row] {
        if self.key.for_record(key) {
            &self.rows
        } else {
            &[]
        }
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
        let Some((text, in_timeline)) = self.notes_at(key) else {
            return false;
        };
        let mut notes = ThinkingRead {
            key: key.clone(),
            text: text.to_owned(),
            rows: Vec::new(),
            width: usize::MAX,
            was_following: self
                .notes
                .as_ref()
                .map_or(!self.is_reading(), |open| open.was_following),
            in_timeline,
        };
        if !self.is_reading() {
            self.scroll(0, width, height);
            if let Some(reading) = &mut self.reading {
                reading.manual = false;
            }
        }
        notes.fit(width);
        self.notes = Some(notes);
        true
    }

    pub(crate) fn close_notes(&mut self) -> bool {
        let Some(notes) = self.notes.take() else {
            return false;
        };
        if let Some(reading) = &mut self.reading {
            if notes.key.for_record(&reading.anchor.key)
                && matches!(reading.anchor.line, AnchorLine::Note(_))
            {
                reading.anchor.line = AnchorLine::Body(0);
            }
        }
        if notes.was_following
            && self
                .reading
                .as_ref()
                .is_some_and(|reading| !reading.manual && !reading.unseen)
        {
            self.show_latest();
        }
        true
    }

    pub(crate) fn note_is_open(&self, key: &DetailKey) -> bool {
        self.notes.as_ref().is_some_and(|notes| &notes.key == key)
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
