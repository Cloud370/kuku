use crate::text::single_line;
use crate::theme::{ACCENT, MODEL, MUTED, SURFACE_TEXT};

use super::{
    body_inset, finish_card, render_preview, Row, Tool, ToolDetails, MAX_INSPECTION_ROWS,
    MAX_SUMMARY_ROWS, MAX_WARNING_ROWS,
};

impl Tool {
    pub(super) fn render_failed_read(&self, width: usize, details: ToolDetails) -> Vec<Row> {
        let rail = body_inset(width);
        let body_width = width.saturating_sub(usize::from(rail));
        let (status, color) = self.status_label();
        let mut rows = vec![Row::caption(
            &format!("Read file · {status}"),
            "",
            color,
            width,
        )];
        let diagnostic = if self.result().trim().is_empty() {
            self.summary()
        } else {
            self.result()
        };
        let mut body = Vec::new();
        let path = single_line(self.invocation());
        if !path.is_empty() && !diagnostic.contains(&path) {
            body.extend(render_preview(&path, SURFACE_TEXT, body_width, 1, 0));
        }
        body.extend(render_preview(
            diagnostic,
            SURFACE_TEXT,
            body_width,
            MAX_WARNING_ROWS,
            0,
        ));
        if details == ToolDetails::Inspection {
            if self.summary() != diagnostic {
                body.extend(render_preview(
                    self.summary(),
                    MUTED,
                    body_width,
                    MAX_SUMMARY_ROWS,
                    0,
                ));
            }
            self.render_arguments(&mut body, body_width, 0);
        }
        rows.extend(body.into_iter().map(|row| row.with_rail(rail)));
        finish_card(rows)
    }

    pub(super) fn render_inspection_rows(&self, width: usize, open: bool) -> Option<Vec<Row>> {
        let outcome = self.state.outcome()?;
        let (headline, body) = if let Some(read) = &outcome.file_read {
            (
                Row::action(
                    "Read ",
                    &single_line(self.invocation()),
                    &read.group_caption(),
                    ACCENT,
                    width,
                ),
                open.then(|| self.result().to_owned()),
            )
        } else if let Some(script) = &outcome.script {
            (
                Row::caption(
                    "Script",
                    &super::super::details::inspection_composition(std::iter::once(self), false),
                    MODEL,
                    width,
                ),
                open.then(|| script.action_details().collect::<Vec<_>>().join("\n")),
            )
        } else {
            return None;
        };
        let mut rows = vec![headline];
        if let Some(body) = body {
            rows.extend(render_preview(
                &body,
                MUTED,
                width,
                MAX_INSPECTION_ROWS,
                body_inset(width),
            ));
            if outcome.file_read.is_some() {
                self.render_arguments(&mut rows, width, body_inset(width));
            } else {
                rows.extend(self.script_code_rows(width, body_inset(width)));
            }
        }
        Some(rows)
    }
}
