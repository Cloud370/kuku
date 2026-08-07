use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};

/// Canonical terminal state reported by a model provider.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ModelStopReason {
    /// The model completed a response without pending tools.
    EndTurn,
    /// The model completed one or more tool calls.
    ToolUse,
    /// The model reached its configured output limit.
    Length,
    /// The provider filtered the response for safety.
    ContentFilter,
    /// The provider reported an incomplete response without a usable limit reason.
    Incomplete,
    /// The provider's terminal response was structurally inconsistent.
    InvalidResponse,
    /// A provider-specific terminal reason that Kuku does not recognize.
    Unknown(String),
}

impl ModelStopReason {
    /// Parses a provider or persisted stop-reason wire value.
    pub fn from_wire(reason: &str) -> Option<Self> {
        match reason {
            "stop" | "end_turn" => Some(Self::EndTurn),
            "tool_calls" | "tool_use" | "function_call" => Some(Self::ToolUse),
            "length" | "max_tokens" | "max_output_tokens" => Some(Self::Length),
            "content_filter" | "content-filter" | "refusal" | "safety" => {
                Some(Self::ContentFilter)
            }
            "incomplete" => Some(Self::Incomplete),
            "invalid_response" => Some(Self::InvalidResponse),
            "" => None,
            other => Some(Self::Unknown(other.to_string())),
        }
    }

    pub(crate) fn wire_value(&self) -> &str {
        match self {
            Self::EndTurn => "end_turn",
            Self::ToolUse => "tool_use",
            Self::Length => "length",
            Self::ContentFilter => "content_filter",
            Self::Incomplete => "incomplete",
            Self::InvalidResponse => "invalid_response",
            Self::Unknown(reason) => reason,
        }
    }
}

impl Serialize for ModelStopReason {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.wire_value())
    }
}

impl<'de> Deserialize<'de> for ModelStopReason {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let reason = String::deserialize(deserializer)?;
        Self::from_wire(&reason)
            .ok_or_else(|| de::Error::custom("model stop reason must not be empty"))
    }
}
