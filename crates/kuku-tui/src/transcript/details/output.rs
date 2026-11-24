use kuku_client::OutputChunk;

use crate::output::OutputViewer;
use crate::theme::{ERROR, MUTED};

use super::{DetailKey, DetailKind, Details, Key, Record, Row, Transcript, VisibleDetail};
use crate::transcript::ToolView;
use crate::transcript::{Anchor, AnchorLine, Layout, Reading, render_rows};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::transcript) enum OutputTarget {
    Tool,
    Script,
    ScriptAction(usize),
}

impl OutputTarget {
    pub(in crate::transcript) fn label(self) -> &'static str {
        match self {
            Self::Script => "View script outputs",
            Self::Tool | Self::ScriptAction(_) => "View output",
        }
    }
}

impl VisibleDetail {
    pub(in crate::transcript) fn output(row: usize, key: &Key, target: OutputTarget) -> Self {
        Self::new(
            row..row + 1,
            key,
            DetailKind::Output(target),
            target.label().into(),
            String::new(),
            false,
        )
    }
}

impl Transcript {
    pub(crate) fn detail_outputs(&self, key: &DetailKey) -> Option<Vec<ToolView>> {
        let DetailKind::Output(target) = key.kind else {
            return None;
        };
        self.detail_items.rows_for(key)?;
        self.tool_outputs(key, target)
    }

    fn tool_outputs(&self, key: &DetailKey, target: OutputTarget) -> Option<Vec<ToolView>> {
        let window = self
            .reading
            .as_ref()
            .map_or(&self.recent, |reading| &reading.window);
        let tool = window
            .records
            .iter()
            .find(|record| key.for_record(&record.key))
            .and_then(Record::tool)?;
        let outputs = match target {
            OutputTarget::Tool => vec![tool.format_output(tool.view())],
            OutputTarget::Script => tool.output_views(),
            OutputTarget::ScriptAction(index) => vec![tool.script_action_output(index)?],
        };
        (!outputs.is_empty()).then_some(outputs)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::transcript) enum InlineOutputAction {
    Open,
    Close,
    Previous,
    Next,
    Refresh,
    Name,
}

impl InlineOutputAction {
    pub(in crate::transcript) fn label(self) -> &'static str {
        match self {
            Self::Open => "View output",
            Self::Close => "Hide output",
            Self::Previous => "Previous output page",
            Self::Next => "Next output page",
            Self::Refresh => "Refresh output",
            Self::Name => "Name output",
        }
    }

    pub(in crate::transcript) fn is_toggle(self) -> bool {
        matches!(self, Self::Open | Self::Close)
    }
}

pub(crate) enum InlineOutputEffect {
    Render,
    Read {
        id: u64,
        output: String,
        offset: u64,
        limit: usize,
    },
    Name(String),
}

#[derive(Debug)]
pub(in crate::transcript) struct InlineOutput {
    record: Key,
    pub(in crate::transcript) target: OutputTarget,
    viewer: OutputViewer,
    revision: u64,
}

impl InlineOutput {
    pub(in crate::transcript) fn revision(&self) -> (u64, u64) {
        (self.viewer.id, self.revision)
    }

    fn read(&self, offset: u64) -> Option<InlineOutputEffect> {
        let (output, limit) = self.viewer.read_request(offset)?;
        Some(InlineOutputEffect::Read {
            id: self.viewer.id,
            output,
            offset,
            limit,
        })
    }

    pub(in crate::transcript) fn rows(&self, width: usize, line_count: usize) -> Vec<Row> {
        use InlineOutputAction::{Name, Next, Previous, Refresh};

        let mut rows = Vec::new();
        if let Some(caption) = self.viewer.page_caption() {
            rows.push(Row::caption(&caption, "", MUTED, width));
        }
        rows.extend(
            self.viewer
                .numbered_output_lines(width, line_count)
                .into_iter()
                .map(Row::plain),
        );
        if self.viewer.loading {
            rows.push(Row::caption("Reading saved output…", "", MUTED, width));
            return rows;
        }
        if let Some(problem) = &self.viewer.problem {
            rows.extend(render_rows(
                crate::markup::render_literal(problem, ratatui::style::Style::new().fg(ERROR)),
                width,
                3,
                0,
            ));
        }
        for (action, visible) in [
            (Previous, self.viewer.page_offset(true).is_some()),
            (Next, self.viewer.page_offset(false).is_some()),
            (Refresh, self.viewer.problem.is_some()),
            (Name, self.viewer.tool.output_id.is_some()),
        ] {
            if visible {
                rows.push(Row::inline_output(self.target, action, width));
            }
        }
        rows
    }
}

