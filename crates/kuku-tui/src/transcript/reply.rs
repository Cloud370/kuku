use kuku_client::{HistoryEntry, HistoryKind, HistoryPage};

use super::{Body, Key, Record, MAX_BODY_BYTES};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ReplySource<'a> {
    Local(&'a str),
    History(u64),
}

#[derive(Debug, Default)]
pub(super) struct LatestReply(Option<Reply>);

impl LatestReply {
    pub(super) fn observe_page(&mut self, page: &HistoryPage) {
        self.remember(page.entries.iter().rev().find_map(Reply::from_entry));
    }

    pub(super) fn observe(&mut self, entry: &HistoryEntry) {
        self.remember(Reply::from_entry(entry));
    }

    pub(super) fn source<'a>(&self, records: &'a [Record]) -> Option<ReplySource<'a>> {
        let reply = self.0.as_ref()?;
        if !reply.clipped {
            let key = Key::Event(reply.id);
            let text = records.iter().rev().find_map(|record| match &record.body {
                Body::Assistant(text) if record.key == key => Some(text.as_str()),
                _ => None,
            });
            if let Some(text) = text {
                return Some(ReplySource::Local(text));
            }
        }
        Some(ReplySource::History(reply.id))
    }

    fn remember(&mut self, reply: Option<Reply>) {
        if let Some(reply) =
            reply.filter(|reply| self.0.as_ref().is_none_or(|latest| reply.id >= latest.id))
        {
            self.0 = Some(reply);
        }
    }
}

#[derive(Debug)]
struct Reply {
    id: u64,
    clipped: bool,
}

impl Reply {
    fn from_entry(entry: &HistoryEntry) -> Option<Self> {
        let text = entry.text.as_deref()?;
        if entry.kind != HistoryKind::Assistant || text.trim().is_empty() {
            return None;
        }
        Some(Self {
            id: entry.id,
            clipped: text.len() > MAX_BODY_BYTES,
        })
    }
}
