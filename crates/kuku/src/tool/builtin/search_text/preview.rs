use std::io;
use std::path::Path;

use super::{FileSearch, SearchMatch, trim_search_line};
use crate::tool::builtin::common::{StableFile, WorkspaceFiles, is_cancellation};

struct ContextRow {
    before_match: usize,
    text: String,
}

pub(super) struct ContextPreview {
    rows: Vec<ContextRow>,
    remaining: usize,
    truncated: bool,
}

impl ContextPreview {
    pub(super) fn new(bytes: usize) -> Self {
        Self {
            rows: Vec::new(),
            remaining: bytes,
            truncated: false,
        }
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the window takes explicit match coordinates, content, and output bounds"
    )]
    pub(super) fn extend(
        &mut self,
        relative: &str,
        path: &Path,
        source: &WorkspaceFiles<'_>,
        input: &mut StableFile<'_>,
        reader: &mut FileSearch,
        bytes_left: &mut u64,
        matches: &[SearchMatch],
        first_match: usize,
        context: usize,
    ) -> io::Result<()> {
        if matches.is_empty() || self.truncated {
            return Ok(());
        }
        let start = matches[0].line_number.saturating_sub(context).max(1);
        let end = matches[matches.len() - 1]
            .line_number
            .saturating_add(context);
        let mut upcoming = matches.iter().map(|hit| hit.line_number).peekable();
        let mut before_match = first_match;
        let mut preceding = None;
        let previous_rows = self.rows.len();
        let remaining = self.remaining;
        input.rewind()?;
        let read = reader.scan(input, bytes_left, |number, line| {
            if number > end {
                return false;
            }
            if number < start {
                return true;
            }
            if upcoming.peek() == Some(&number) {
                preceding = upcoming.next();
                before_match += 1;
                return true;
            }
            let close = upcoming.peek().is_some_and(|next| next - number <= context)
                || preceding.is_some_and(|prior| number - prior <= context);
            if !close {
                return true;
            }
            let nearby = trim_search_line(line, usize::MAX);
            let text = format!("{relative}:{number}- {nearby}");
            if text.len() + 1 > self.remaining {
                self.truncated = true;
                return false;
            }
            self.remaining -= text.len() + 1;
            self.rows.push(ContextRow { before_match, text });
            true
        });
        if let Err(error) = read.and_then(|()| source.verify(path, input)) {
            if is_cancellation(&error) {
                return Err(error);
            }
            self.rows.truncate(previous_rows);
            self.remaining = remaining;
            self.truncated = true;
        }
        Ok(())
    }

    pub(super) fn render(self, primary: Vec<String>, mut available: usize) -> (String, bool) {
        let mut truncated = self.truncated;
        let mut context = self
            .rows
            .into_iter()
            .take_while(|row| {
                let cost = row.text.len() + 1;
                if cost > available {
                    truncated = true;
                    return false;
                }
                available -= cost;
                true
            })
            .peekable();
        let mut rows = Vec::with_capacity(primary.len());
        for (index, hit) in primary.into_iter().enumerate() {
            while context.peek().is_some_and(|row| row.before_match == index) {
                rows.push(context.next().expect("peeked context row").text);
            }
            rows.push(hit);
        }
        rows.extend(context.map(|row| row.text));
        (rows.join("\n"), truncated)
    }
}
