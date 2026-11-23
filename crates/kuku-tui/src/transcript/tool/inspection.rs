use crate::text::single_line;
use crate::theme::{tool_operation_color, MUTED, SURFACE_TEXT};

use super::{
    body_inset, finish_card, render_preview, render_rows, Row, Tool, ToolDetails, ToolView,
    MAX_INSPECTION_ROWS, MAX_SUMMARY_ROWS, MAX_WARNING_ROWS,
};

impl Tool {
    pub(in crate::transcript) fn inspection_preview(&self, width: usize) -> Option<Row> {
        match self.group_name()? {
            "read_file" if self.read_path().is_some() => {
                self.render_read_details(width, false)?.into_iter().next()
            }
            "find_files" | "search_text" if !self.invocation().is_empty() => Some(Row::action(
                &format!("{} · ", self.title()),
                &single_line(self.invocation()),
                self.search_summary().unwrap_or_default(),
                tool_operation_color(self.name()),
                width,
            )),
            "run_js" => {
                let script = self.state.outcome()?.script.as_ref()?;
                render_rows(vec![script.inspection_preview()?], width, 1, 0).next()
            }
            _ => None,
        }
    }

    pub(super) fn render_failed_read(&self, width: usize, details: ToolDetails) -> Vec<Row> {
        let body_width = width;
        let (status, _) = self.status_line();
        let mut rows = Row::styled_status(
            "Read file",
            status,
            tool_operation_color("read_file"),
            width,
        );
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
        rows.extend(body);
        finish_card(rows)
    }

    pub(super) fn render_read_details(&self, width: usize, open: bool) -> Option<Vec<Row>> {
        let read = self.state.outcome()?.file_read.as_ref()?;
        let inset = body_inset(width);
        let mut rows = vec![Row::read(
            &single_line(self.invocation()),
            read.group_caption(),
            width,
        )];
        if open {
            rows.extend(render_preview(
                self.result(),
                MUTED,
                width,
                MAX_INSPECTION_ROWS,
                inset,
            ));
            self.render_arguments(&mut rows, width, inset);
        }
        Some(rows)
    }
}

impl ToolView {
    pub(crate) fn has_full_output(&self) -> bool {
        self.output_id.is_some()
            || (self.inline_output_complete && (!self.details.is_empty() || !self.diff.is_empty()))
    }
}
