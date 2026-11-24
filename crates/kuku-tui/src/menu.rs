use std::ops::Range;

use kuku_client::{JobSnapshot, NamedValue, Target};
use ratatui::layout::{Position, Rect};

use crate::editor::Editor;
use crate::jobs::Selection;
use crate::transcript::{DetailKey, RewindGroupAction, SelectedMessage, ToolView};

mod commands;

pub(crate) use commands::{CommandInput, CommandMenu};

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
    AgentSetup(crate::app::agent_setup::AgentSetupAction),
    ModelRoute {
        tier: Box<kuku_client::TierSummary>,
        current: bool,
    },
    EditModel(crate::setup::SetupIntent),
    Close,
    Reconnect,
    SetCurrentModelAsDefault,
    ConfirmModelDefault,
    CancelModelDefault,
    Thinking(Option<String>),
    Speed(crate::app::speed::SpeedAction),
    RewindMessage {
        event_id: u64,
        turn: u64,
        text: String,
    },
    HistoryJumpLatest,
    HistoryJumpReview(kuku_client::HistoryDestination),
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
    Tool(ToolView),
    BackgroundJob(Box<JobSnapshot>),
    CancelBackground(String),
    Value(NamedValue),
    AttachValue(NamedValue),
    ReadValue(NamedValue),
    ClearValues,
    SendImages {
        original: bool,
        timing: kuku_client::Timing,
        route: Option<Box<crate::app::send_route::RouteChange>>,
    },
    ConfirmRestart(Box<kuku_client::local::ServiceIdentity>),
}

