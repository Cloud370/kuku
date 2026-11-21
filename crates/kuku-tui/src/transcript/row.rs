use ratatui::text::Line;

use crate::markup::LinkDestination;
use crate::theme::CardFrame;

#[derive(Debug, Clone)]
pub(super) struct Row {
    pub(super) line: Line<'static>,
    pub(super) inset: u16,
    pub(super) links: Vec<LinkDestination>,
}

impl Row {
    pub(super) fn padding(line: Line<'static>, width: usize) -> Self {
        Self {
            line,
            inset: width as u16,
            links: Vec::new(),
        }
    }

    pub(super) fn frame_padding(frame: &CardFrame) -> Option<Self> {
        frame.padding_line().map(|line| {
            let width = line.width();
            Self::padding(line, width)
        })
    }

    pub(super) fn pad_framed_body(rows: &mut Vec<Self>, frame: &CardFrame) {
        if rows.len() > 1 {
            if let Some(padding) = Self::frame_padding(frame) {
                rows.insert(1, padding.clone());
                rows.push(padding);
            }
        }
    }

    pub(super) fn framed(mut self, frame: &CardFrame) -> Self {
        let inset = frame.content_inset();
        self.line = frame.line(self.line);
        self.inset += inset as u16;
        for link in &mut self.links {
            link.columns = link.columns.start + inset..link.columns.end + inset;
        }
        self
    }

    pub(super) fn frame_end(frame: &CardFrame) -> Vec<Self> {
        let mut rows: Vec<_> = Self::frame_padding(frame).into_iter().collect();
        if frame.bordered() {
            let line = frame.border('╰', '╯');
            let width = line.width();
            rows.push(Self::padding(line, width));
        }
        rows.push(Self::plain(Line::default()));
        rows
    }

    pub(super) fn plain(line: Line<'static>) -> Self {
        Self {
            line,
            inset: 0,
            links: Vec::new(),
        }
    }
}
