use std::ops::Range;

use kuku_client::Target;
use ratatui::layout::{Position, Rect};

use crate::editor::Editor;
use crate::jobs::Selection;
use crate::keys::Shortcut;
use crate::transcript::{DetailKey, RewindGroupAction, SelectedMessage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandMenu {
    New,
    Resume,
    Agents,
    Model,
    Thinking,
    StatusLine,
    Server,
    Web,
    Quit,
}

impl CommandMenu {
    pub(crate) const ALL: &[Self] = &[
        Self::New,
        Self::Resume,
        Self::Agents,
        Self::Model,
        Self::Thinking,
        Self::StatusLine,
        Self::Server,
        Self::Web,
        Self::Quit,
    ];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Resume => "resume",
            Self::Agents => "agents",
            Self::Model => "model",
            Self::Thinking => "thinking",
            Self::StatusLine => "statusline",
            Self::Server => "server",
            Self::Web => "web",
            Self::Quit => "quit",
        }
    }

    fn description(self) -> String {
        let (description, shortcut) = match self {
            Self::New => ("Open a fresh draft", None),
            Self::Resume => ("Find a conversation in this workspace", None),
            Self::Agents => ("Choose or create an Agent in this Session", None),
            Self::Model => ("Choose a configured model route", Some(Shortcut::MODEL)),
            Self::Thinking => ("Choose the model's reasoning setting", None),
            Self::StatusLine => ("Choose and reorder status line fields", None),
            Self::Server => ("Inspect the connected service", None),
            Self::Web => ("Open this target in the browser", None),
            Self::Quit => ("Leave the terminal", None),
        };
        match shortcut {
            Some(shortcut) => format!("{description} · {}", shortcut.label),
            None => description.to_owned(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum ChoiceKind {
    Command(CommandMenu),
    Skill(String),
    Path(String),
    Session {
        id: String,
        title: String,
        exists: bool,
    },
    Agent {
        target: Target,
    },
    CreateAgent,
    Tier(String),
    Connection(String),
    Reconnect,
    Thinking(Option<String>),
    RewindMessage {
        event_id: u64,
        turn: u64,
        text: String,
    },
    MessageEntry(SelectedMessage),
    MessageAction {
        message: SelectedMessage,
        action: MessageAction,
    },
    RewindSection,
    RewindGroup {
        selection: Selection,
        rewind_event_id: u64,
        action: RewindGroupAction,
    },
    Pending(String),
    PendingRetry(String),
    PendingCheck(String),
    PendingDiscard(String),
    PendingOpen {
        target: Target,
        exists: bool,
    },
    RestoreRejected(String),
    Detail {
        key: DetailKey,
        message: Option<SelectedMessage>,
    },
    Restart,
}

impl ChoiceKind {
    fn opens_on_click(&self) -> bool {
        matches!(
            self,
            Self::Command(
                CommandMenu::Resume
                    | CommandMenu::Agents
                    | CommandMenu::Model
                    | CommandMenu::Thinking
                    | CommandMenu::StatusLine
                    | CommandMenu::Server
            ) | Self::Session { .. }
                | Self::Agent { .. }
                | Self::CreateAgent
                | Self::Connection(_)
                | Self::RewindMessage { .. }
                | Self::MessageEntry(_)
                | Self::MessageAction { .. }
                | Self::RewindGroup { .. }
                | Self::Pending(_)
                | Self::PendingOpen { .. }
                | Self::Detail { .. }
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MessageAction {
    Copy,
    Source,
    Thinking,
    Expand,
    FullOutput,
}

impl MessageAction {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Copy => "Copy",
            Self::Source => "Source",
            Self::Thinking => "Thinking",
            Self::Expand => "Expand",
            Self::FullOutput => "Full output",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Choice {
    pub(crate) label: String,
    pub(crate) detail: String,
    pub(crate) kind: ChoiceKind,
    search: String,
}

impl Choice {
    pub(crate) fn new(
        label: impl Into<String>,
        detail: impl Into<String>,
        kind: ChoiceKind,
    ) -> Self {
        let label = label.into();
        let detail = detail.into();
        let search = format!("{label} {detail}").to_lowercase();
        Self {
            label,
            detail,
            search,
            kind,
        }
    }

    pub(crate) fn commands() -> Vec<Self> {
        CommandMenu::ALL
            .iter()
            .map(|command| {
                Self::new(
                    format!("/{}", command.name()),
                    command.description(),
                    ChoiceKind::Command(*command),
                )
            })
            .collect()
    }

    fn matches(&self, query: &str) -> bool {
        query
            .split_whitespace()
            .all(|part| self.search.contains(part))
    }
}

#[derive(Debug, Clone)]
pub(crate) enum PickerKind {
    Commands,
    Paths(Range<usize>),
    Sessions,
    Agents,
    Models,
    Thinking,
    Pending,
    PendingItem,
    Server,
    Rewind,
    Messages,
    MessageActions,
}

impl PickerKind {
    pub(crate) fn title(&self) -> &'static str {
        match self {
            Self::Commands => "Actions and Skills",
            Self::Paths(_) => "Workspace paths",
            Self::Sessions => "Resume session",
            Self::Agents => "Switch Agent",
            Self::Models => "Model route",
            Self::Thinking => "Thinking",
            Self::Pending => "Unconfirmed commands",
            Self::PendingItem => "Recover command",
            Self::Server => "Service status",
            Self::Rewind => "Rewind to message",
            Self::Messages => "Operate on message",
            Self::MessageActions => "Message actions",
        }
    }

    pub(crate) fn searchable(&self) -> bool {
        matches!(
            self,
            Self::Commands | Self::Paths(_) | Self::Sessions | Self::Agents | Self::Models
        )
    }

    pub(crate) fn completes_text(&self) -> bool {
        matches!(self, Self::Commands | Self::Paths(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Ticket {
    pub(crate) id: u64,
    revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PickerHit {
    id: u64,
    revision: u64,
    index: usize,
}

#[derive(Debug)]
struct PickerRows {
    area: Rect,
    indices: Range<usize>,
    row_height: u16,
}

#[derive(Debug)]
pub(crate) struct Picker {
    pub(crate) kind: PickerKind,
    pub(crate) query: Editor,
    pub(crate) header: String,
    pub(crate) loading: bool,
    pub(crate) problem: Option<String>,
    pub(crate) selected: usize,
    pub(crate) header_top: usize,
    pub(crate) ticket: Ticket,
    local: Vec<Choice>,
    remote: Vec<Choice>,
    rows: Option<PickerRows>,
    row_revision: u64,
}

impl Picker {
    pub(crate) fn new(kind: PickerKind, id: u64, local: Vec<Choice>) -> Self {
        Self {
            kind,
            query: Editor::default(),
            header: String::new(),
            loading: false,
            problem: None,
            selected: 0,
            header_top: 0,
            ticket: Ticket { id, revision: 0 },
            local,
            remote: Vec::new(),
            rows: None,
            row_revision: 0,
        }
    }

    pub(crate) fn begin_search(&mut self) -> Ticket {
        self.invalidate_rows();
        self.loading = true;
        self.problem = None;
        self.remote.clear();
        self.selected = 0;
        self.ticket.revision = self.ticket.revision.wrapping_add(1);
        self.ticket
    }

    pub(crate) fn set_results(&mut self, items: Vec<Choice>, header: String) {
        self.remote = items;
        self.loading = false;
        self.problem = None;
        self.clamp_selection();
        if !header.is_empty() {
            self.header = header;
        }
    }

    pub(crate) fn set_local(&mut self, choices: Vec<Choice>) {
        self.local = choices;
        self.clamp_selection();
    }

    pub(crate) fn move_by(&mut self, amount: isize) {
        let choices = self.visible_choices();
        if choices.is_empty() {
            self.selected = 0;
            return;
        }
        let direction = amount.signum();
        let mut selected = self.selected.min(choices.len() - 1) as isize;
        for _ in 0..amount.unsigned_abs().min(choices.len()) {
            let mut candidate = selected.saturating_add(direction);
            while candidate >= 0 && candidate < choices.len() as isize {
                if Self::selectable(choices[candidate as usize]) {
                    selected = candidate;
                    break;
                }
                candidate = candidate.saturating_add(direction);
            }
        }
        self.selected = selected as usize;
        self.invalidate_rows();
    }

    pub(crate) fn cycle_by(&mut self, amount: isize) {
        let choices = self.visible_choices();
        let selectable: Vec<_> = choices
            .iter()
            .enumerate()
            .filter_map(|(index, choice)| Self::selectable(choice).then_some(index))
            .collect();
        if selectable.is_empty() {
            return;
        }
        let position = selectable
            .iter()
            .position(|index| *index == self.selected)
            .unwrap_or(0) as isize;
        let count = selectable.len() as isize;
        let next = (position + amount.rem_euclid(count)).rem_euclid(count) as usize;
        self.selected = selectable[next];
        self.invalidate_rows();
    }

    pub(crate) fn clamp_selection(&mut self) {
        self.invalidate_rows();
        let choices = self.visible_choices();
        if choices.is_empty() {
            self.selected = 0;
            return;
        }
        let selected = self.selected.min(choices.len() - 1);
        let selected = if !Self::selectable(choices[selected]) {
            (selected + 1..choices.len())
                .find(|index| Self::selectable(choices[*index]))
                .or_else(|| {
                    (0..selected)
                        .rev()
                        .find(|index| Self::selectable(choices[*index]))
                })
                .unwrap_or(0)
        } else {
            selected
        };
        drop(choices);
        self.selected = selected;
    }

    pub(crate) fn choice_count(&self) -> usize {
        self.local.len() + self.remote.len()
    }

    pub(crate) fn visible_choices(&self) -> Vec<&Choice> {
        let query = self
            .query
            .text()
            .trim()
            .trim_start_matches(match self.kind {
                PickerKind::Commands => '/',
                _ => '\0',
            })
            .to_lowercase();
        let mut choices: Vec<_> = self
            .local
            .iter()
            .filter(|choice| choice.matches(&query))
            .chain(self.remote.iter().filter(|choice| {
                !matches!(self.kind, PickerKind::Models) || choice.matches(&query)
            }))
            .collect();
        if matches!(self.kind, PickerKind::Commands | PickerKind::Agents) && !query.is_empty() {
            choices.sort_by_key(|choice| match choice.kind {
                ChoiceKind::Command(command) if command.name() == query => 0,
                ChoiceKind::Command(command) if command.name().starts_with(&query) => 1,
                ChoiceKind::Agent { ref target } if target.agent == query => 0,
                ChoiceKind::Agent { ref target } if target.agent.starts_with(&query) => 1,
                _ => 2,
            });
        }
        choices
    }

    pub(crate) fn selected_choice(&self) -> Option<ChoiceKind> {
        self.visible_choices()
            .get(self.selected)
            .filter(|choice| Self::selectable(choice))
            .map(|choice| choice.kind.clone())
    }

    fn selectable(choice: &Choice) -> bool {
        !matches!(&choice.kind, ChoiceKind::RewindSection)
    }

    pub(crate) fn set_rows(&mut self, area: Rect, indices: Range<usize>, row_height: u16) {
        self.rows = Some(PickerRows {
            area,
            indices,
            row_height,
        });
    }

    pub(crate) fn hit(&self, point: Position) -> Option<PickerHit> {
        let rows = self.rows.as_ref()?;
        if !rows.area.contains(point) {
            return None;
        }
        let index = rows.indices.start + usize::from((point.y - rows.area.y) / rows.row_height);
        rows.indices.contains(&index).then_some(PickerHit {
            id: self.ticket.id,
            revision: self.row_revision,
            index,
        })
    }

    pub(crate) fn click(&mut self, hit: PickerHit) -> Option<ChoiceKind> {
        if hit.id != self.ticket.id || hit.revision != self.row_revision {
            return None;
        }
        let choice = &self.visible_choices().get(hit.index)?.kind;
        if matches!(choice, ChoiceKind::RewindSection) {
            return None;
        }
        let opened = choice.opens_on_click().then(|| choice.clone());
        self.selected = hit.index;
        opened
    }

    fn invalidate_rows(&mut self) {
        self.rows = None;
        self.row_revision = self.row_revision.wrapping_add(1);
    }
}

#[derive(Debug)]
pub(crate) enum PromptKind {
    Agent,
    Model { connection: String },
    Question { request_id: String, target: Target },
    QueuedInput { input_command_id: String },
    RewindMessage { event_id: u64, text: String },
}

impl PromptKind {
    pub(crate) fn multiline(&self) -> bool {
        matches!(self, Self::Question { .. } | Self::QueuedInput { .. })
    }
}

#[derive(Debug)]
pub(crate) struct Prompt {
    pub(crate) kind: PromptKind,
    pub(crate) title: String,
    pub(crate) instructions: String,
    pub(crate) instructions_top: usize,
    pub(crate) editor: Editor,
}

#[cfg(test)]
mod tests {
    use super::Choice;

    #[test]
    fn action_picker_omits_contextual_and_inline_controls() {
        let choices = Choice::commands();
        let labels = choices
            .iter()
            .map(|choice| choice.label.as_str())
            .collect::<Vec<_>>();
        for omitted in [
            "/back",
            "/continue",
            "/interrupt",
            "/pending",
            "/reconnect",
            "/restart",
            "/queue",
            "/queued",
            "/withdraw",
            "/reply",
        ] {
            assert!(!labels.contains(&omitted));
        }
        for retained in ["/new", "/resume", "/server", "/quit"] {
            assert!(labels.contains(&retained));
        }
        assert_eq!(9, choices.len());
    }
}
