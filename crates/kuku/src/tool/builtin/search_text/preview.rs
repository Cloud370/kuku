use std::io;

use super::{trim_search_line, SearchMatch};
use crate::tool::builtin::common::{ensure_active, FileCancellation};

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

    pub(super) fn extend(
        &mut self,
        path: &str,
        content: &str,
        matches: &[SearchMatch],
        first_match: usize,
        context: usize,
        cancelled: &FileCancellation<'_>,
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
        for (index, line) in content.lines().enumerate().take(end) {
            ensure_active(cancelled)?;
            let number = index + 1;
            if number < start {
                continue;
            }
            if upcoming.peek() == Some(&number) {
                preceding = upcoming.next();
                before_match += 1;
                continue;
            }
            let close = upcoming.peek().is_some_and(|next| next - number <= context)
                || preceding.is_some_and(|prior| number - prior <= context);
            if !close {
                continue;
            }
            let nearby = trim_search_line(line, usize::MAX);
            let text = format!("{path}:{number}- {nearby}");
            if text.len() + 1 > self.remaining {
                self.truncated = true;
                break;
            }
            self.remaining -= text.len() + 1;
            self.rows.push(ContextRow { before_match, text });
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
