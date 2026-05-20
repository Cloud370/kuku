use super::types::{ContextDriftStatus, Notice, NoticeKind};

const NOTICE_CONTEXT_DRIFT_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/prompts/notice-context-drift.md"
));

const NOTICE_SKILL_CHANGED_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/prompts/notice-skill-changed.md"
));

pub(crate) fn render_notice_block(notice: &Notice) -> String {
    match &notice.kind {
        NoticeKind::ContextDrift { entries } => {
            let rendered_entries: String = entries
                .iter()
                .map(render_context_drift_entry)
                .collect::<Vec<_>>()
                .join("\n");
            NOTICE_CONTEXT_DRIFT_TEMPLATE.replace("{{entries}}", &rendered_entries)
        }
        NoticeKind::SkillChanged {
            updated,
            added,
            removed,
        } => {
            let mut parts = Vec::new();
            for name in added {
                parts.push(format!("{name} (added)"));
            }
            for name in updated {
                parts.push(format!("{name} (updated)"));
            }
            for name in removed {
                parts.push(format!("{name} (removed)"));
            }
            NOTICE_SKILL_CHANGED_TEMPLATE.replace("{{summary}}", &parts.join(", "))
        }
    }
}

fn render_context_drift_entry(entry: &super::types::ContextDriftEntry) -> String {
    let status = match entry.status {
        ContextDriftStatus::Updated => "updated",
        ContextDriftStatus::Deleted => "deleted",
    };
    format!("- {} ({status})", entry.path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notice::types::{ContextDriftEntry, ContextDriftStatus, NoticeSeverity};

    #[test]
    fn renders_summary_only_context_drift_notice() {
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

        let rendered = render_notice_block(&notice);
        assert!(rendered.contains("Only unacknowledged drift is reported here."));
        assert!(rendered.contains("successful full-file reads or writes"));
        assert!(rendered.contains("- AGENTS.md (updated)"));
        assert!(rendered.contains("- notes.md (deleted)"));
        assert!(!rendered.contains("line 17"));
        assert!(!rendered.contains("current preview:"));
    }

    #[test]
    fn renders_skill_changed_notice() {
        let notice = Notice {
            kind: NoticeKind::SkillChanged {
                updated: vec!["tdd".to_string()],
                added: vec!["review".to_string()],
                removed: vec![],
            },
            severity: NoticeSeverity::Info,
        };
        let rendered = render_notice_block(&notice);
        assert!(rendered.contains("tdd (updated)"));
        assert!(rendered.contains("review (added)"));
        assert!(!rendered.contains("removed"));
    }
}
