use std::collections::VecDeque;
use std::ops::Range;

use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation};
use unicode_width::UnicodeWidthStr;

use crate::text::{replace_tabs, terminal_text, TAB};

const MAX_HISTORY: usize = 64;
const MAX_SAVED_PROMPT_BYTES: usize = 32 * 1024;

#[derive(Debug, Default)]
pub(crate) struct Editor {
    text: String,
    cursor: usize,
    top: usize,
    preferred_column: Option<usize>,
    history: VecDeque<String>,
    recall: Option<(usize, String, usize)>,
}

impl Editor {
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    pub(crate) fn set_text(&mut self, text: String) {
        self.text = if text.contains('\r') {
            normalized(&text)
        } else {
            text
        };
        self.cursor = self.text.len();
        self.reset_motion();
    }

    pub(crate) fn take_text(&mut self) -> String {
        self.cursor = 0;
        self.top = 0;
        self.reset_motion();
        std::mem::take(&mut self.text)
    }

    pub(crate) fn insert(&mut self, text: &str) {
        self.replace(self.cursor..self.cursor, text);
    }

    pub(crate) fn replace(&mut self, range: Range<usize>, text: &str) {
        let text = normalized(text);
        self.text.replace_range(range.clone(), &text);
        let end = range.start + text.len();
        self.cursor = self.boundary_at_or_after(end);
        self.reset_motion();
    }

    pub(crate) fn completion_range(&self) -> Range<usize> {
        let start = self.text[..self.cursor]
            .rfind(char::is_whitespace)
            .map(|offset| {
                offset
                    + self.text[offset..]
                        .chars()
                        .next()
                        .expect("whitespace")
                        .len_utf8()
            })
            .unwrap_or(0);
        start..self.cursor
    }

    pub(crate) fn cursor(&self) -> usize {
        self.cursor
    }

    pub(crate) fn left(&mut self) {
        self.cursor = self.previous_boundary();
        self.preferred_column = None;
    }

    pub(crate) fn right(&mut self) {
        self.cursor = self.next_boundary();
        self.preferred_column = None;
    }

    pub(crate) fn word_left(&mut self) {
        self.cursor = self.word_start();
        self.preferred_column = None;
    }

    pub(crate) fn word_right(&mut self) {
        self.cursor += self.text[self.cursor..]
            .unicode_word_indices()
            .next()
            .map_or(self.text.len() - self.cursor, |(offset, word)| {
                offset + word.len()
            });
        self.cursor = self.boundary_at_or_after(self.cursor);
        self.preferred_column = None;
    }

    pub(crate) fn home(&mut self) {
        self.cursor = self.text[..self.cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        self.preferred_column = None;
    }

    pub(crate) fn end(&mut self) {
        self.cursor = self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |index| self.cursor + index);
        self.preferred_column = None;
    }

    pub(crate) fn backspace(&mut self) {
        self.replace(self.previous_boundary()..self.cursor, "");
    }

    pub(crate) fn delete(&mut self) {
        self.replace(self.cursor..self.next_boundary(), "");
    }

    pub(crate) fn delete_word(&mut self) {
        self.replace(self.word_start()..self.cursor, "");
    }

    pub(crate) fn vertical(&mut self, width: usize, direction: isize) {
        let rows = visual_rows(&self.text, width.max(1));
        let current = row_index(&rows, self.cursor);
        let Some(destination) = current
            .checked_add_signed(direction)
            .filter(|row| *row < rows.len())
        else {
            self.recall(direction);
            return;
        };
        let row = &rows[current];
        let preferred = self
            .preferred_column
            .get_or_insert_with(|| columns(&self.text[row.start..self.cursor]));
        let row = &rows[destination];
        let mut offset = offset_at_column(&self.text, row, *preferred);
        if rows
            .get(destination + 1)
            .is_some_and(|next| next.start == offset)
        {
            offset = self.text[row.clone()]
                .grapheme_indices(true)
                .next_back()
                .map_or(row.start, |(index, _)| row.start + index);
        }
        self.cursor = offset;
    }

    pub(crate) fn remember(&mut self, prompt: &str) {
        if prompt.len() <= MAX_SAVED_PROMPT_BYTES
            && self.history.front().is_none_or(|old| old != prompt)
        {
            self.history.retain(|previous| previous != prompt);
            self.history.push_front(prompt.to_owned());
            self.history.truncate(MAX_HISTORY);
            self.recall = None;
        }
    }

    pub(crate) fn recall(&mut self, direction: isize) {
        match (direction.is_negative(), self.recall.take()) {
            (true, None) if !self.history.is_empty() => {
                let draft = std::mem::take(&mut self.text);
                self.recall = Some((0, draft, self.cursor));
                self.text.clone_from(&self.history[0]);
                self.cursor = self.text.len();
            }
            (true, Some((index, draft, cursor))) => {
                let index = (index + 1).min(self.history.len() - 1);
                self.text.clone_from(&self.history[index]);
                self.cursor = self.text.len();
                self.recall = Some((index, draft, cursor));
            }
            (false, Some((0, draft, cursor))) => {
                self.text = draft;
                self.cursor = cursor;
            }
            (false, Some((index, draft, cursor))) => {
                self.text.clone_from(&self.history[index - 1]);
                self.cursor = self.text.len();
                self.recall = Some((index - 1, draft, cursor));
            }
            (_, None) => {}
        }
        self.preferred_column = None;
    }