impl ChoiceKind {
    fn opens_on_click(&self) -> bool {
        matches!(
            self,
            Self::Command(
                CommandMenu::Resume
                    | CommandMenu::Rewind
                    | CommandMenu::Agents
                    | CommandMenu::Jobs
                    | CommandMenu::Model
                    | CommandMenu::Thinking
                    | CommandMenu::Speed
                    | CommandMenu::StatusLine
                    | CommandMenu::Save
                    | CommandMenu::Values
                    | CommandMenu::Server
                    | CommandMenu::Restart
                    | CommandMenu::Help
            ) | Self::Session { .. }
                | Self::Agent { .. }
                | Self::CreateAgent
                | Self::AgentSetup(_)
                | Self::ModelRoute { .. }
                | Self::Speed(_)
                | Self::EditModel(_)
                | Self::Close
                | Self::ConfirmRestart(_)
                | Self::SetCurrentModelAsDefault
                | Self::ConfirmModelDefault
                | Self::CancelModelDefault
                | Self::RewindMessage { .. }
                | Self::MessageEntry(_)
                | Self::MessageAction { .. }
                | Self::RewindGroup { .. }
                | Self::Pending(_)
                | Self::PendingOpen { .. }
                | Self::Detail { .. }
                | Self::Tool(_)
                | Self::BackgroundJob(_)
                | Self::Value(_)
                | Self::AttachValue(_)
                | Self::ReadValue(_)
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PickerKind {
    Commands,
    Paths(Range<usize>),
    Sessions,
    Agents,
    AgentSetup(crate::app::agent_setup::AgentSetupStep),
    Models,
    ModelDefault,
    ModelActions,
    Restart,
    Thinking,
    Speed,
    CustomSpeed,
    Pending,
    PendingItem,
    Tools,
    Jobs,
    JobActions,
    Values,
    ValueActions,
    Images,
    Server,
    Rewind,
    HistoryJump,
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
            Self::AgentSetup(step) => step.title(),
            Self::Models => "Model route",
            Self::ModelDefault => "Set current model as default",
            Self::ModelActions => "Model settings",
            Self::Restart => "Restart kuku service",
            Self::Thinking => "Thinking",
            Self::Speed => "Service tier",
            Self::CustomSpeed => "Custom service tier",
            Self::Pending => "Unconfirmed commands",
            Self::PendingItem => "Recover command",
            Self::Tools => "Saved outputs",
            Self::Jobs => "Background jobs",
            Self::JobActions => "Background job",
            Self::Values => "Saved values",
            Self::Images => "Image quality",
            Self::ValueActions => "Saved value",
            Self::Server => "Service status",
            Self::Rewind => "Rewind to message",
            Self::HistoryJump => "Conversation history",
            Self::Messages => "Operate on message",
            Self::MessageActions => "Message actions",
        }
    }

    pub(crate) fn searchable(&self) -> bool {
        matches!(
            self,
            Self::Commands
                | Self::Paths(_)
                | Self::Sessions
                | Self::Agents
                | Self::AgentSetup(crate::app::agent_setup::AgentSetupStep::Model)
                | Self::Models
                | Self::Speed
                | Self::Tools
                | Self::Jobs
                | Self::Values
        )
    }

    pub(crate) fn completes_text(&self) -> bool {
        matches!(self, Self::Commands | Self::Paths(_) | Self::Speed)
    }

    pub(crate) fn composer_anchored(&self) -> bool {
        matches!(
            self,
            Self::Commands
                | Self::Thinking
                | Self::Models
                | Self::ModelActions
                | Self::Images
                | Self::Speed
                | Self::CustomSpeed
        )
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
    pub(crate) image_auto_send: Option<crate::app::images::ImageAutoSend>,
    pub(crate) query: Editor,
    pub(crate) header: String,
    pub(crate) loading: bool,
    pub(crate) problem: Option<String>,
    pub(crate) selected: usize,
    pub(crate) header_top: usize,
    pub(crate) search_focused: bool,
    pub(crate) ticket: Ticket,
    pub(crate) rewind_history: Option<RewindHistory>,
    pub(crate) thinking: Option<Box<crate::app::ThinkingPicker>>,
    pub(crate) speed: Option<Box<crate::app::speed::SpeedTarget>>,
    pub(crate) agent_setup: Option<Box<crate::app::agent_setup::AgentSetup>>,
    local: Vec<Choice>,
    remote: Vec<Choice>,
    rows: Option<PickerRows>,
    row_revision: u64,
    top: usize,
}

impl Picker {
    pub(crate) fn new(kind: PickerKind, id: u64, local: Vec<Choice>) -> Self {
        Self {
            kind,
            image_auto_send: None,
            query: Editor::default(),
            header: String::new(),
            loading: false,
            problem: None,
            selected: 0,
            header_top: 0,
            search_focused: false,
            ticket: Ticket { id, revision: 0 },
            rewind_history: None,
            thinking: None,
            speed: None,
            agent_setup: None,
            local,
            remote: Vec::new(),
            rows: None,
            row_revision: 0,
            top: 0,
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
        if self.kind == PickerKind::Models && self.query.text().is_empty() {
            if let Some(index) = self.visible_choices().iter().position(|choice| {
                matches!(choice.kind, ChoiceKind::ModelRoute { current: true, .. })
            }) {
                self.selected = index;
            }
        }
        if !header.is_empty() {
            self.header = header;
        }
    }

    pub(crate) fn set_local(&mut self, choices: Vec<Choice>) {
        self.local = choices;
        self.clamp_selection();
    }

    pub(crate) fn set_older_choices(&mut self, choices: Vec<Choice>, anchor: usize) {
        self.top = self
            .top
            .saturating_add(anchor.saturating_sub(self.selected));
        self.set_local(choices);
        self.selected = anchor;
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
        let query = self.query.text().trim().to_lowercase();
        let command_query =
            (self.kind == PickerKind::Commands).then(|| CommandInput::query(&query));
        let query = command_query.map_or(query.as_str(), |input| input.text());
        let exact_command = command_query.and_then(CommandInput::command);
        let local = self
            .local
            .iter()
            .filter(|choice| match (&choice.kind, command_query) {
                (ChoiceKind::Command(command), Some(_)) if !command.advertised() => {
                    exact_command == Some(*command)
                }
                (ChoiceKind::Command(command), Some(input)) => exact_command.map_or_else(
                    || command.starts_with(input.name) || choice.matches(query),
                    |exact| exact == *command,
                ),
                _ => choice.matches(query),
            });
        let remote = self.remote.iter().filter(|choice| {
            !matches!(
                self.kind,
                PickerKind::Models | PickerKind::Values | PickerKind::Jobs
            ) || choice.matches(query)
        });
        let mut choices: Vec<_> = if self.kind == PickerKind::Models {
            remote.chain(local).collect()
        } else {
            local.chain(remote).collect()
        };
        if matches!(self.kind, PickerKind::Commands | PickerKind::Agents) && !query.is_empty() {
            choices.sort_by_key(|choice| match choice.kind {
                ChoiceKind::Command(command) if Some(command) == exact_command => 0,
                ChoiceKind::Command(command)
                    if command_query.is_some_and(|input| command.starts_with(input.name)) =>
                {
                    1
                }
                ChoiceKind::Agent { ref target } if target.agent == query => 0,
                ChoiceKind::Agent { ref target } if target.agent.starts_with(query) => 1,
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

    pub(crate) fn composer_rows(&mut self, count: usize, len: usize) -> Range<usize> {
        let selected = self.selected.min(len.saturating_sub(1));
        self.top = self
            .top
            .min(len.saturating_sub(count))
            .max(selected.saturating_add(1).saturating_sub(count))
            .min(selected);
        self.top..(self.top + count).min(len)
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
pub(crate) struct RewindHistory {
    pub(crate) selection: Selection,
    pub(crate) next_before: Option<u64>,
    pub(crate) load: RewindLoad,
    pub(crate) window_limited: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RewindLoad {
    Idle,
    Page {
        before: Option<u64>,
        pending_up: usize,
    },
    Source {
        event_id: u64,
        turn: u64,
    },
}

#[derive(Debug)]
pub(crate) enum PromptKind {
    Agent,
    Question {
        request_id: String,
        target: Target,
    },
    QueuedInput {
        input_command_id: String,
    },
    RewindMessage {
        event_id: u64,
        text: String,
        images: Vec<kuku_client::ImageAttachment>,
        restore_files: bool,
    },
    HistoryJump(Box<crate::app::history_jump::HistoryJumpPrompt>),
    SavePaste,
    SaveValue {
        output_id: Option<String>,
    },
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
    use super::{Choice, CommandInput, Picker, PickerKind};

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
            "/reload",
            "/queue",
            "/queued",
            "/withdraw",
            "/reply",
        ] {
            assert!(!labels.contains(&omitted));
        }
        for retained in [
            "/new",
            "/resume",
            "/rewind",
            "/compact",
            "/speed",
            "/fast",
            "/ultrafast",
            "/notifications",
            "/permissions",
            "/server",
            "/restart",
            "/quit",
        ] {
            assert!(labels.contains(&retained));
        }
        assert_eq!(21, choices.len());
    }

    #[test]
    fn aliases_find_exactly_one_canonical_action() {
        let mut picker = Picker::new(PickerKind::Commands, 1, Choice::commands());
        assert!(
            !picker
                .visible_choices()
                .iter()
                .any(|choice| choice.label == "/web")
        );
        for (alias, canonical) in [
            ("new-session", "/new"),
            ("sessions", "/resume"),
            ("backtrack", "/rewind"),
            ("agent", "/agents"),
            ("subagents", "/agents"),
            ("job", "/jobs"),
            ("ps", "/jobs"),
            ("models", "/model"),
            ("think", "/thinking"),
            ("effort", "/thinking"),
            ("status-line", "/statusline"),
            ("save-value", "/save"),
            ("value", "/values"),
            ("saved-values", "/values"),
            ("service", "/server"),
            ("restart-service", "/restart"),
            ("browser", "/web"),
            ("hotkeys", "/help"),
            ("exit", "/quit"),
            ("q", "/quit"),
        ] {
            let spelling = format!("/{alias}");
            picker.query.set_text(spelling.clone());
            let results = picker.visible_choices();
            assert_eq!(1, results.len(), "{alias}");
            assert_eq!(canonical, results[0].label);
            let input = CommandInput::draft(&spelling).unwrap();
            assert_eq!(canonical, input.complete(input.command().unwrap()));
        }
        assert!(CommandInput::draft("/reload").unwrap().command().is_none());
    }
}
