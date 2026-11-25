use kuku_client::{ForwardHistoryPage, HistoryPage};

use super::{
    Anchor, AnchorLine, Block, HISTORY_PAGE, HistoryRequest, MAX_RECORDS, Reading, Transcript,
    Window,
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
                .is_some_and(|(expected, _, _)| *expected == before)
            {
                let (_, anchor, activity) = self.restore.take().expect("restoration requested");
                let mut window = Window::from_page(page);
                window.freeze_activity(activity);
                if window.contains(&anchor.key) {
                    let newer = window.last_position().filter(|after| {
                        self.recent
                            .last_position()
                            .is_none_or(|recent| recent > *after)
                    });
                    self.reading = Some(Reading {
                        window,
                        anchor,
                        newer,
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
        if matches!(request, HistoryRequest::Older(before) if reading.window.older == Some(before))
        {
            reading.rebase_after_page = true;
            reading.window.older = page.next_before;
            reading.window.merge(page);
            if let Some(after) = reading
                .window
                .trim_back(reading.visible_through, &mut self.detail_items)
            {
                reading.newer = Some(after);
            }
        }
    }

    pub(crate) fn apply_forward_page(&mut self, request: HistoryRequest, page: ForwardHistoryPage) {
        let Some(reading) = &mut self.reading else {
            return;
        };
        if reading.newer.map(HistoryRequest::Newer) != Some(request) {
            return;
        }
        reading.rebase_after_page = true;
        reading.newer = page.next_after;
        reading.window.merge(page.page);
        let limit = reading_window_limit(reading);
        reading.window.trim_front_to(limit, &mut self.detail_items);
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
    pub(super) fn trim_back(
        &mut self,
        visible_through: Option<u64>,
        items: &mut super::Details,
    ) -> Option<u64> {
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
        self.preserve_group_identities(items, |record| {
            record.position.is_none_or(|position| position < first)
        });
        self.records
            .retain(|record| record.position.is_none_or(|position| position < first));
        self.rewind_groups.retain(|group| group.position() < first);
        self.reconcile_retry_errors();
        Some(
            positions[..keep]
                .iter()
                .rev()
                .find(|(id, _)| *id < first)
                .map_or_else(|| first.saturating_sub(1), |(id, _)| *id),
        )
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
        let block = &blocks[index];
        let end = match anchor.line {
            AnchorLine::Group(_) if block.grouped && block.group.is_none() => {
                let Some(member) = blocks[index..]
                    .iter()
                    .take_while(|block| block.grouped && block.group.is_none())
                    .position(|block| block.body_height() > 0)
                else {
                    return;
                };
                index + member
            }
            AnchorLine::Body(_) | AnchorLine::Note(_) if block.collapsed => index + 1,
            _ => return,
        };
        let row = blocks[..end].iter().map(Self::height).sum();
        if let Some(next) = Self::anchor_at(blocks, row) {
            *anchor = next;
        }
    }
}
