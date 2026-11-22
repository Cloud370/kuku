use std::collections::VecDeque;

use kuku_client::HistoryPage;

use super::{
    Anchor, AnchorLine, Block, HistoryRequest, Reading, Record, Transcript, Window, HISTORY_PAGE,
    MAX_BACKTRACK_PAGES, MAX_RECORDS,
};

const MAX_READING_RECORDS: usize = MAX_RECORDS + 2 * HISTORY_PAGE;

impl Transcript {
    pub(crate) fn apply_page(&mut self, request: HistoryRequest, page: HistoryPage) {
        if request == HistoryRequest::Recent {
            self.replace_recent(page);
            return;
        }
        if let HistoryRequest::Restore(before) = request {
            if self
                .restore
                .as_ref()
                .is_some_and(|(expected, _)| *expected == before)
            {
                let (_, anchor) = self.restore.take().expect("restoration requested");
                let window = Window::from_page(page);
                if window.contains(&anchor.key) {
                    self.reading = Some(Reading {
                        window,
                        anchor,
                        newer: VecDeque::new(),
                        rebase_after_page: false,
                        visible_through: None,
                        unseen: false,
                        manual: true,
                    });
                }
            }
            return;
        }
        let Some(reading) = &mut self.reading else {
            return;
        };
        match request {
            HistoryRequest::Older(before) if reading.window.older == Some(before) => {
                reading.rebase_after_page = true;
                reading.window.older = page.next_before;
                for record in page.entries.into_iter().filter_map(Record::from_entry) {
                    reading.window.insert(record);
                }
                reading.window.add_rewind_groups(page.rewind_groups);
                if let Some(request) = reading.window.trim_back(reading.visible_through) {
                    reading.newer.push_front(request);
                    reading.newer.truncate(MAX_BACKTRACK_PAGES);
                }
            }
            HistoryRequest::Newer { first, .. } if reading.newer.front() == Some(&request) => {
                reading.rebase_after_page = true;
                reading.newer.pop_front();
                for record in page
                    .entries
                    .into_iter()
                    .filter(|entry| entry.id >= first)
                    .filter_map(Record::from_entry)
                {
                    reading.window.insert(record);
                }
                reading.window.add_rewind_groups(
                    page.rewind_groups
                        .into_iter()
                        .filter(|group| group.target_event_id >= first),
                );
                let limit = reading_window_limit(reading);
                reading.window.trim_front_to(limit);
            }
            HistoryRequest::Older(_)
            | HistoryRequest::Newer { .. }
            | HistoryRequest::Restore(_)
            | HistoryRequest::Recent => {}
        }
    }
}

fn reading_window_limit(reading: &Reading) -> usize {
    let window = &reading.window;
    let Some(index) = window
        .records
        .iter()
        .position(|record| record.key == reading.anchor.key)
    else {
        return MAX_RECORDS;
    };
    let first = super::details::groups(&window.records)
        .into_iter()
        .find(|span| span.range.contains(&index))
        .map_or(index, |span| span.range.start);
    let Some(position) = window.records[first].position else {
        return MAX_RECORDS;
    };
    let keep = window
        .positions()
        .iter()
        .filter(|(event, _)| *event >= position)
        .count()
        + window
            .records
            .iter()
            .filter(|record| record.position.is_none())
            .count();
    keep.clamp(MAX_RECORDS, MAX_READING_RECORDS)
}

pub(super) fn last_visible_card(blocks: &[Block<'_>], start: usize, height: usize) -> Option<u64> {
    if height == 0 {
        return None;
    }
    let last = start + height - 1;
    let mut end = 0;
    let mut visible = None;
    for (index, block) in blocks.iter().enumerate() {
        if block.height() == 0 {
            continue;
        }
        end += block.height();
        if end > start {
            visible = Some(index);
        }
        if end > last {
            break;
        }
    }
    let index = visible?;
    let mut position = blocks[index].position;
    if blocks[index].grouped {
        for member in blocks[index + 1..]
            .iter()
            .take_while(|block| block.grouped && block.group.is_none())
        {
            position = member.position;
        }
    }
    Some(position)
}

impl Window {
    pub(super) fn trim_back(&mut self, visible_through: Option<u64>) -> Option<HistoryRequest> {
        let positions = self.positions();
        let live = self
            .records
            .iter()
            .filter(|record| record.position.is_none())
            .count();
        let visible = visible_through.map_or(0, |last| {
            positions.partition_point(|(position, _)| *position <= last)
        });
        let keep = visible.clamp(
            MAX_RECORDS.saturating_sub(live),
            MAX_READING_RECORDS.saturating_sub(live),
        );
        let first = positions.get(keep)?.0;
        let last = positions.last()?.0;
        self.records
            .retain(|record| record.position.is_none_or(|position| position < first));
        self.rewind_groups.retain(|group| group.position() < first);
        self.reconcile_retry_errors();
        Some(HistoryRequest::Newer {
            first,
            before: last.saturating_add(1),
        })
    }
}

impl Block<'_> {
    pub(super) fn rebase_anchor(blocks: &[Self], anchor: &mut Anchor) {
        let Some(index) = blocks.iter().position(|block| block.key == &anchor.key) else {
            if let Some(first) = Self::anchor_at(blocks, 0) {
                *anchor = first;
            }
            return;
        };
        if !blocks[index].collapsed
            || !matches!(anchor.line, AnchorLine::Body(_) | AnchorLine::Note(_))
        {
            return;
        }
        let after = blocks[..=index].iter().map(Self::height).sum();
        if let Some(next) = Self::anchor_at(blocks, after) {
            *anchor = next;
        }
    }
}
