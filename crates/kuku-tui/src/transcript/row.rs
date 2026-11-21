use ratatui::text::Line;

use crate::markup::LinkDestination;

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

    pub(super) fn frame_padding(frame: &crate::theme::CardFrame) -> Option<Self> {
        frame.padding_line().map(|line| {
            let width = line.width();
            Self::padding(line, width)
        })
    }

    pub(super) fn pad_framed_body(rows: &mut Vec<Self>, frame: &crate::theme::CardFrame) {
        if rows.len() > 1 {
            if let Some(padding) = Self::frame_padding(frame) {
                rows.insert(1, padding.clone());
                rows.push(padding);
            }
        }
    }

    pub(super) fn plain(line: Line<'static>) -> Self {
        Self {
            line,
            inset: 0,
            links: Vec::new(),
        }
    }
}
