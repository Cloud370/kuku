use crate::app::App;
use crate::jobs::Job;
use crate::menu::{Choice, ChoiceKind, MessageAction, PickerKind};
use crate::transcript::{MessageId, MessageInfo, MessageKind, SelectedMessage};

impl App {
    pub(super) fn open_messages_picker(&mut self) {
        self.open_messages_picker_at(None);
    }

    pub(super) fn back_to_messages(&mut self, selected: SelectedMessage) {
        self.open_messages_picker_at(Some(selected));
    }

    fn open_messages_picker_at(&mut self, previous: Option<SelectedMessage>) {
        let selection = self.selection();
        if previous
            .as_ref()
            .is_some_and(|message| message.selection != selection)
        {
            self.problem("Selected message is no longer available");
            self.modal = None;
            return;
        }
        let entries = self
            .pane()
            .transcript
            .message_entries(&selection.target.agent);
        let choices: Vec<_> = entries
            .into_iter()
            .map(|entry| {
                Choice::new(
                    entry.label,
                    entry.detail,
                    ChoiceKind::MessageEntry(SelectedMessage {
                        selection: selection.clone(),
                        id: entry.id,
                    }),
                )
            })
            .collect();
        let picker = self.picker(PickerKind::Messages, choices);
        picker.header = "Loaded active transcript only · older history is not loaded".into();
        if let Some(previous) = previous {
            if let Some(index) = picker
                .visible_choices()
                .iter()
                .position(|choice| matches!(&choice.kind, ChoiceKind::MessageEntry(message) if message.id == previous.id))
            {
                picker.selected = index;
            }
        }
    }

    pub(super) fn open_message_actions(&mut self, message: SelectedMessage) {
        let Some(info) = self.resolve_message(&message) else {
            self.problem("Selected message is no longer available");
            return;
        };
        let header = self
            .pane()
            .transcript
            .message_entries(&message.selection.target.agent)
            .into_iter()
            .find(|entry| entry.id == message.id)
            .map_or_else(
                || "Selected message".to_owned(),
                |entry| format!("Selected · {} · {}", entry.label, entry.detail),
            );
        let mut choices = vec![Choice::new(
            MessageAction::Copy.label(),
            "Copy this entry",
            ChoiceKind::MessageAction {
                message: message.clone(),
                action: MessageAction::Copy,
            },
        )];
        if info.kind == MessageKind::Assistant {
            choices.push(Choice::new(
                MessageAction::Source.label(),
                "Show Markdown source for this entry",
                ChoiceKind::MessageAction {
                    message: message.clone(),
                    action: MessageAction::Source,
                },
            ));
        }
        if info.thinking.is_some() {
            choices.push(Choice::new(
                MessageAction::Thinking.label(),
                "Open thinking associated with this entry",
                ChoiceKind::MessageAction {
                    message: message.clone(),
                    action: MessageAction::Thinking,
                },
            ));
        }
        if info.expandable.is_some() {
            choices.push(Choice::new(
                MessageAction::Expand.label(),
                "Expand details for this entry",
                ChoiceKind::MessageAction {
                    message: message.clone(),
                    action: MessageAction::Expand,
                },
            ));
        }
        if info
            .tool
            .as_ref()
            .is_some_and(|tool| tool.has_full_output())
        {
            choices.push(Choice::new(
                MessageAction::FullOutput.label(),
                "Open the complete tool output",
                ChoiceKind::MessageAction {
                    message,
                    action: MessageAction::FullOutput,
                },
            ));
        }
        self.picker(PickerKind::MessageActions, choices).header = header;
    }

    pub(super) fn execute_message_action(
        &mut self,
        message: SelectedMessage,
        action: MessageAction,
    ) {
        let Some(info) = self.resolve_message(&message) else {
            self.problem("Selected message is no longer available");
            return;
        };
        match action {
            MessageAction::Copy => self.copy_selected_message(message, info),
            MessageAction::Source if info.kind == MessageKind::Assistant => {
                if self.reveal_selected_message(&message) {
                    self.pane_mut().transcript.close_notes();
                    let showing_source = self
                        .pane_mut()
                        .transcript
                        .toggle_message_source(&message.id, &message.selection.target.agent);
                    if let Some(showing_source) = showing_source {
                        self.confirmation(if showing_source {
                            "Markdown source for selected message"
                        } else {
                            "Markdown preview"
                        });
                    } else {
                        self.problem("Selected message is no longer available");
                    }
                } else {
                    self.problem("Selected message is no longer available");
                }
            }
            MessageAction::Thinking => {
                let Some(key) = info.thinking else {
                    self.problem("Thinking for this message is no longer available");
                    return;
                };
                if self.reveal_selected_message(&message) {
                    self.project_selected_message();
                    if !self.toggle_transcript_detail(&key) {
                        self.problem("Thinking for this message is no longer available");
                    }
                } else {
                    self.problem("Selected message is no longer available");
                }
            }
            MessageAction::Expand => {
                let Some(key) = info.expandable else {
                    self.problem("Details for this message are no longer available");
                    return;
                };
                if self.reveal_selected_message(&message) {
                    self.project_selected_message();
                    if !self.toggle_transcript_detail(&key) {
                        self.problem("Details for this message are no longer available");
                    }
                } else {
                    self.problem("Selected message is no longer available");
                }
            }
            MessageAction::FullOutput => {
                let Some(tool) = info.tool.filter(|tool| tool.has_full_output()) else {
                    self.problem("Complete output for this tool is no longer available");
                    return;
                };
                self.show_output(tool);
            }
            MessageAction::Source => {
                self.problem("Source is only available for assistant messages");
            }
        }
    }

    fn copy_selected_message(&mut self, message: SelectedMessage, info: MessageInfo) {
        self.pane_mut().pending_reply_copy = None;
        self.pane_mut().pending_selected_reply_copy = None;
        if info.kind == MessageKind::Assistant && info.fetch_source {
            let MessageId::Event(id) = message.id else {
                self.problem("Selected message is no longer available");
                return;
            };
            if self.require_service() {
                let pane = self.pane_mut();
                pane.pending_reply_copy = Some(id);
                pane.pending_selected_reply_copy = Some(id);
                self.effects.push(Job::ReplySource(message.selection, id));
            }
            return;
        }
        self.copy_text(info.copy_text);
    }

    fn resolve_message(&self, message: &SelectedMessage) -> Option<MessageInfo> {
        if message.selection != self.selection() {
            return None;
        }
        self.pane()
            .transcript
            .message_info(&message.id, &message.selection.target.agent)
    }

    fn reveal_selected_message(&mut self, message: &SelectedMessage) -> bool {
        if message.selection != self.selection() {
            return false;
        }
        self.pane_mut()
            .transcript
            .reveal_message(&message.id, &message.selection.target.agent)
    }

    fn project_selected_message(&mut self) {
        let (width, height) = self.transcript_area;
        self.pane_mut().transcript.project(width, height);
    }
}
