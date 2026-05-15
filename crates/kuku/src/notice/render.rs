use super::types::{ContextDriftStatus, Notice, NoticeKind};

pub(crate) fn render_notice_block(notice: &Notice) -> String {
    match &notice.kind {
        NoticeKind::ContextDrift { entries } => {
            let mut lines = vec![
                "<kuku_system_notice>".to_string(),
                "Previously loaded file-backed context has changed since the last acknowledged snapshot.".to_string(),
                "".to_string(),
                "Only unacknowledged drift is reported here.".to_string(),
                "Changes already acknowledged through successful full-file reads or writes are not included.".to_string(),
                "".to_string(),
                "This notice does not include the changed file contents.".to_string(),
                "Do not assume you know what changed from this notice alone.".to_string(),
                "".to_string(),
                "Use the context loaded in this turn as the current source of truth.".to_string(),
                "If the task depends on the details of a changed file that is not fully included in the current prompt, read that file again before relying on it.".to_string(),
                "".to_string(),
                "Changed tracked files:".to_string(),
            ];
            lines.extend(entries.iter().map(render_context_drift_entry));
            lines.push("</kuku_system_notice>".to_string());
            lines.join("\n")
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
}
