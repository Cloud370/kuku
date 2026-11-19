use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Position;

use crate::app::{App, Modal};
use crate::selection::{ClickTarget, MouseAction};

impl App {
    pub(super) fn pointer(&mut self, mouse: MouseEvent, screen: &Buffer) {
        let click = self.click_target(mouse);
        match self.text_selection.mouse(mouse, screen, click) {
            Some(MouseAction::Copy(text)) => self.copy_text(text),
            Some(MouseAction::Click(ClickTarget::Detail(key))) => {
                self.toggle_transcript_detail(&key);
            }
            Some(MouseAction::Click(ClickTarget::Picker(hit))) => {
                if let Some(Modal::Picker(picker)) = &mut self.modal {
                    if let Some(choice) = picker.click(hit) {
                        let kind = picker.kind.clone();
                        self.choose(kind, choice);
                    }
                }
            }
            Some(MouseAction::Click(ClickTarget::Agent(target))) => self.select_agent(target),
            Some(MouseAction::Click(ClickTarget::Link(url))) => {
                if let Err(error) = crate::open_browser(&url) {
                    self.problem(error.to_string());
                }
            }
            Some(MouseAction::Click(ClickTarget::Queued)) => self.open_queued(),
            Some(MouseAction::Click(ClickTarget::Messages)) => self.open_messages_picker(),
            Some(MouseAction::Click(ClickTarget::Transcript)) => self.scroll(0),
            Some(MouseAction::Click(ClickTarget::Composer)) => {
                let area = self.composer_area;
                self.focus_composer();
                self.pane_mut().draft.place_cursor(
                    usize::from(area.width),
                    usize::from(mouse.column - area.x),
                    usize::from(mouse.row - area.y),
                );
            }
            Some(MouseAction::Click(ClickTarget::JumpLatest)) => self.jump_latest(),
            None => {}
        }
    }

    fn click_target(&self, mouse: MouseEvent) -> Option<ClickTarget> {
        if !matches!(
            mouse.kind,
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
        ) || !mouse.modifiers.is_empty()
        {
            return None;
        }
        let position = Position::new(mouse.column, mouse.row);
        match &self.modal {
            Some(Modal::Picker(picker)) => picker.hit(position).map(ClickTarget::Picker),
            Some(Modal::Prompt(_) | Modal::Output(_) | Modal::StatusLineEditor(_)) => None,
            None => {
                if self
                    .text_selection
                    .messages_area()
                    .is_some_and(|area| area.contains(position))
                {
                    Some(ClickTarget::Messages)
                } else if self.reading_area.contains(position)
                    && self.pane().transcript.is_reading()
                {
                    Some(ClickTarget::JumpLatest)
                } else if self.composer_area.contains(position) {
                    Some(ClickTarget::Composer)
                } else if self.delivery_area.contains(position)
                    && self.queued_for_edit().next().is_some()
                {
                    Some(ClickTarget::Queued)
                } else {
                    self.text_selection
                        .transcript_cell(mouse)
                        .map(|(row, column)| {
                            if let Some(url) = self.pane().transcript.markdown_link_at(row, column)
                            {
                                return ClickTarget::Link(url.to_owned());
                            }
                            if let Some(agent) = self.pane().transcript.agent_link_at(
                                row,
                                column,
                                self.transcript_area.0,
                            ) {
                                return ClickTarget::Agent(kuku_client::Target {
                                    workspace: self.workspace.clone(),
                                    session_id: self.active.session.clone(),
                                    agent: agent.to_owned(),
                                });
                            }
                            self.pane()
                                .transcript
                                .detail_at(row)
                                .cloned()
                                .map_or(ClickTarget::Transcript, ClickTarget::Detail)
                        })
                }
            }
        }
    }
}
