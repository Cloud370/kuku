use crate::keys::Shortcut;
use crate::theme::CardFrame;

use super::details::{DetailRecord, VisibleDetail};
use super::{
    body_inset, message, preview, Block, Key, Layout, ToolDetails, Transcript, TranscriptView,
    TurnSurface,
};

impl Transcript {
    pub(crate) fn project(&mut self, width: usize, height: usize) -> TranscriptView {
        self.link_recent_agent_cards();
        if let Some(notes) = &mut self.notes {
            notes.fit(width);
        }
        self.keep_first_reply_line(width, height);
        let layout = Layout {
            width,
            tools: self.details,
            assistant: self.assistant_view,
            agents: &self.agent_cards,
            items: &self.detail_items,
            notes: self.notes.as_ref(),
        };
        let (blocks, start, following) = if let Some(reading) = &mut self.reading {
            let blocks = reading.window.blocks(layout);
            if std::mem::take(&mut reading.rebase_after_page) {
                Block::rebase_anchor(&blocks, &mut reading.anchor);
            }
            let start = Block::position_of(&blocks, &reading.anchor);
            reading.visible_through = super::history::last_visible_card(&blocks, start, height);
            (blocks, start, false)
        } else {
            let blocks = self.recent.blocks(layout);
            let start = Block::row_count(&blocks).saturating_sub(height);
            (blocks, start, true)
        };
        let reply_top =
            if following && self.reply_position.is_none() && self.current_execution.is_some() {
                Block::anchor_at(&blocks, start)
            } else {
                None
            };
        let limit = height;
        let group_frame = CardFrame::timeline(width);
        let mut turn_surface = TurnSurface {
            connector_column: group_frame.bordered().then(|| {
                group_frame.content_inset() + usize::from(body_inset(group_frame.content_width()))
            }),
            rows: Vec::new(),
            connectors: Vec::new(),
        };
        let mut lines = Vec::with_capacity(limit);
        let mut row_insets = Vec::with_capacity(height);
        self.visible_links.clear();
        let mut visible = Vec::new();
        let mut thinking_row = None;
        let mut skip = start;
        self.visible_rewind_actions.clear();
        let mut blocks = blocks.into_iter();
        while let Some(mut block) = blocks.next() {
            if lines.len() == limit {
                break;
            }
            if skip == 0 && block.height() == 0 {
                if let Some(detail @ DetailRecord::Reasoning(_)) = block.detail {
                    let row = lines.len();
                    visible.push(VisibleDetail::record(row..row, block.key, detail, false));
                }
                continue;
            }
            if skip >= block.height() {
                skip -= block.height();
                continue;
            }
            let block_end = lines.len() + block.height() - skip;
            if let Some(group) = block.rewind_group {
                let first = lines.len();
                for (offset, row) in group.rows.iter().enumerate().skip(skip).take(limit - first) {
                    lines.push(row.line.clone());
                    row_insets.push(row.text_insets());
                    if let Some(action) = group.action_at(offset) {
                        self.visible_rewind_actions.push((
                            lines.len() - 1,
                            group.summary.rewind_event_id,
                            action,
                        ));
                    }
                }
                skip = skip.saturating_sub(group.rows.len());
                continue;
            }
            if block.turn_gap {
                if skip == 0 {
                    lines.push(ratatui::text::Line::default());
                    row_insets.push(Default::default());
                }
                skip = skip.saturating_sub(1);
                if lines.len() == limit {
                    break;
                }
            }
            let turn_start = lines.len();
            if let Some(turn) = block.heading {
                if skip == 0 {
                    lines.push(message::render_turn(turn, width));
                    row_insets.push(Default::default());
                }
                skip = skip.saturating_sub(1);
            }
            if let Some(group) = block.group.take() {
                let first = lines.len();
                let title_visible = skip == 0;
                for row in group.rows.iter().skip(skip).take(limit - first) {
                    lines.push(row.line.clone());
                    row_insets.push(row.text_insets());
                }
                skip = skip.saturating_sub(group.rows.len());
                let end = lines.len()
                    - usize::from(lines.last().is_some_and(|line| line.spans.is_empty()));
                if first < end {
                    visible.push(VisibleDetail::group(
                        first..end,
                        block.key,
                        group,
                        title_visible,
                    ));
                }
            }
            let first = lines.len();
            let footer_skip = skip.saturating_sub(block.body_height());
            for row in block.body_rows().skip(skip).take(limit - lines.len()) {
                let row = if block.grouped {
                    row.clone().framed(&group_frame)
                } else {
                    row.clone()
                };
                if let (Some(detail), Some(DetailRecord::Tool(tool))) =
                    (row.script_detail, block.detail.as_ref())
                {
                    visible.extend(VisibleDetail::script_detail(
                        lines.len(),
                        block.key,
                        tool,
                        detail,
                        self.detail_items.script_detail(block.key) == Some(detail),
                    ));
                }
                if let Some(target) = row.output {
                    visible.push(VisibleDetail::output(lines.len(), block.key, target));
                }
                if let Some((target, action)) = row.inline_output {
                    visible.push(VisibleDetail::inline_output(
                        lines.len(),
                        block.key,
                        target,
                        action,
                    ));
                }
                if let Some(open) = row.command_arguments {
                    visible.push(VisibleDetail::command_arguments(
                        lines.len(),
                        block.key,
                        open,
                    ));
                }
                row_insets.push(row.text_insets());
                self.visible_links
                    .extend(row.links.into_iter().map(|link| (lines.len(), link)));
                lines.push(row.line);
            }
            let placeholder = 1 + block.note_height();
            if following
                && block.thinking
                && skip <= placeholder
                && first + placeholder - skip < lines.len()
            {
                thinking_row = Some(first + placeholder - skip);
            }
            if let Some(detail) = block.detail {
                let last = lines.len()
                    - usize::from(
                        lines.len() > first
                            && lines.last().is_some_and(|line| line.spans.is_empty()),
                    );
                let open_script = matches!(detail, DetailRecord::Tool(tool) if tool.name() == "run_js")
                    && self
                        .detail_items
                        .tool_mode(block.key, ToolDetails::Collapsed)
                        == ToolDetails::Inspection;
                let headline_only = matches!(detail, DetailRecord::Reasoning(_)) || open_script;
                let last = if headline_only {
                    last.min(first + usize::from(skip == 0))
                } else {
                    last
                };
                if last > first
                    || (matches!(detail, DetailRecord::Reasoning(_))
                        && block.rows.is_empty()
                        && first < height)
                {
                    visible.push(VisibleDetail::record(
                        first..last,
                        block.key,
                        detail,
                        skip == 0 && !block.grouped,
                    ));
                }
            }
            for row in block
                .footer
                .into_iter()
                .skip(footer_skip)
                .take(limit - lines.len())
            {
                row_insets.push(row.text_insets());
                lines.push(row.line);
            }
            if let Some(turn) = block.turn.filter(|_| turn_start < lines.len()) {
                let mut end = lines.len();
                if end == block_end && lines[end - 1].spans.is_empty() {
                    let next = blocks.as_slice().iter().find(|next| next.height() != 0);
                    let column = turn_surface.connector_column;
                    let inset = column.map_or(1, |column| column + 2);
                    let metric = next.and_then(|_| block.timing.line(width, inset));
                    if next.is_some_and(|next| next.turn == Some(turn)) {
                        if let Some(column) = column {
                            turn_surface.connectors.push(end - 1);
                            row_insets[end - 1].left = (column + 1) as u16;
                        }
                    } else if metric.is_none() {
                        end -= 1;
                    }
                    if let Some(metric) = metric {
                        lines[end - 1] = metric;
                        row_insets[end - 1].left = inset as u16;
                    }
                }
                if turn_start < end {
                    turn_surface.rows.push(turn_start..end);
                }
            }
            skip = 0;
        }
        if thinking_row.is_some() && self.reply_position.is_none() {
            if let (Some(execution), Some(top)) = (self.current_execution, reply_top) {
                self.reply_position = Some(preview::ReplyPosition {
                    key: Key::Preview(execution),
                    top,
                });
            }
        }
        self.detail_items.visible = visible;
        let tool_hint = (!self.detail_items.visible.is_empty()).then(|| {
            if self.detail_items.only_reasoning() {
                return Shortcut::DETAILS.label.to_owned();
            }
            let action = if self.details_can_expand() {
                "show"
            } else {
                "hide"
            };
            format!("{} {action}", Shortcut::DETAILS.label)
        });
        TranscriptView {
            thinking_row,
            lines,
            row_insets,
            turn_surface,
            following,
            tool_hint,
        }
    }

    pub(super) fn details_can_expand(&self) -> bool {
        self.details == ToolDetails::Collapsed
            || self.detail_items.has_collapsed_detail()
            || self
                .detail_items
                .agent_keys()
                .any(|key| !self.agent_cards.is_open(key))
    }
}
