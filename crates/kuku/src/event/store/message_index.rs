use std::borrow::Cow;

use rpds::{HashTrieMapSync, VectorSync};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A user message that can be reopened as a draft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserMessagePreview {
    /// Exact committed message event identity.
    pub event_id: u64,
    /// Bounded message preview.
    pub text: String,
}

pub(crate) type MessagePreviews = HashTrieMapSync<u64, VectorSync<UserMessagePreview>>;

#[derive(Default)]
pub(super) struct MessageIndex {
    ordered: Vec<IndexedMessage>,
    pub(super) previews: MessagePreviews,
}

struct IndexedMessage {
    id: u64,
    turn: u64,
    preview_index: Option<usize>,
}

#[derive(Serialize, Deserialize)]
struct SavedMessage<'a> {
    id: u64,
    turn: u64,
    preview: Option<Cow<'a, str>>,
}

impl MessageIndex {
    pub(super) fn push(&mut self, id: u64, turn: u64, preview: Option<String>) {
        let preview_index = preview.map(|text| {
            let preview = UserMessagePreview { event_id: id, text };
            if let Some(turn_previews) = self.previews.get_mut(&turn) {
                let index = turn_previews.len();
                turn_previews.push_back_mut(preview);
                index
            } else {
                self.previews
                    .insert_mut(turn, VectorSync::new_sync().push_back(preview));
                0
            }
        });
        self.ordered.push(IndexedMessage {
            id,
            turn,
            preview_index,
        });
    }

    pub(super) fn contains(&self, id: u64) -> bool {
        self.ordered
            .binary_search_by_key(&id, |message| message.id)
            .is_ok()
    }

    pub(super) fn latest_before(&self, id: u64) -> u64 {
        let end = self.ordered.partition_point(|message| message.id < id);
        self.ordered[..end].last().map_or(0, |message| message.id)
    }
}

impl Serialize for MessageIndex {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.ordered.iter().map(|message| {
            SavedMessage {
                id: message.id,
                turn: message.turn,
                preview: message
                    .preview_index
                    .map(|index| Cow::Borrowed(self.previews[&message.turn][index].text.as_str())),
            }
        }))
    }
}

impl<'de> Deserialize<'de> for MessageIndex {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let messages = Vec::<SavedMessage<'_>>::deserialize(deserializer)?;
        let mut index = Self::default();
        index.ordered.reserve(messages.len());
        for message in messages {
            index.push(
                message.id,
                message.turn,
                message.preview.map(Cow::into_owned),
            );
        }
        Ok(index)
    }
}
