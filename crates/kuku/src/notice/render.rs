use super::types::{ContextDriftStatus, Notice, NoticeKind};

const NOTICE_CONTEXT_DRIFT_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/prompts/notice-context-drift.md"
));

const NOTICE_SKILL_CHANGED_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/prompts/notice-skill-changed.md"
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
        NoticeKind::SkillChanged {
            updated,
            added,
            removed,
        } => {
            let mut lines = Vec::new();
            for entry in added {
                lines.push(format!(
                    "- {} (added) — {} ({})",
                    entry.name, entry.description, entry.path
                ));
            }
            for entry in removed {
                lines.push(format!("- {} (removed)", entry));
            }
            for entry in updated {
                lines.push(format!(
                    "- {} (updated) — {} ({})",
                    entry.name, entry.description, entry.path
                ));
            }
            if lines.is_empty() {
                return None;
            }
            let body =
                NOTICE_SKILL_CHANGED_TEMPLATE.replace("{{skill_changes}}", &lines.join("\n"));
            Some(format!("{body}{NOTICE_IGNORE_HINT}"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notice::types::{
        ContextDriftEntry, ContextDriftStatus, NoticeSeverity, SkillChangeEntry,
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
    fn renders_skill_changed_body() {
        let notice = Notice {
            kind: NoticeKind::SkillChanged {
                updated: vec![],
                added: vec![SkillChangeEntry {
                    name: "review".to_string(),
                    description: "Code review".to_string(),
                    path: "~/.claude/skills/review".to_string(),
                }],
                removed: vec![],
            },
            severity: NoticeSeverity::Info,
        };
        let rendered = render_notice_body(&notice).expect("should render");
        assert!(rendered.contains("- review (added) — Code review (~/.claude/skills/review)"));
        assert!(rendered.contains("If not relevant"));
        assert!(!rendered.contains("<kuku_system_notice>"));
    }

    #[test]
    fn renders_removed_skill_without_desc_or_path() {
        let notice = Notice {
            kind: NoticeKind::SkillChanged {
                updated: vec![],
                added: vec![],
                removed: vec!["old-skill".to_string()],
            },
            severity: NoticeSeverity::Info,
        };
        let rendered = render_notice_body(&notice).expect("should render");
        assert!(rendered.contains("- old-skill (removed)"));
        assert!(
            !rendered.contains(" — "),
            "removed should have no desc/path dash"
        );
    }

    #[test]
    fn skill_changed_returns_none_when_all_empty() {
        let notice = Notice {
            kind: NoticeKind::SkillChanged {
                updated: vec![],
                added: vec![],
                removed: vec![],
            },
            severity: NoticeSeverity::Info,
        };
        assert!(render_notice_body(&notice).is_none());
    }
}
