use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
/// Speaker role in a conversation message.
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq)]
/// A single conversation message with a role and one or more content blocks.
pub struct CanonicalMessage {
    pub role: Role,
    pub blocks: Vec<MessageBlock>,
}

impl CanonicalMessage {
    /// Create a user message containing a single text block.
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            blocks: vec![MessageBlock::Text(text.into())],
        }
    }

    /// Create an assistant message from the given content blocks.
    pub fn assistant(blocks: Vec<MessageBlock>) -> Self {
        Self {
            role: Role::Assistant,
            blocks,
        }
    }

    /// Create a user message from the given content blocks.
    pub fn user(blocks: Vec<MessageBlock>) -> Self {
        Self {
            role: Role::User,
            blocks,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
/// A single content block within a conversation message.
pub enum MessageBlock {
    Text(String),
    Thinking(String),
    ToolUse(ToolUse),
    ToolResult(ToolResult),
}

#[derive(Debug, Clone, PartialEq)]
/// A tool invocation requested by the model.
pub struct ToolUse {
    pub id: String,
    pub name: String,
    pub args: Value,
}

#[derive(Debug, Clone, PartialEq)]
/// The outcome of a tool invocation.
pub struct ToolResult {
    pub tool_call_id: String,
    pub status: String,
    pub summary: String,
    pub model_content: String,
    pub structured: Option<Value>,
    pub truncated: bool,
}
