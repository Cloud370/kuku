use std::collections::VecDeque;

use crate::jobs::Selection;
use crate::text::{shorten, single_line};

use super::{
    Anchor, AnchorLine, AssistantView, Body, DetailKey, Reading, ToolView, Transcript,
    DISPLAY_LIMIT_NOTICE,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum MessageId {
    Event(u64),
    Tool { turn: Option<u64>, call_id: String },
}

impl MessageId {
    fn from_key(key: &super::Key) -> Option<Self> {
        match key {
            super::Key::Event(id) => Some(Self::Event(*id)),
            super::Key::Tool { turn, id } => Some(Self::Tool {
                turn: *turn,
                call_id: id.clone(),
            }),
            super::Key::Preview(_) | super::Key::RewindGroup(_) | super::Key::Compaction(_) => None,
        }
    }

    fn key(&self) -> super::Key {
        match self {
            Self::Event(id) => super::Key::Event(*id),
            Self::Tool { turn, call_id } => super::Key::Tool {
                turn: *turn,
                id: call_id.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectedMessage {
    pub(crate) selection: Selection,
    pub(crate) id: MessageId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MessageKind {
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone)]
pub(crate) struct MessageEntry {
    pub(crate) id: MessageId,
    pub(crate) label: String,
    pub(crate) detail: String,
}

#[derive(Debug, Clone)]
pub(crate) struct MessageInfo {
    pub(crate) kind: MessageKind,
    pub(crate) copy_text: String,
    pub(crate) fetch_source: bool,
    pub(crate) thinking: Option<DetailKey>,
    pub(crate) expandable: Option<DetailKey>,
    pub(crate) outputs: Vec<ToolView>,
}

impl Transcript {
    pub(crate) fn message_entries(&self, agent: &str) -> Vec<MessageEntry> {
        let window = self
            .reading
            .as_ref()
            .map_or(&self.recent, |reading| &reading.window);
        window
            .records
            .iter()
            .rev()
            .filter_map(|record| {
                if record.owner.as_deref() != Some(agent) || !record.visible() {
                    return None;
                }
                let id = MessageId::from_key(&record.key)?;
                let (label, detail) = match &record.body {
                    Body::User { text, .. } => ("User".into(), text.clone()),
                    Body::Assistant(text) | Body::Interrupted(text) => {
                        let detail = if text.is_empty() {
                            "Thinking available".to_owned()
                        } else {
                            text.clone()
                        };
                        ("Assistant".into(), detail)
                    }
                    Body::Tool(tool) => {
                        let view = tool.view();
                        let label = if view.name.is_empty() {
                            "Tool".to_owned()
                        } else {
                            format!("Tool · {}", view.name)
                        };
                        let detail = if view.details.is_empty() {
                            view.summary
                        } else {
                            view.details
                        };
                        (label, detail)
                    }
                    Body::Job(job) => (job.label(), job.text.clone()),
                    Body::Notice(..)
                    | Body::ModelError { .. }
                    | Body::Retry { .. }
                    | Body::Compaction(_)
                    | Body::TurnSummary => return None,
                };
                let turn = record
                    .turn_number
                    .or(record.turn)
                    .map_or_else(String::new, |turn| format!(" · Turn {turn}"));
                let label = format!("{label}{turn}");
                let detail = shorten(&single_line(&detail), 96);
                Some(MessageEntry { id, label, detail })
            })
            .collect()
    }

    pub(crate) fn message_info(&self, id: &MessageId, agent: &str) -> Option<MessageInfo> {
        let key = id.key();
        let window = self
            .reading
            .as_ref()
            .map_or(&self.recent, |reading| &reading.window);
        let record = window.records.iter().find(|record| {
            record.key == key && record.owner.as_deref() == Some(agent) && record.visible()
        })?;
        let (kind, copy_text, outputs, fetch_source) = match &record.body {
            Body::User { text, .. } => (MessageKind::User, text.clone(), Vec::new(), false),
            Body::Assistant(text) => (
                MessageKind::Assistant,
                text.clone(),
                Vec::new(),
                text.ends_with(DISPLAY_LIMIT_NOTICE),
            ),
            Body::Interrupted(text) => (MessageKind::Assistant, text.clone(), Vec::new(), false),
            Body::Tool(tool) => {
                let view = tool.view();
                let text = if view.details.is_empty() {
                    view.summary.clone()
                } else {
                    view.details.clone()
                };
                let mut outputs = tool.output_views();
                if view.has_full_output() {
                    outputs.insert(0, view);
                }
                (MessageKind::Tool, text, outputs, false)
            }
            Body::Job(job) => (
                MessageKind::Tool,
                job.text.clone(),
                job.output().into_iter().collect(),
                false,
            ),
            Body::Notice(..)
            | Body::ModelError { .. }
            | Body::Retry { .. }
            | Body::Compaction(_)
            | Body::TurnSummary => return None,
        };
        let (thinking, expandable) = self.message_action_keys(record);
        Some(MessageInfo {
            kind,
            fetch_source,
            copy_text,
            thinking,
            expandable,
            outputs,
        })
    }

    pub(crate) fn reveal_message(&mut self, id: &MessageId, agent: &str) -> bool {
        let key = id.key();
        if self.message_info(id, agent).is_none() {
            return false;
        }
        if let Some(reading) = &mut self.reading {
            reading.anchor = Anchor {
                key,
                line: AnchorLine::Body(0),
            };
            reading.manual = true;
            return true;
        }
        self.reading = Some(Reading {
            window: self.recent.clone(),
            anchor: Anchor {
                key,
                line: AnchorLine::Body(0),
            },
            newer: VecDeque::new(),
            unseen: false,
            manual: true,
        });
        true
    }

    pub(crate) fn has_assistant_message(&self, id: u64, agent: &str) -> bool {
        let key = super::Key::Event(id);
        let window = self
            .reading
            .as_ref()
            .map_or(&self.recent, |reading| &reading.window);
        window.records.iter().any(|record| {
            record.key == key
                && record.owner.as_deref() == Some(agent)
                && matches!(&record.body, Body::Assistant(_))
        })
    }

    pub(crate) fn toggle_message_source(&mut self, id: &MessageId, agent: &str) -> Option<bool> {
        let MessageId::Event(event_id) = id else {
            return None;
        };
        if !self
            .message_info(id, agent)
            .is_some_and(|message| message.kind == MessageKind::Assistant)
        {
            return None;
        }
        self.assistant_view = if self.assistant_view == AssistantView::MessageSource(*event_id) {
            AssistantView::Preview
        } else {
            AssistantView::MessageSource(*event_id)
        };
        Some(self.assistant_view == AssistantView::MessageSource(*event_id))
    }

    pub(crate) fn show_message_preview(&mut self, id: &MessageId) {
        let MessageId::Event(event_id) = id else {
            return;
        };
        if self.assistant_view == AssistantView::MessageSource(*event_id) {
            self.assistant_view = AssistantView::Preview;
        }
    }

    #[cfg(test)]
    pub(crate) fn toggle_source(&mut self) -> bool {
        self.assistant_view = match self.assistant_view {
            AssistantView::Preview => AssistantView::Source,
            AssistantView::Source => AssistantView::Preview,
            AssistantView::MessageSource(_) => AssistantView::Source,
        };
        self.assistant_view == AssistantView::Source
    }

    #[cfg(test)]
    pub(crate) fn tools(&self) -> Vec<ToolView> {
        let window = self
            .reading
            .as_ref()
            .map_or(&self.recent, |reading| &reading.window);
        window
            .records
            .iter()
            .rev()
            .filter_map(|record| match &record.body {
                Body::Tool(tool) => Some(tool.view()),
                Body::User { .. }
                | Body::Assistant(_)
                | Body::Interrupted(_)
                | Body::Notice(..)
                | Body::Job(_)
                | Body::ModelError { .. }
                | Body::Retry { .. }
                | Body::Compaction(_)
                | Body::TurnSummary => None,
            })
            .collect()
    }
}
