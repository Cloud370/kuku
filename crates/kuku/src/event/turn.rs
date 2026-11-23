use serde::{Deserialize, Serialize};

pub(crate) const REPLY_PREFIX_BYTES: usize = 64 * 1024;

/// Visible reply prefix retained when a provider stream fails.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartialReply {
    pub text: String,
    pub truncated: bool,
}

impl PartialReply {
    pub(crate) fn from_text(text: &str) -> Option<Self> {
        if text.is_empty() {
            return None;
        }
        let end = text.floor_char_boundary(REPLY_PREFIX_BYTES.min(text.len()));
        Some(Self {
            text: text[..end].to_owned(),
            truncated: end < text.len(),
        })
    }
}

pub(super) fn encode_interruption(
    id: u64,
    ts: &str,
    conversation: &str,
    turn: u64,
    reason: &str,
    partial_reply: Option<&PartialReply>,
) -> serde_json::Result<serde_json::Value> {
    let mut value = serde_json::json!({
        "id": id, "ts": ts, "kind": "turn.interrupted",
        "conversation": conversation, "turn": turn, "reason": reason,
    });
    if let Some(reply) = partial_reply {
        value["partial_reply"] = serde_json::to_value(reply)?;
    }
    Ok(value)
}
