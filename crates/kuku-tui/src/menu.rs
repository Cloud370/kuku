use std::ops::Range;

use kuku_client::{JobSnapshot, NamedValue, Target};
use ratatui::layout::{Position, Rect};

use crate::editor::Editor;
use crate::keys::Shortcut;
use crate::transcript::{DetailKey, ToolView};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandMenu {
    New,
    Resume,
    Agents,
    Back,
    Advance,
    Interrupt,
    Jobs,
    Pending,
    Model,
    Thinking,
    StatusLine,
    Copy,
    Source,
    Notes,
    Details,
    Tools,
    Save,
    Values,
    Server,
    Reconnect,
    Restart,
    Web,
    Quit,
}

impl CommandMenu {
    pub(crate) const ALL: &[Self] = &[
        Self::New,
        Self::Resume,
        Self::Agents,
        Self::Back,
        Self::Advance,
        Self::Interrupt,
        Self::Jobs,
        Self::Pending,
        Self::Model,
        Self::Thinking,
        Self::StatusLine,
        Self::Copy,
        Self::Source,
        Self::Notes,
        Self::Details,
        Self::Tools,
        Self::Save,
        Self::Values,
        Self::Server,
        Self::Reconnect,
        Self::Restart,
        Self::Web,
        Self::Quit,
    ];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Resume => "resume",
            Self::Agents => "agents",
            Self::Back => "back",
            Self::Advance => "continue",
            Self::Interrupt => "interrupt",
            Self::Jobs => "jobs",
            Self::Pending => "pending",
            Self::Model => "model",
            Self::Thinking => "thinking",
            Self::StatusLine => "statusline",
            Self::Copy => "copy",
            Self::Source => "source",
            Self::Notes => "notes",
            Self::Details => "details",
            Self::Tools => "tools",
            Self::Save => "save",
            Self::Values => "values",
            Self::Server => "server",
            Self::Reconnect => "reconnect",
            Self::Restart => "restart",
            Self::Web => "web",
            Self::Quit => "quit",
        }
    }

    fn description(self) -> String {
        let (description, shortcut) = match self {
            Self::New => ("Open a fresh draft", None),
            Self::Resume => ("Find a conversation in this workspace", None),
            Self::Agents => ("Choose or create an Agent in this Session", None),
            Self::Back => ("Return to the previous Agent in this Session", None),
            Self::Advance => ("Continue queued work", None),
            Self::Interrupt => ("Interrupt this execution", Some(Shortcut::ESCAPE)),
            Self::Jobs => ("List background jobs · /jobs cancel <id>", None),
            Self::Pending => ("Check, retry or discard unconfirmed commands", None),
            Self::Model => ("Choose a configured model route", Some(Shortcut::MODEL)),
            Self::Thinking => ("Choose the model's reasoning setting", None),
            Self::StatusLine => ("Choose and reorder status line fields", None),
            Self::Copy => ("Copy the latest reply", None),
            Self::Source => (
                "Switch between Markdown preview and source for this Agent",
                None,
            ),
            Self::Notes => ("Read thinking in the conversation", None),
            Self::Details => {
                return format!(
                    "Inspect visible tools and Agent cards · {} toggles previews",
                    Shortcut::DETAILS.label
                );
            }
            Self::Tools => ("Browse tools and saved output", None),
            Self::Save => ("Name pasted text or the current draft", None),
            Self::Values => ("Browse and attach saved values", None),
            Self::Server => ("Inspect the connected service", None),
            Self::Reconnect => (
                "Reconnect and check unconfirmed commands",
                Some(Shortcut::RECONNECT),
            ),
            Self::Restart => ("Restart the managed local service", None),
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
    SetCurrentModelAsDefault,
    ConfirmModelDefault,
    CancelModelDefault,
    Thinking(Option<String>),
    RewindMessage {
        event_id: u64,
        turn: u64,
        text: String,
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
    Detail(DetailKey),
    Tool(ToolView),
    BackgroundJob(Box<JobSnapshot>),
    CancelBackground(String),
    Value(NamedValue),
    AttachValue(NamedValue),
    ReadValue(NamedValue),
    ClearValues,
    Restart,
}

impl ChoiceKind {
    fn opens_on_click(&self) -> bool {
        matches!(
            self,
            Self::Command(
                CommandMenu::Resume
                    | CommandMenu::Agents
                    | CommandMenu::Back
                    | CommandMenu::Jobs
                    | CommandMenu::Pending
                    | CommandMenu::Model
                    | CommandMenu::Thinking
                    | CommandMenu::StatusLine
                    | CommandMenu::Notes
                    | CommandMenu::Source
                    | CommandMenu::Details
                    | CommandMenu::Tools
                    | CommandMenu::Save
                    | CommandMenu::Values
                    | CommandMenu::Server
            ) | Self::Session { .. }
                | Self::Agent { .. }
                | Self::CreateAgent
                | Self::Connection(_)
                | Self::SetCurrentModelAsDefault
                | Self::ConfirmModelDefault
                | Self::CancelModelDefault
                | Self::RewindMessage { .. }
                | Self::Pending(_)
                | Self::PendingOpen { .. }
                | Self::Detail(_)
                | Self::Tool(_)
                | Self::BackgroundJob(_)
                | Self::Value(_)
                | Self::AttachValue(_)
                | Self::ReadValue(_)
        )
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
    ModelDefault,
    Thinking,
    Pending,
    PendingItem,
    Details,
    Tools,
    Jobs,
    JobActions,
    Values,
    ValueActions,
    Server,
    Rewind,
}

impl PickerKind {
    pub(crate) fn title(&self) -> &'static str {
        match self {
            Self::Commands => "Actions and Skills",
            Self::Paths(_) => "Workspace paths",
            Self::Sessions => "Resume session",
            Self::Agents => "Switch Agent",
            Self::Models => "Model route",
            Self::ModelDefault => "Set current model as default",
            Self::Thinking => "Thinking",
            Self::Pending => "Unconfirmed commands",
            Self::PendingItem => "Recover command",
            Self::Details => "Transcript details",
            Self::Tools => "Tool details and saved output",
            Self::Jobs => "Background jobs",
            Self::JobActions => "Background job",
            Self::Values => "Saved values",
            Self::ValueActions => "Saved value",
            Self::Server => "Service status",
            Self::Rewind => "Rewind to message",
        }
    }

    pub(crate) fn searchable(&self) -> bool {
        matches!(
            self,
            Self::Commands
                | Self::Paths(_)
                | Self::Sessions
                | Self::Agents
                | Self::Models
                | Self::Details
                | Self::Tools
                | Self::Jobs
                | Self::Values
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
        self.selected = self.selected.saturating_add_signed(amount);
        self.clamp_selection();
    }

    pub(crate) fn clamp_selection(&mut self) {
        self.invalidate_rows();
        self.selected = self
            .selected
            .min(self.visible_choices().len().saturating_sub(1));
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
                !matches!(
                    self.kind,
                    PickerKind::Models | PickerKind::Values | PickerKind::Jobs
                ) || choice.matches(&query)
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
            .map(|choice| choice.kind.clone())
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
    SavePaste,
    SaveValue { output_id: Option<String> },
}

impl PromptKind {
    pub(crate) fn multiline(&self) -> bool {
        matches!(
            self,
            Self::Question { .. } | Self::QueuedInput { .. } | Self::SavePaste
        )
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
    use super::{Choice, ChoiceKind, CommandMenu, Picker, PickerKind};

    #[test]
    fn slash_command_names_rank_before_description_matches() {
        let mut picker = Picker::new(PickerKind::Commands, 0, Choice::commands());
        for query in ["/pend", "/pending"] {
            picker.query.set_text(query.into());
            picker.selected = 0;
            assert_eq!("/pending", picker.visible_choices()[0].label);
            assert!(matches!(
                picker.selected_choice(),
                Some(ChoiceKind::Command(CommandMenu::Pending))
            ));
        }
    }

    #[test]
    fn action_picker_omits_inline_queue_and_reply_controls_only() {
        let choices = Choice::commands();
        let labels = choices
            .iter()
            .map(|choice| choice.label.as_str())
            .collect::<Vec<_>>();
        for omitted in ["/queue", "/queued", "/withdraw", "/reply"] {
            assert!(!labels.contains(&omitted));
        }
        for retained in ["/new", "/resume", "/continue", "/pending", "/quit"] {
            assert!(labels.contains(&retained));
        }
        assert_eq!(23, choices.len());
    }
}
