use serde_json::Value;

use crate::config::ProviderFormat;
use crate::context::{CanonicalMessage, MessageBlock};

use super::prepare::invalid_response;
use super::types::ProviderFailure;

const MAX_REPLAY_BYTES: usize = 512 * 1024;
pub(crate) const MAX_REPLAY_ITEMS: usize = 1024;

#[derive(Default)]
pub(crate) struct ReplayBudget {
    bytes: usize,
}

impl ReplayBudget {
    pub(crate) fn add(&mut self, value: &Value) -> Result<(), ProviderFailure> {
        let bytes = serde_json::to_vec(value)
            .map_err(|_| invalid_response("could not encode native replay data"))?
            .len();
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or_else(|| invalid_response("native replay data exceeds the byte limit"))?;
        if self.bytes > MAX_REPLAY_BYTES {
            return Err(invalid_response(
                "native replay data exceeds the byte limit",
            ));
        }
        Ok(())
    }
}

pub(crate) fn validate_replay_size(value: &Value) -> Result<(), ProviderFailure> {
    ReplayBudget::default().add(value)
}

pub(crate) fn native_replay(
    message: &CanonicalMessage,
    protocol: ProviderFormat,
) -> Option<&Value> {
    message.blocks.iter().find_map(|block| match block {
        MessageBlock::ProviderReplay {
            protocol: source,
            value,
        } if *source == protocol => Some(value),
        MessageBlock::ProviderReplay { .. }
        | MessageBlock::Text(_)
        | MessageBlock::Thinking(_)
        | MessageBlock::ToolUse(_)
        | MessageBlock::InputImage(_)
        | MessageBlock::ToolResult(_) => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaped_native_fragments_share_one_bounded_budget() {
        let mut budget = ReplayBudget::default();
        let escaped = Value::String("\n".repeat(MAX_REPLAY_BYTES / 8));
        budget.add(&escaped).unwrap();
        budget.add(&escaped).unwrap();
        budget.add(&escaped).unwrap();
        assert!(budget.add(&escaped).is_err());
        assert!(validate_replay_size(&Value::String("x".repeat(MAX_REPLAY_BYTES))).is_err());
    }
}
