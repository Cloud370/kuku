use crate::conversation::address::ConversationAddress;
use crate::prompt::PromptCatalog;

use super::types::{
    ContextDriftStatus, ConversationInboxMessage, Notice, NoticeKind, OpenConversationEntry,
};

/// Get a block template from the catalog, falling back to an inline string.
fn block_tmpl<'a>(catalog: &'a PromptCatalog, name: &str, fallback: &'a str) -> &'a str {
    catalog
        .blocks
        .get(name)
        .map(|a| a.text.as_str())
        .unwrap_or(fallback)
}

const NOTICE_IGNORE_HINT: &str =
    "\nIf not relevant to your current task, ignore. Do not mention to the user.\n";

/// Render a notice body, wrapped in `<kuku_system_notice>` where the template
/// does not already include it.
pub(crate) fn render_notice_body(notice: &Notice, catalog: &PromptCatalog) -> Option<String> {
    match &notice.kind {
        NoticeKind::ContextDrift { entries } => {
            let rendered_entries: String = entries
                .iter()
                .map(render_context_drift_entry)
                .collect::<Vec<_>>()
                .join("\n");
            let body_tmpl = block_tmpl(
                catalog,
                "notice-context-drift",
                "Previously loaded file-backed context has changed.\n\nChanged tracked files:\n{{entries}}",
            );
            let body = body_tmpl.replace("{{entries}}", &rendered_entries);
            let wrapper = block_tmpl(
                catalog,
                "system-notice",
                "<kuku_system_notice>\n{{notice_body}}\n</kuku_system_notice>",
            );
            Some(wrapper.replace("{{notice_body}}", &format!("{body}{NOTICE_IGNORE_HINT}")))
        }
        NoticeKind::AgentDirectory { directory } => {
            let tmpl = block_tmpl(
                catalog,
                "notice-agent-directory",
                "<kuku_system_notice>\nThis conversation can route work to specialist agents.\n{{agent_list}}\nUse exact agent names when delegating.\n</kuku_system_notice>",
            );
            Some(tmpl.replace("{{agent_list}}", directory))
        }
        NoticeKind::OpenConversations { entries } => {
            if entries.is_empty() {
                return None;
            }
            let list = entries
                .iter()
                .map(render_open_conversation_entry)
                .collect::<Vec<_>>()
                .join("\n");
            let tmpl = block_tmpl(
                catalog,
                "notice-open-conversations",
                "<kuku_system_notice>\nOpen conversations:\n{{conversation_list}}\n</kuku_system_notice>",
            );
            Some(tmpl.replace("{{conversation_list}}", &list))
        }
        NoticeKind::ConversationInbox { messages } => {
            if messages.is_empty() {
                return None;
            }
            let list = messages
                .iter()
                .map(render_inbox_message)
                .collect::<Vec<_>>()
                .join("\n");
            let tmpl = block_tmpl(
                catalog,
                "notice-inbox",
                "<kuku_system_notice>\nUnread messages:\n{{inbox_messages}}\n</kuku_system_notice>",
            );
            Some(tmpl.replace("{{inbox_messages}}", &list))
        }
        NoticeKind::LoadedSkills { skills } => {
            if skills.is_empty() {
                return None;
            }
            let tmpl = block_tmpl(
                catalog,
                "notice-loaded-skills",
                "<kuku_system_notice>\nLoaded skills:\n{{skill_list}}\n</kuku_system_notice>",
            );
            Some(tmpl.replace("{{skill_list}}", &skills.join(", ")))
        }
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
    use crate::prompt::builtin_prompt_catalog;

    fn test_catalog() -> PromptCatalog {
        builtin_prompt_catalog()
    }

    #[test]
    fn renders_context_drift_body() {
        let catalog = test_catalog();
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

        let rendered = render_notice_body(&notice, &catalog).expect("should render");
        assert!(rendered.contains("Only unacknowledged drift is reported here."));
        assert!(rendered.contains("successful full-file reads or writes"));
        assert!(rendered.contains("- AGENTS.md (updated)"));
        assert!(rendered.contains("- notes.md (deleted)"));
        assert!(rendered.contains("If not relevant"));
        assert!(!rendered.contains("line 17"));
        assert!(!rendered.contains("current preview:"));
        // Now wrapped with system-notice template
        assert!(rendered.contains("<kuku_system_notice>"));
    }

    #[test]
    fn renders_open_conversation_summary_body() {
        let catalog = test_catalog();
        let notice = Notice {
            kind: NoticeKind::OpenConversations {
                entries: vec![OpenConversationEntry {
                    conversation: ConversationAddress::parse("review").unwrap(),
                    summary: "waiting on response".to_string(),
                }],
            },
            severity: NoticeSeverity::Info,
        };

        let rendered = render_notice_body(&notice, &catalog).expect("should render");
        assert!(rendered.contains("Open conversations:"));
        assert!(rendered.contains("review: waiting on response"));
        assert!(rendered.contains("<kuku_system_notice>"));
    }
}