impl Details {
    pub(in crate::transcript) fn inline_output(&self, key: &Key) -> Option<&InlineOutput> {
        self.inline_output
            .as_ref()
            .filter(|output| &output.record == key)
    }

    pub(super) fn close_inline_output(&mut self, key: &Key) {
        if self.inline_output(key).is_some() {
            self.inline_output = None;
        }
    }
}

impl VisibleDetail {
    pub(in crate::transcript) fn inline_output(
        row: usize,
        key: &Key,
        target: OutputTarget,
        action: InlineOutputAction,
    ) -> Self {
        Self::new(
            row..row + 1,
            key,
            DetailKind::InlineOutput(target, action),
            action.label().into(),
            String::new(),
            action == InlineOutputAction::Close,
        )
    }
}

impl Transcript {
    pub(crate) fn activate_inline_output(
        &mut self,
        key: &DetailKey,
        id: u64,
        width: usize,
        height: usize,
    ) -> Option<InlineOutputEffect> {
        use InlineOutputAction::{Close, Name, Next, Open, Previous, Refresh};

        let DetailKind::InlineOutput(target, action) = key.kind else {
            return None;
        };
        let rows = self.detail_items.rows_for(key)?;
        match action {
            Open => {
                let tool = self.tool_outputs(key, target)?.pop()?;
                let offset = tool.output_range.as_ref().map_or(0, |range| range.start);
                self.keep_output_in_view(key, target, width, height, Some(rows.start));
                let output = InlineOutput {
                    record: key.record.clone(),
                    target,
                    viewer: OutputViewer::new(id, tool),
                    revision: 0,
                };
                let effect = output.read(offset).unwrap_or(InlineOutputEffect::Render);
                self.detail_items.inline_output = Some(output);
                Some(effect)
            }
            Close => {
                self.detail_items.close_inline_output(&key.record);
                Some(InlineOutputEffect::Render)
            }
            Previous | Next | Refresh | Name => {
                let output = self
                    .detail_items
                    .inline_output
                    .as_mut()
                    .filter(|output| output.record == key.record && output.target == target)?;
                if action == Name {
                    return output
                        .viewer
                        .tool
                        .output_id
                        .clone()
                        .map(InlineOutputEffect::Name);
                }
                let offset = if action == Refresh {
                    output.viewer.retry()
                } else {
                    output.viewer.request_page(action == Previous)
                }?;
                output.viewer.id = id;
                let effect = output.read(offset)?;
                self.keep_output_in_view(key, target, width, height, None);
                Some(effect)
            }
        }
    }

    pub(crate) fn apply_inline_output(&mut self, id: u64, result: Result<OutputChunk, String>) {
        let Some(output) = self
            .detail_items
            .inline_output
            .as_mut()
            .filter(|output| output.viewer.id == id)
        else {
            return;
        };
        match result {
            Ok(chunk) => output.viewer.apply(chunk),
            Err(problem) => {
                output.viewer.loading = false;
                output.viewer.problem = Some(problem);
            }
        }
        output.revision = output.revision.wrapping_add(1);
    }

    fn keep_output_in_view(
        &mut self,
        key: &DetailKey,
        target: OutputTarget,
        width: usize,
        height: usize,
        row: Option<usize>,
    ) {
        let layout = Layout {
            width,
            tools: self.details,
            assistant: self.assistant_view,
            agents: &self.agent_cards,
            items: &self.detail_items,
            notes: self.notes.as_ref(),
        };
        let reveal = row.map(|row| (row + 5).saturating_sub(height));
        let trigger = Anchor {
            key: key.record.clone(),
            line: AnchorLine::InlineOutput(target),
        };
        if let Some(reading) = &mut self.reading {
            match reveal {
                None => reading.anchor = trigger,
                Some(reveal) if reveal > 0 => {
                    let start = reading.window.position_of(&reading.anchor, layout);
                    let end = reading.window.position_of(&trigger, layout);
                    if let Some(anchor) =
                        reading.window.anchor_at((start + reveal).min(end), layout)
                    {
                        reading.anchor = anchor;
                    }
                }
                Some(_) => {}
            }
        } else {
            let anchor = reveal
                .and_then(|reveal| {
                    let start = self.recent.row_count(layout).saturating_sub(height);
                    let end = self.recent.position_of(&trigger, layout);
                    self.recent.anchor_at((start + reveal).min(end), layout)
                })
                .unwrap_or(trigger);
            self.reading = Some(Reading {
                window: self.recent.clone(),
                anchor,
                newer: None,
                rebase_after_page: false,
                visible_through: None,
                unseen: false,
                manual: true,
            });
        }
    }
}
