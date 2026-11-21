use kuku_client::RetryMetadata;
use ratatui::style::Color;

use super::job::JobNotice;
use super::tool::Tool;

#[derive(Debug, Clone)]
pub(super) enum Body {
    User {
        sender: Option<String>,
        text: String,
    },
    Assistant(String),
    Interrupted(String),
    Notice(&'static str, String, Color),
    Job(JobNotice),
    ModelError {
        title: &'static str,
        diagnostic: String,
        hint: String,
        fallback_hint: String,
        retry_message: Option<String>,
        request_id: Option<String>,
        reply_interrupted: bool,
    },
    Retry {
        metadata: RetryMetadata,
        include_detail: bool,
    },
    TurnSummary,
    Tool(Box<Tool>),
}
