use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::markup::{render_literal, wrap};
use crate::text::shorten;
use crate::theme::{emphasis, MUTED, SURFACE_TEXT};

use super::{DetailKey, Transcript, TranscriptView};

#[derive(Debug)]
pub(super) struct ThinkingRead {
    pub(super) key: DetailKey,
    text: String,
    rows: Vec<Line<'static>>,
    width: usize,
    origin: usize,
    pub(super) header: usize,
    page: usize,
    top: usize,
    was_following: bool,
}

impl ThinkingRead {
    pub(super) fn paint(&mut self, view: &mut TranscriptView, width: usize, height: usize) {
        let header = self
            .origin
            .min(height.saturating_sub(2))
            .min(view.lines.len());
        self.header = header;
        self.page = height.saturating_sub(header + 1);
        if self.width != width {
            self.width = width;
            self.rows = wrap(render_literal(&self.text, Default::default()), width.max(1));
        }
        self.top = self.top.min(self.rows.len().saturating_sub(self.page));
        view.lines.truncate(header);
        view.row_insets.truncate(header);
        if header < height {
            let (label, hint) = if width >= 48 {
                ("Assistant   Thinking", "   ·   ↑↓ read   ·   Esc back")
            } else {
                ("Thinking", " · Esc back")
            };
            let mut title = Line::from(Span::styled(shorten(label, width), emphasis(SURFACE_TEXT)));
            title.push_span(Span::styled(
                shorten(hint, width.saturating_sub(title.width())),
                Style::new().fg(MUTED),
            ));
            view.lines.push(title);
            view.row_insets.push(0);
        }
        view.lines
            .extend(self.rows.iter().skip(self.top).take(self.page).cloned());
        view.row_insets.resize(view.lines.len(), 0);
        view.thinking_row = None;
        view.tool_hint = None;
    }

    fn scroll(&mut self, amount: isize) {
        self.top = self
            .top
            .saturating_add_signed(amount)
            .min(self.rows.len().saturating_sub(self.page));
    }
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
            width: usize::MAX,
            origin,
            header: origin,
            page: 1,
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

    pub(crate) fn scroll_notes(&mut self, amount: isize) -> bool {
        if let Some(notes) = &mut self.notes {
            notes.scroll(amount);
            true
        } else {
            false
        }
    }

    pub(crate) fn notes_page(&self) -> Option<usize> {
        self.notes.as_ref().map(|n| n.page.max(1))
    }
}
