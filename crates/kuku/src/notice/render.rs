use crate::prompt::PromptCatalog;

use super::types::{ContextDriftStatus, Notice, NoticeKind};

const NOTICE_IGNORE_HINT: &str =
    "\nIf not relevant to your current task, ignore. Do not mention to the user.\n";

/// Render a notice body, wrapped in `<kuku_system_notice>` where the template
/// does not already include it.
pub(crate) fn render_notice_body(
    notice: &Notice,
    catalog: &PromptCatalog,
) -> crate::Result<Option<String>> {
    let renderer = catalog.renderer();
    let text = match &notice.kind {
        NoticeKind::ContextDrift { entries } => {
            let entries = entries
                .iter()
                .map(render_context_drift_entry)
                .collect::<Vec<_>>()
                .join("\n");
            let body = renderer.render(
                "blocks/notice-context-drift.md",
                minijinja::context! { entries },
            )?;
            renderer.render(
                "blocks/system-notice.md",
                minijinja::context! {
                    notice_body => format!("{body}{NOTICE_IGNORE_HINT}"),
                },
            )?
        }
        NoticeKind::LoadedSkills { skills } => {
            if skills.is_empty() {
                return Ok(None);
            }
            renderer.render(
                "blocks/notice-loaded-skills.md",
                minijinja::context! {
                    skill_list => skills.join(", "),
                    skills => minijinja::value::Serde(skills),
                },
            )?
        }
    };
    Ok(Some(text))
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
    use crate::notice::types::{ContextDriftEntry, ContextDriftStatus, NoticeSeverity};
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

        let rendered = render_notice_body(&notice, &catalog)
            .unwrap()
            .expect("should render");
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
}
