use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CanonicalMessage {
    pub role: Role,
    pub blocks: Vec<MessageBlock>,
}

impl CanonicalMessage {
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            blocks: vec![MessageBlock::Text(text.into())],
        }
    }

    pub fn assistant(blocks: Vec<MessageBlock>) -> Self {
        Self {
            role: Role::Assistant,
            blocks,
        }
    }

    pub fn user(blocks: Vec<MessageBlock>) -> Self {
        Self {
            role: Role::User,
            blocks,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum MessageBlock {
    Text(String),
    Thinking(String),
    ToolUse(ToolUse),
    ToolResult(ToolResult),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolUse {
    pub id: String,
    pub name: String,
    pub args: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub status: String,
    pub summary: String,
    pub model_content: String,
    pub structured: Option<Value>,
    pub truncated: bool,
}
