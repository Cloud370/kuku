use crate::keys::Shortcut;
use crate::theme::CardFrame;

use super::details::{DetailRecord, VisibleDetail};
use super::{message, preview, Key, Layout, ToolDetails, Transcript, TranscriptView};

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
        let (window, start, following) = if let Some(reading) = &mut self.reading {
            let start = reading.window.position_of(&reading.anchor, layout);
            (&mut reading.window, start, false)
        } else {
            let rows = self.recent.row_count(layout);
            (&mut self.recent, rows.saturating_sub(height), true)
        };
        let limit = height;
        let group_frame = CardFrame::new(width);
        let mut lines = Vec::with_capacity(limit);
        let mut row_insets = Vec::with_capacity(height);
        self.visible_links.clear();
        let mut visible = Vec::new();
        let mut thinking_row = None;
        let mut skip = start;
        self.visible_rewind_actions.clear();
        for mut block in window.blocks(layout) {
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
            if let Some(group) = block.rewind_group {
                let first = lines.len();
                for (offset, row) in group.rows.iter().enumerate().skip(skip).take(limit - first) {
                    lines.push(row.line.clone());
                    row_insets.push(row.inset);
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
                    row_insets.push(0);
                }
                skip = skip.saturating_sub(1);
                if lines.len() == limit {
                    break;
                }
            }
            if let Some(turn) = block.heading {
                if skip == 0 {
                    lines.push(message::render_turn(turn, width));
                    row_insets.push(0);
                }
                skip = skip.saturating_sub(1);
            }
            if let Some(group) = block.group.take() {
                let first = lines.len();
                let title_visible = skip == 0;
                for row in group.rows.iter().skip(skip).take(limit - first) {
                    lines.push(row.line.clone());
                    row_insets.push(row.inset);
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
                self.visible_links
                    .extend(row.links.into_iter().map(|link| (lines.len(), link)));
                lines.push(row.line);
                row_insets.push(row.inset);
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
                let last = if let DetailRecord::Reasoning(_) = detail {
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
                lines.push(row.line);
                row_insets.push(row.inset);
            }
            skip = 0;
        }
        if thinking_row.is_some() && self.reply_position.is_none() {
            if let (Some(execution), Some(top)) =
                (self.current_execution, window.anchor_at(start, layout))
            {
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
