use crate::conversation::address::ConversationAddress;

use super::types::{
    ContextDriftStatus, ConversationInboxMessage, Notice, NoticeKind, OpenConversationEntry,
};

const NOTICE_CONTEXT_DRIFT_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/prompts/notice-context-drift.md"
));

const NOTICE_IGNORE_HINT: &str =
    "\nIf not relevant to your current task, ignore. Do not mention to the user.\n";

/// Render the inner body of a notice (no outer `<kuku_system_notice>` wrapper).
pub(crate) fn render_notice_body(notice: &Notice) -> Option<String> {
    match &notice.kind {
        NoticeKind::ContextDrift { entries } => {
            let rendered_entries: String = entries
                .iter()
                .map(render_context_drift_entry)
                .collect::<Vec<_>>()
                .join("\n");
            let body = NOTICE_CONTEXT_DRIFT_TEMPLATE.replace("{{entries}}", &rendered_entries);
            Some(format!("{body}{NOTICE_IGNORE_HINT}"))
        }
        NoticeKind::AgentDirectory { directory } => Some(directory.clone()),
        NoticeKind::OpenConversations { entries } => {
            if entries.is_empty() {
                None
            } else {
                Some(format!(
                    "Open conversations:\n{}",
                    entries
                        .iter()
                        .map(render_open_conversation_entry)
                        .collect::<Vec<_>>()
                        .join("\n")
                ))
            }
        }
        NoticeKind::ConversationInbox { messages } => {
            if messages.is_empty() {
                None
            } else {
                Some(format!(
                    "Incoming messages for this conversation:\n{}",
                    messages
                        .iter()
                        .map(render_inbox_message)
                        .collect::<Vec<_>>()
                        .join("\n")
                ))
            }
        }
        NoticeKind::LoadedSkills { skills } => {
            if skills.is_empty() {
                None
            } else {
                Some(format!(
                    "Loaded skills for this conversation: {}",
                    skills.join(", ")
                ))
            }
        }
        NoticeKind::PendingPermission { request } => Some(format!(
            "Pending permission for this conversation: {} | tool: {} | candidate: {}",
            request.summary, request.tool, request.candidate
        )),
        NoticeKind::InterruptedTurn {
            conversation,
            turn,
            reason,
        } => Some(format!(
            "Interrupted turn for {}: turn {} | reason: {}",
            render_conversation(conversation),
            turn,
            reason
        )),
    }
}

fn render_context_drift_entry(entry: &super::types::ContextDriftEntry) -> String {
    let status = match entry.status {
        ContextDriftStatus::Updated => "updated",
        ContextDriftStatus::Deleted => "deleted",
    };
    format!("- {} ({})", entry.path, status)
}

fn render_open_conversation_entry(entry: &OpenConversationEntry) -> String {
    format!(
        "- {}: {}",
        render_conversation(&entry.conversation),
        entry.summary
    )
}

fn render_inbox_message(message: &ConversationInboxMessage) -> String {
    let from = message
        .from
        .as_ref()
        .map(render_conversation)
        .unwrap_or("host".to_string());
    format!("- from {}: {}", from, message.text)
}

fn render_conversation(conversation: &ConversationAddress) -> String {
    conversation.as_str().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notice::types::{
        ContextDriftEntry, ContextDriftStatus, NoticeSeverity, OpenConversationEntry,
    };

    #[test]
    fn renders_context_drift_body() {
        let notice = Notice {
            kind: NoticeKind::ContextDrift {
                entries: vec![
                    ContextDriftEntry {
                        path: "AGENTS.md".to_string(),
                        status: ContextDriftStatus::Updated,
                    },
                    ContextDriftEntry {
                        path: "notes.md".to_string(),
                        status: ContextDriftStatus::Deleted,
                    },
                ],
            },
            severity: NoticeSeverity::Info,
        };

        let rendered = render_notice_body(&notice).expect("should render");
        assert!(rendered.contains("Only unacknowledged drift is reported here."));
        assert!(rendered.contains("successful full-file reads or writes"));
        assert!(rendered.contains("- AGENTS.md (updated)"));
        assert!(rendered.contains("- notes.md (deleted)"));
        assert!(rendered.contains("If not relevant"));
        assert!(!rendered.contains("line 17"));
        assert!(!rendered.contains("current preview:"));
        assert!(!rendered.contains("<kuku_system_notice>"));
    }

    #[test]
    fn renders_open_conversation_summary_body() {
        let notice = Notice {
            kind: NoticeKind::OpenConversations {
                entries: vec![OpenConversationEntry {
                    conversation: ConversationAddress::parse("review").unwrap(),
                    summary: "waiting on response".to_string(),
                }],
            },
            severity: NoticeSeverity::Info,
        };

        let rendered = render_notice_body(&notice).expect("should render");
        assert!(rendered.contains("Open conversations:"));
        assert!(rendered.contains("review: waiting on response"));
    }
}
