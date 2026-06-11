//! Conversation address validation and helpers.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

const MAX_ADDRESS_BYTES: usize = 128;

/// A validated slash-separated conversation address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

    /// Returns the top-level conversation contact.
    pub fn root_contact(&self) -> Self {
        match self.as_str().split_once('/') {
            Some((root, _)) => Self(Cow::Owned(root.to_owned())),
            None if self.is_main() => Self::MAIN,
            None => self.clone(),
        }
    }

    /// Returns the validated address string.
    pub fn as_str(&self) -> &str {
        self.0.as_ref()
    }
}

fn validate_address(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err("conversation address cannot be empty".into());
    }

    if value.len() > MAX_ADDRESS_BYTES {
        return Err("conversation address exceeds 128 bytes".into());
    }

    if value.starts_with('/') || value.ends_with('/') || value.contains("//") {
        return Err("conversation address has invalid slash placement".into());
    }

    for segment in value.split('/') {
        if segment.is_empty() {
            return Err("conversation address cannot contain empty segments".into());
        }

        if !segment
            .bytes()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_'))
        {
            return Err(format!(
                "conversation address segment is invalid: {segment}"
            ));
        }
    }

    if value != "main" && value.starts_with("main/") {
        return Err("agent conversation addresses cannot be rooted at main".into());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_addresses() {
        let cases = [
            "main",
            "review",
            "review/api",
            "explore/auth-flow",
            "a_b/c-1",
        ];

        for case in cases {
            let address = ConversationAddress::parse(case).unwrap();
            assert_eq!(case, address.as_str());
        }
    }

    #[test]
    fn rejects_invalid_addresses() {
        let over_128 = "a".repeat(129);
        let cases = [
            "",
            "/review",
            "review/",
            "review//api",
            "review api",
            "Review",
            "main/api",
            "你好",
            over_128.as_str(),
        ];

        for case in cases {
            assert!(
                ConversationAddress::parse(case).is_err(),
                "expected invalid address: {case}"
            );
        }
    }

    #[test]
    fn main_helpers_work() {
        assert!(ConversationAddress::MAIN.is_main());
        assert_eq!("main", ConversationAddress::MAIN.as_str());
        assert_eq!("main", ConversationAddress::MAIN.root_contact().as_str());
    }

    #[test]
    fn nested_addresses_report_root_contact() {
        let address = ConversationAddress::parse("review/api").unwrap();
        assert_eq!("review", address.root_contact().as_str());
        assert!(!address.is_main());
    }
}
