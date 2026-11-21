use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Position;

use crate::app::{App, Modal};
use crate::contextual::ContextControl;
use crate::selection::{ClickTarget, MouseAction};

impl App {
    pub(super) fn pointer(&mut self, mouse: MouseEvent, screen: &Buffer) {
        let click = self.click_target(mouse);
        let selection = if self.modal.is_some() {
            &mut self.modal_selection
        } else {
            &mut self.text_selection
        };
        let action = selection.mouse(mouse, screen, click);
        if matches!(&action, Some(MouseAction::Click(_)))
            && !self.modal.as_ref().is_some_and(Modal::composer_anchored)
        {
            self.deactivate_context_focus();
        }
        match action {
            Some(MouseAction::Copy(text)) => self.copy_text(text),
            Some(MouseAction::Click(ClickTarget::Detail(key))) => {
                self.toggle_transcript_detail(&key);
            }
            Some(MouseAction::Click(ClickTarget::RewindGroup { id, action })) => {
                self.activate_readonly_rewind_group(id, action);
            }
            Some(MouseAction::Click(ClickTarget::Picker(hit))) => {
                if let Some(Modal::Picker(picker)) = &mut self.modal {
                    if let Some(choice) = picker.click(hit) {
                        let kind = picker.kind.clone();
                        let ticket = picker.ticket.id;
                        self.choose(kind, choice, ticket);
                    }
                }
            }
            Some(MouseAction::Click(ClickTarget::Agent(target))) => self.select_agent(target),
            Some(MouseAction::Click(ClickTarget::Request { target, request_id })) => {
                self.focus_context_control(ContextControl::Request { target, request_id });
            }
            Some(MouseAction::Click(ClickTarget::Permission {
                target,
                request_id,
                allow,
            })) => {
                if self.focus_context_control(ContextControl::Request { target, request_id }) {
                    self.pane_mut().permission_choice = Some(allow);
                }
            }
            Some(MouseAction::Click(ClickTarget::Contextual(control))) => {
                self.focus_context_control(control);
            }
            Some(MouseAction::Click(ClickTarget::Link(url))) => {
                if let Err(error) = crate::open_browser(&url) {
                    self.problem(error.to_string());
                }
            }
            Some(MouseAction::Click(ClickTarget::QueueInput(id))) => self.focus_queue_input(id),
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
            Some(
                Modal::Help
                | Modal::Prompt(_)
                | Modal::Output(_)
                | Modal::StatusLineEditor(_)
                | Modal::ExitConfirmation,
            ) => None,
            None => {
                if let Some(hit) = self
                    .contextual_hits
                    .iter()
                    .find(|hit| hit.area.contains(position))
                {
                    Some(ClickTarget::Contextual(hit.control.clone()))
                } else if self
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
                } else if self.permission_choice_areas[0].contains(position) {
                    self.request_hit
                        .as_ref()
                        .map(|(target, request_id)| ClickTarget::Permission {
                            target: target.clone(),
                            request_id: request_id.clone(),
                            allow: false,
                        })
                } else if self.permission_choice_areas[1].contains(position) {
                    self.request_hit
                        .as_ref()
                        .map(|(target, request_id)| ClickTarget::Permission {
                            target: target.clone(),
                            request_id: request_id.clone(),
                            allow: true,
                        })
                } else if self.request_area.contains(position) {
                    self.request_hit
                        .as_ref()
                        .map(|(target, request_id)| ClickTarget::Request {
                            target: target.clone(),
                            request_id: request_id.clone(),
                        })
                } else if self.delivery_area.contains(position) {
                    let row = mouse.row.saturating_sub(self.delivery_area.y);
                    self.delivery_hits
                        .iter()
                        .find(|(range, _)| range.contains(&row))
                        .map(|(_, id)| ClickTarget::QueueInput(id.clone()))
                } else {
                    self.text_selection
                        .transcript_cell(mouse)
                        .map(|(row, column)| {
                            if let Some((id, action)) =
                                self.pane().transcript.rewind_group_action_at(row)
                            {
                                return ClickTarget::RewindGroup { id, action };
                            }
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
