use std::collections::VecDeque;
use std::ops::Range;

use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation};
use unicode_width::UnicodeWidthStr;

use crate::text::{TAB, replace_tabs, terminal_text};

const MAX_HISTORY: usize = 64;
const MAX_SAVED_PROMPT_BYTES: usize = 32 * 1024;
const MAX_HISTORY_IMAGE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Default)]
pub(crate) struct Editor {
    text: String,
    atoms: Vec<Range<usize>>,
    cursor: usize,
    top: usize,
    preferred_column: Option<usize>,
    history: VecDeque<kuku_client::MessageDraft>,
    recall: Option<Recall>,
}

#[derive(Debug)]
struct Recall {
    index: usize,
    text: String,
    cursor: usize,
    atoms: Vec<Range<usize>>,
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
        self.atoms.clear();
        self.cursor = self.text.len();
        self.reset_motion();
    }

    pub(crate) fn set_image_markers<'a>(
        &mut self,
        text: &str,
        markers: impl IntoIterator<Item = &'a kuku_client::ImageMarker>,
    ) {
        self.set_text(text.to_owned());
        self.atoms = markers
            .into_iter()
            .filter_map(|marker| {
                let range = marker.range_in(text)?;
                let offset = if self.text == text {
                    range.start
                } else {
                    normalized(&text[..range.start]).len()
                };
                Some(offset..offset + marker.label.len())
            })
            .collect();
        self.atoms.sort_unstable_by_key(|range| range.start);
    }

    pub(crate) fn image_markers(&self) -> Vec<kuku_client::ImageMarker> {
        self.atoms
            .iter()
            .map(|range| kuku_client::ImageMarker {
                offset: range.start,
                label: self.text[range.clone()].into(),
            })
            .collect()
    }

    pub(crate) fn take_text(&mut self) -> String {
        self.atoms.clear();
        self.cursor = 0;
        self.top = 0;
        self.reset_motion();
        std::mem::take(&mut self.text)
    }

    pub(crate) fn insert(&mut self, text: &str) {
        self.replace(self.cursor..self.cursor, text);
    }

    pub(crate) fn append(&mut self, text: &str) {
        let end = self.text.len();
        self.replace(end..end, text);
    }

    pub(crate) fn insert_atom(&mut self, text: &str) {
        assert!(!text.is_empty() && !text.contains(['\r', '\n']));
        let start = self.cursor;
        self.insert(text);
        let end = start + text.len();
        let index = self.atoms.partition_point(|atom| atom.start < start);
        self.atoms.insert(index, start..end);
        self.cursor = end;
    }

    pub(crate) fn atom_count(&self) -> usize {
        self.atoms.len()
    }

    pub(crate) fn atom_range(&self, text: &str) -> Option<Range<usize>> {
        self.atoms
            .iter()
            .find(|atom| &self.text[(*atom).clone()] == text)
            .cloned()
    }

    pub(crate) fn retains_atom(&self, text: &str) -> bool {
        self.atom_range(text).is_some()
            || self.recall.as_ref().is_some_and(|recall| {
                recall
                    .atoms
                    .iter()
                    .any(|atom| &recall.text[atom.clone()] == text)
            })
    }

    pub(crate) fn replace(&mut self, mut range: Range<usize>, text: &str) {
        let text = normalized(text);
        for atom in &self.atoms {
            if atom.start < range.end && range.start < atom.end {
                range.start = range.start.min(atom.start);
                range.end = range.end.max(atom.end);
            }
        }
        let end = range.start + text.len();
        self.atoms.retain_mut(|atom| {
            if atom.end <= range.start {
                true
            } else if atom.start >= range.end {
                atom.start = end + (atom.start - range.end);
                atom.end = end + (atom.end - range.end);
                true
            } else {
                false
            }
        });
        self.text.replace_range(range, &text);
        self.cursor = self.boundary_at_or_after(end);
        self.reset_motion();
    }

    pub(crate) fn completion_range(&self) -> Range<usize> {
        let boundary = self
            .atoms
            .iter()
            .rev()
            .find(|atom| atom.end <= self.cursor)
            .map_or(0, |atom| atom.end);
        let start = self.text[boundary..self.cursor]
            .rfind(char::is_whitespace)
            .map_or(boundary, |offset| {
                let offset = boundary + offset;
                offset
                    + self.text[offset..]
                        .chars()
                        .next()
                        .expect("whitespace")
                        .len_utf8()
            });
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

    pub(crate) fn vertical(&mut self, width: usize, direction: isize) -> bool {
        let rows = visual_rows(&self.text, &self.atoms, width.max(1));
        let current = row_index(&rows, self.cursor);
        let Some(destination) = current
            .checked_add_signed(direction)
            .filter(|row| *row < rows.len())
        else {
            return false;
        };
        let row = &rows[current];
        let preferred = self
            .preferred_column
            .get_or_insert_with(|| columns(&self.text, &self.atoms, row.start..self.cursor));
        self.cursor = offset_at_column(&self.text, &self.atoms, &rows, destination, *preferred);
        true
    }

    pub(crate) fn place_cursor(&mut self, width: usize, column: usize, visible_row: usize) {
        let rows = visual_rows(&self.text, &self.atoms, width.max(1));
        let row = (self.top + visible_row).min(rows.len() - 1);
        self.cursor = offset_at_column(&self.text, &self.atoms, &rows, row, column);
        self.preferred_column = None;
    }

    pub(crate) fn remember(&mut self, prompt: &str) {
        if prompt.len() <= MAX_SAVED_PROMPT_BYTES
            && self.history.front().is_none_or(|old| old.text != prompt)
        {
            self.remember_draft(kuku_client::MessageDraft {
                text: prompt.into(),
                images: Vec::new(),
            });
        }
    }

    pub(crate) fn remember_draft(&mut self, draft: kuku_client::MessageDraft) {
        self.history.retain(|previous| previous.text != draft.text);
        self.history.push_front(draft);
        let mut bytes = 0;
        let keep = self
            .history
            .iter()
            .take(MAX_HISTORY)
            .take_while(|draft| {
                bytes += draft
                    .images
                    .iter()
                    .map(|image| image.data.len())
                    .sum::<usize>();
                bytes <= MAX_HISTORY_IMAGE_BYTES
            })
            .count();
        self.history.truncate(keep);
        self.recall = None;
    }

    pub(crate) fn recalled_images(&self) -> Option<&[kuku_client::ImageAttachment]> {
        self.recall
            .as_ref()
            .map(|recall| self.history[recall.index].images.as_slice())
    }

    fn load_history(&mut self, index: usize) {
        let draft = &self.history[index];
        self.text.clone_from(&draft.text);
        self.atoms.clear();
        for image in &draft.images {
            let marker = &image.marker;
            if let Some(range) = marker.range_in(&draft.text) {
                self.atoms.push(range);
            } else if marker.offset == draft.text.len() {
                if !self.text.is_empty() {
                    self.text.push(' ');
                }
                let start = self.text.len();
                self.text.push_str(&marker.label);
                self.atoms.push(start..self.text.len());
            }
        }
        self.atoms.sort_unstable_by_key(|range| range.start);
        self.cursor = self.text.len();
    }

    pub(crate) fn recall(&mut self, direction: isize) {
        match (direction.is_negative(), self.recall.take()) {
            (true, None) if !self.history.is_empty() => {
                self.recall = Some(Recall {
                    index: 0,
                    text: std::mem::take(&mut self.text),
                    cursor: self.cursor,
                    atoms: std::mem::take(&mut self.atoms),
                });
                self.load_history(0);
            }
            (true, Some(mut recall)) => {
                recall.index = (recall.index + 1).min(self.history.len() - 1);
                self.load_history(recall.index);
                self.recall = Some(recall);
            }
            (false, Some(recall)) if recall.index == 0 => {
                self.text = recall.text;
                self.cursor = recall.cursor;
                self.atoms = recall.atoms;
            }
            (false, Some(mut recall)) => {
                recall.index -= 1;
                self.load_history(recall.index);
                self.recall = Some(recall);
            }
            (_, None) => {}
        }
        self.preferred_column = None;
    }

    pub(crate) fn project(&mut self, width: usize, height: usize) -> EditorView {
        let width = width.max(1);
        let height = height.max(1);
        let rows = visual_rows(&self.text, &self.atoms, width);
        let cursor_row = row_index(&rows, self.cursor);
        self.top = self.top.min(rows.len().saturating_sub(height));
        self.top = self
            .top
            .max(cursor_row.saturating_sub(height - 1))
            .min(cursor_row);
        let mut lines = Vec::new();
        let mut atoms = Vec::new();
        for range in rows.iter().skip(self.top).take(height) {
            let text = replace_tabs(&terminal_text(&self.text[range.clone()]));
            let row = lines.len();
            for atom in self
                .atoms
                .iter()
                .filter(|atom| range.start <= atom.start && atom.end <= range.end)
            {
                let start = columns(&self.text, &self.atoms, range.start..atom.start);
                let end = start + self.text[atom.clone()].width();
                atoms.push((row, start..end.min(width)));
            }
            let text = if text.width() <= width {
                text
            } else if self.atoms.contains(range) {
                crate::text::shorten(&text, width)
            } else {
                "�".to_owned()
            };
            lines.push(text);
        }
        let column = columns(&self.text, &self.atoms, rows[cursor_row].start..self.cursor);
        EditorView {
            lines,
            cursor: (column.min(width - 1), cursor_row - self.top),
            atoms,
        }
    }

    fn previous_boundary(&self) -> usize {
        let preceding = self
            .atoms
            .iter()
            .rev()
            .find(|atom| atom.start < self.cursor);
        if let Some(atom) = preceding
            && self.cursor <= atom.end
        {
            return atom.start;
        }
        let start = preceding.map_or(0, |atom| atom.end);
        self.text[start..self.cursor]
            .grapheme_indices(true)
            .next_back()
            .map_or(start, |(index, _)| start + index)
    }

    fn next_boundary(&self) -> usize {
        let next = self.atoms.iter().find(|atom| self.cursor < atom.end);
        if let Some(atom) = next
            && atom.start <= self.cursor
        {
            return atom.end;
        }
        let end = next.map_or(self.text.len(), |atom| atom.start);
        self.cursor
            + self.text[self.cursor..end]
                .graphemes(true)
                .next()
                .map_or(0, str::len)
    }

    fn word_start(&self) -> usize {
        let offset = self.text[..self.cursor]
            .unicode_word_indices()
            .next_back()
            .map_or(0, |(index, _)| index);
        self.atoms
            .iter()
            .find(|atom| atom.contains(&offset))
            .map_or(offset, |atom| atom.start)
    }

    fn boundary_at_or_after(&self, offset: usize) -> usize {
        let mut start = 0;
        let mut end = self.text.len();
        for atom in &self.atoms {
            if offset < atom.start {
                end = atom.start;
                break;
            }
            if offset == atom.start {
                return offset;
            }
            if offset <= atom.end {
                return atom.end;
            }
            start = atom.end;
        }
        let text = &self.text[start..end];
        let mut cursor = GraphemeCursor::new(offset - start, text.len(), true);
        if cursor.is_boundary(text, 0).expect("complete text") {
            offset
        } else {
            start
                + cursor
                    .next_boundary(text, 0)
                    .expect("complete text")
                    .unwrap_or(text.len())
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
    pub(crate) atoms: Vec<(usize, Range<usize>)>,
}

fn visual_rows(text: &str, atoms: &[Range<usize>], width: usize) -> Vec<Range<usize>> {
    let mut rows = Vec::new();
    let mut start = 0;
    let mut used = 0;
    for (offset, unit) in display_units(text, atoms, 0..text.len()) {
        if unit == "\n" {
            rows.push(start..offset);
            start = offset + 1;
            used = 0;
            continue;
        }
        let cells = unit_columns(unit).min(width);
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

fn offset_at_column(
    text: &str,
    atoms: &[Range<usize>],
    rows: &[Range<usize>],
    row: usize,
    column: usize,
) -> usize {
    let range = &rows[row];
    let mut used = 0;
    let mut last = range.start;
    for (offset, unit) in display_units(text, atoms, range.clone()) {
        last = offset;
        used += unit_columns(unit);
        if used > column {
            return last;
        }
    }
    if rows
        .get(row + 1)
        .is_some_and(|next| next.start == range.end)
    {
        last
    } else {
        range.end
    }
}

fn columns(text: &str, atoms: &[Range<usize>], range: Range<usize>) -> usize {
    display_units(text, atoms, range)
        .map(|(_, unit)| unit_columns(unit))
        .sum()
}

fn display_units<'a>(
    text: &'a str,
    atoms: &'a [Range<usize>],
    range: Range<usize>,
) -> impl Iterator<Item = (usize, &'a str)> {
    let mut start = range.start;
    atoms
        .iter()
        .filter(move |atom| range.start <= atom.start && atom.end <= range.end)
        .map(Some)
        .chain(std::iter::once(None))
        .flat_map(move |atom| {
            let plain_start = start;
            let plain_end = atom.map_or(range.end, |atom| atom.start);
            start = atom.map_or(range.end, |atom| atom.end);
            text[plain_start..plain_end]
                .grapheme_indices(true)
                .map(move |(offset, grapheme)| (plain_start + offset, grapheme))
                .chain(atom.map(|atom| (atom.start, &text[atom.clone()])))
        })
}

fn unit_columns(unit: &str) -> usize {
    if unit == "\t" {
        TAB.len()
    } else {
        unit.width().max(1)
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
    fn atom_wraps_as_a_unit_and_mouse_placement_skips_its_interior() {
        let mut editor = Editor::default();
        editor.insert("甲 ");
        editor.insert_atom("[Image #1]");
        editor.insert(" word");

        let view = editor.project(10, 3);
        assert_eq!(["甲 ", "[Image #1]", " word"], view.lines.as_slice());
        assert_eq!(vec![(1, 0..10)], view.atoms);

        editor.place_cursor(10, 6, 1);
        assert_eq!("甲 ".len(), editor.cursor());
        editor.delete();
        assert_eq!("甲  word", editor.text());
        assert_eq!(0, editor.atom_count());
    }

    #[test]
    fn unsent_atoms_survive_prompt_history_recall() {
        let mut editor = Editor::default();
        editor.insert_atom("[Image #1]");
        editor.remember("[Image #1]");

        editor.recall(-1);
        assert!(editor.atom_range("[Image #1]").is_none());
        assert!(editor.retains_atom("[Image #1]"));

        editor.recall(1);
        assert_eq!(Some(0..10), editor.atom_range("[Image #1]"));
        editor.backspace();
        assert!(editor.text().is_empty());
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
