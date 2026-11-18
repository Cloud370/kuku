use std::ops::Range;

use kuku_client::Target;

use crate::editor::Editor;
use crate::keys::Shortcut;
use crate::transcript::{AgentCardKey, ToolView};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandMenu {
    New,
    Resume,
    Agents,
    Queue,
    Withdraw,
    Advance,
    Interrupt,
    Reply,
    Pending,
    Model,
    Thinking,
    Copy,
    Details,
    Tools,
    Server,
    Reconnect,
    Restart,
    Web,
    Quit,
}

impl CommandMenu {
    pub(crate) const ALL: [Self; 19] = [
        Self::New,
        Self::Resume,
        Self::Agents,
        Self::Queue,
        Self::Withdraw,
        Self::Advance,
        Self::Interrupt,
        Self::Reply,
        Self::Pending,
        Self::Model,
        Self::Thinking,
        Self::Copy,
        Self::Details,
        Self::Tools,
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
            Self::Queue => "queue",
            Self::Withdraw => "withdraw",
            Self::Advance => "continue",
            Self::Interrupt => "interrupt",
            Self::Reply => "reply",
            Self::Pending => "pending",
            Self::Model => "model",
            Self::Thinking => "thinking",
            Self::Copy => "copy",
            Self::Details => "details",
            Self::Tools => "tools",
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
            Self::Agents => ("Choose or create a recipient", None),
            Self::Queue => ("Send on the next Turn", Some(Shortcut::NEXT_TURN)),
            Self::Withdraw => ("Withdraw input still in the queue", None),
            Self::Advance => ("Continue queued work", None),
            Self::Interrupt => ("Interrupt this execution", Some(Shortcut::ESCAPE)),
            Self::Reply => ("Answer or decide a pending request", Some(Shortcut::REPLY)),
            Self::Pending => ("Check, retry or discard unconfirmed commands", None),
            Self::Model => ("Choose a configured model route", Some(Shortcut::MODEL)),
            Self::Thinking => ("Choose the model's reasoning setting", None),
            Self::Copy => ("Copy the latest reply", None),
            Self::Details => {
                return format!(
                    "Choose a visible Agent card · {} toggles previews",
                    Shortcut::DETAILS.label
                );
            }
            Self::Tools => ("Browse tools and saved output", None),
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
        address: String,
    },
    CreateAgent,
    Tier(String),
    Connection(String),
    Thinking(Option<String>),
    Withdraw(String),
    Pending(String),
    PendingRetry(String),
    PendingCheck(String),
    PendingDiscard(String),
    PendingOpen {
        target: Target,
        exists: bool,
    },
    RestoreRejected(String),
    Permission {
        request_id: String,
        allow: bool,
    },
    AgentCard(AgentCardKey),
    Tool(ToolView),
    Restart,
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
    Queue,
    Pending,
    PendingItem,
    Permission,
    AgentDetails,
    Tools,
    Server,
}

impl PickerKind {
    pub(crate) fn title(&self) -> &'static str {
        match self {
            Self::Commands => "Actions and Skills",
            Self::Paths(_) => "Workspace paths",
            Self::Sessions => "Resume session",
            Self::Agents => "Select agent",
            Self::Models => "Model route",
            Self::Thinking => "Thinking",
            Self::Queue => "Withdraw queued input",
            Self::Pending => "Unconfirmed commands",
            Self::PendingItem => "Recover command",
            Self::Permission => "Authorization required",
            Self::AgentDetails => "Agent details",
            Self::Tools => "Tool details and saved output",
            Self::Server => "Service status",
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
                | Self::AgentDetails
                | Self::Tools
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
        }
    }

    pub(crate) fn begin_search(&mut self) -> Ticket {
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
                !matches!(self.kind, PickerKind::Agents | PickerKind::Models)
                    || choice.matches(&query)
            }))
            .collect();
        if matches!(self.kind, PickerKind::Commands) && !query.is_empty() {
            choices.sort_by_key(|choice| match choice.kind {
                ChoiceKind::Command(command) if command.name() == query => 0,
                ChoiceKind::Command(command) if command.name().starts_with(&query) => 1,
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
}

#[derive(Debug)]
pub(crate) enum PromptKind {
    Agent,
    Model { connection: String },
    Question { request_id: String },
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
            picker.move_by(1);
            assert_eq!("/reply", picker.visible_choices()[1].label);
            assert!(matches!(
                picker.selected_choice(),
                Some(ChoiceKind::Command(CommandMenu::Reply))
            ));
        }
    }
}
