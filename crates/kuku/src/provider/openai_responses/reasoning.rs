use std::collections::BTreeMap;

use serde_json::Value;

use super::super::chunk::ProviderChunk;
use super::super::replay::MAX_REPLAY_ITEMS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReasoningText {
    Text,
    Summary,
}

impl ReasoningText {
    pub(super) fn part_index(self, event: &Value) -> u64 {
        let field = match self {
            Self::Text => "content_index",
            Self::Summary => "summary_index",
        };
        event[field].as_u64().unwrap_or_default()
    }

    fn parts(self, item: &Value) -> impl Iterator<Item = (u64, &str)> {
        let (field, part_type) = match self {
            Self::Text => ("content", "reasoning_text"),
            Self::Summary => ("summary", "summary_text"),
        };
        item[field]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .filter_map(move |(index, part)| {
                if part["type"] != part_type {
                    return None;
                }
                part["text"]
                    .as_str()
                    .filter(|text| !text.trim().is_empty())
                    .map(|text| (index as u64, text))
            })
    }
}

#[derive(Default)]
pub(super) struct PublishedReasoning {
    items: BTreeMap<u64, Option<ReasoningText>>,
    lengths: BTreeMap<(u64, u64), usize>,
}

impl PublishedReasoning {
    pub(super) fn announce(&mut self, index: u64, chunks: &mut Vec<ProviderChunk>) {
        if self.items.len() < MAX_REPLAY_ITEMS && !self.items.contains_key(&index) {
            self.items.insert(index, None);
            chunks.push(ProviderChunk::Reasoning);
        }
    }

    pub(super) fn item(&mut self, index: u64, item: &Value, chunks: &mut Vec<ProviderChunk>) {
        if item["type"] != "reasoning" {
            return;
        }
        let kind = self
            .items
            .get(&index)
            .copied()
            .flatten()
            .unwrap_or_else(|| {
                if ReasoningText::Summary.parts(item).next().is_some() {
                    ReasoningText::Summary
                } else {
                    ReasoningText::Text
                }
            });
        for (part, text) in kind.parts(item) {
            self.publish(index, part, kind, text, true, chunks);
        }
        self.announce(index, chunks);
    }

    pub(super) fn publish(
        &mut self,
        item: u64,
        part: u64,
        kind: ReasoningText,
        text: &str,
        complete: bool,
        chunks: &mut Vec<ProviderChunk>,
    ) {
        if text.is_empty()
            || !self.items.contains_key(&item) && self.items.len() >= MAX_REPLAY_ITEMS
            || self
                .items
                .get(&item)
                .is_some_and(|selected| selected.is_some_and(|selected| selected != kind))
        {
            return;
        }
        let key = (item, part);
        let is_new = !self.lengths.contains_key(&key);
        if is_new && self.lengths.len() >= MAX_REPLAY_ITEMS {
            return;
        }
        let separate = is_new && !self.lengths.is_empty();
        let length = self.lengths.entry(key).or_default();
        let delta = if complete {
            text.get(*length..).unwrap_or_default()
        } else {
            text
        };
        if !delta.is_empty() {
            *length += delta.len();
            self.items.insert(item, Some(kind));
            let text = if separate {
                format!("\n\n{delta}")
            } else {
                delta.into()
            };
            chunks.push(ProviderChunk::ThinkingDelta { text });
        }
    }
}