    pub(crate) fn project(&mut self, width: usize, height: usize) -> EditorView {
        let width = width.max(1);
        let height = height.max(1);
        let rows = visual_rows(&self.text, width);
        let cursor_row = row_index(&rows, self.cursor);
        self.top = self.top.min(rows.len().saturating_sub(height));
        self.top = self
            .top
            .max(cursor_row.saturating_sub(height - 1))
            .min(cursor_row);
        let lines = rows
            .iter()
            .skip(self.top)
            .take(height)
            .map(|range| {
                let text = replace_tabs(&terminal_text(&self.text[range.clone()]));
                if text.width() > width {
                    "�".to_owned()
                } else {
                    text
                }
            })
            .collect();
        let column = columns(&self.text[rows[cursor_row].start..self.cursor]);
        EditorView {
            lines,
            cursor: (column.min(width - 1), cursor_row - self.top),
        }
    }

    fn previous_boundary(&self) -> usize {
        self.text[..self.cursor]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    fn next_boundary(&self) -> usize {
        self.cursor
            + self.text[self.cursor..]
                .graphemes(true)
                .next()
                .map_or(0, str::len)
    }

    fn word_start(&self) -> usize {
        self.text[..self.cursor]
            .unicode_word_indices()
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    fn boundary_at_or_after(&self, offset: usize) -> usize {
        let mut cursor = GraphemeCursor::new(offset, self.text.len(), true);
        if cursor.is_boundary(&self.text, 0).expect("complete text") {
            offset
        } else {
            cursor
                .next_boundary(&self.text, 0)
                .expect("complete text")
                .unwrap_or(self.text.len())
        }
    }

    fn reset_motion(&mut self) {
        self.recall = None;
        self.preferred_column = None;
    }
}

pub(crate) struct EditorView {
    pub(crate) lines: Vec<String>,
    pub(crate) cursor: (usize, usize),
}

fn visual_rows(text: &str, width: usize) -> Vec<Range<usize>> {
    let mut rows = Vec::new();
    let mut start = 0;
    let mut used = 0;
    for (offset, grapheme) in text.grapheme_indices(true) {
        if grapheme == "\n" {
            rows.push(start..offset);
            start = offset + 1;
            used = 0;
            continue;
        }
        let cells = grapheme_columns(grapheme).min(width);
        if used > 0 && used + cells > width {
            rows.push(start..offset);
            start = offset;
            used = 0;
        }
        used += cells;
    }
    rows.push(start..text.len());
    if used == width {
        rows.push(text.len()..text.len());
    }
    rows
}

fn row_index(rows: &[Range<usize>], offset: usize) -> usize {
    rows.partition_point(|range| range.start <= offset) - 1
}

fn offset_at_column(text: &str, range: &Range<usize>, column: usize) -> usize {
    let mut used = 0;
    for (offset, grapheme) in text[range.clone()].grapheme_indices(true) {
        used += grapheme_columns(grapheme);
        if used > column {
            return range.start + offset;
        }
    }
    range.end
}

fn columns(text: &str) -> usize {
    text.graphemes(true).map(grapheme_columns).sum()
}

fn grapheme_columns(grapheme: &str) -> usize {
    if grapheme == "\t" {
        TAB.len()
    } else {
        grapheme.width().max(1)
    }
}

fn normalized(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

#[cfg(test)]
mod tests {
    use super::Editor;

    #[test]
    fn paste_normalizes_line_endings_and_backspace_removes_one_grapheme() {
        let mut editor = Editor::default();
        editor.insert("中\r\n文\r👨‍👩‍👧‍👦e\u{301}");
        assert_eq!("中\n文\n👨‍👩‍👧‍👦e\u{301}", editor.text());
        editor.left();
        editor.backspace();
        assert_eq!("中\n文\ne\u{301}", editor.text());
        editor.right();
        editor.backspace();
        assert_eq!("中\n文\n", editor.text());
    }

    #[test]
    fn wrapped_wide_text_places_cursor_and_preserves_preferred_column() {
        let mut editor = Editor::default();
        editor.insert("你好x\n界a");
        let view = editor.project(4, 2);
        assert_eq!(["x", "界a"], view.lines.as_slice());
        assert_eq!((3, 1), view.cursor);
        editor.vertical(4, -1);
        editor.vertical(4, -1);
        let view = editor.project(4, 3);
        assert_eq!((2, 0), view.cursor);
        assert_eq!(["你好", "x", "界a"], view.lines.as_slice());
    }

    #[test]
    fn prompt_history_restores_the_unsent_draft() {
        let mut editor = Editor::default();
        editor.insert("unsent draft");
        editor.remember("older prompt");
        editor.remember("newer prompt");
        editor.recall(-1);
        assert_eq!("newer prompt", editor.text());
        editor.recall(-1);
        assert_eq!("older prompt", editor.text());
        editor.recall(1);
        editor.recall(1);
        assert_eq!("unsent draft", editor.text());
    }
}
