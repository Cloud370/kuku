use std::collections::VecDeque;

use kuku_client::HistoryPage;

use super::{HistoryRequest, Reading, Record, Transcript, Window, MAX_BACKTRACK_PAGES};

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
                reading.window.older = page.next_before;
                for record in page.entries.into_iter().filter_map(Record::from_entry) {
                    reading.window.insert(record);
                }
                if let Some(request) = reading.window.trim_back() {
                    reading.newer.push_front(request);
                    reading.newer.truncate(MAX_BACKTRACK_PAGES);
                }
            }
            HistoryRequest::Newer { first, .. } if reading.newer.front() == Some(&request) => {
                reading.newer.pop_front();
                for record in page
                    .entries
                    .into_iter()
                    .filter(|entry| entry.id >= first)
                    .filter_map(Record::from_entry)
                {
                    reading.window.insert(record);
                }
                reading.window.trim_front();
            }
            HistoryRequest::Older(_)
            | HistoryRequest::Newer { .. }
            | HistoryRequest::Restore(_)
            | HistoryRequest::Recent => {}
        }
    }
}
