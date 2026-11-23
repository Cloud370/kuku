//! Conversation address validation and helpers.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

const MAX_ADDRESS_BYTES: usize = 64;

/// A validated flat address for an Agent within a Session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct ConversationAddress(Cow<'static, str>);

impl ConversationAddress {
    pub const MAIN: Self = Self(Cow::Borrowed("main"));

    /// Parse and validate a conversation address.
    pub fn parse(value: &str) -> Result<Self, String> {
        validate_address(value)?;
        Ok(Self(Cow::Owned(value.to_owned())))
    }

    /// Returns true when this is the root host conversation.
    pub fn is_main(&self) -> bool {
        self.as_str() == Self::MAIN.as_str()
    }

    /// Returns the validated address string.
    pub fn as_str(&self) -> &str {
        self.0.as_ref()
    }
}

impl TryFrom<String> for ConversationAddress {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        validate_address(&value)?;
        Ok(Self(Cow::Owned(value)))
    }
}

pub(crate) fn validate_address(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > MAX_ADDRESS_BYTES
        || !value
            .bytes()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_'))
    {
        return Err(format!(
            "agent address '{value}' must be flat and contain 1..=64 lowercase ASCII letters, digits, '-' or '_'"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ConversationAddress;

    #[test]
    fn validates_flat_session_agents() {
        for valid in ["main", "review", "explore-auth", "worker_2"] {
            assert_eq!(ConversationAddress::parse(valid).unwrap().as_str(), valid);
        }
        let long = "a".repeat(65);
        for invalid in [
            "",
            "review/api",
            "Main",
            "/agent",
            "two words",
            long.as_str(),
        ] {
            assert!(ConversationAddress::parse(invalid).is_err(), "{invalid}");
        }
    }
}
