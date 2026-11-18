use std::ops::Range;

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
};
use kuku_client::{Action, AgentConfig, Timing};
use ratatui::buffer::Buffer;

use crate::app::{Address, App, Modal, PendingItem, PendingStage};
use crate::editor::Editor;
use crate::jobs::{Job, Search};
use crate::keys::Shortcut;
use crate::menu::{Choice, ChoiceKind, CommandMenu, Picker, PickerKind, Prompt, PromptKind};
use crate::output::OutputViewer;
use crate::selection::MouseAction;
use crate::text::{shorten, single_line, TAB};
use crate::transcript::ToolView;

const MOUSE_SCROLL_LINES: isize = 3;

impl App {
    pub(crate) fn event(&mut self, event: Event, screen: &Buffer) {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => self.key(key),
            Event::Paste(text) => self.paste(&text),
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::ScrollUp,
                ..
            }) => self.wheel(-MOUSE_SCROLL_LINES),
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::ScrollDown,
                ..
            }) => self.wheel(MOUSE_SCROLL_LINES),
            Event::Mouse(mouse) => {
                let agent = match mouse.kind {
                    MouseEventKind::Down(_) | MouseEventKind::Up(_) => {
                        self.text_selection.transcript_row(mouse).and_then(|row| {
                            self.panes
                                .get(&self.active)?
                                .transcript
                                .visible_agent_at(row)
                        })
                    }
                    _ => None,
                };
                match self.text_selection.mouse(mouse, screen, agent) {
                    Some(MouseAction::Copy(text)) => self.effects.push(Job::Copy(text)),
                    Some(MouseAction::ToggleAgent(row)) => {
                        self.pane_mut().transcript.toggle_agent_at(row);
                    }
                    None => {}
                }
            }
            Event::FocusLost | Event::Resize(_, _) => self.text_selection.clear(),
            Event::FocusGained | Event::Key(_) => {}
        }
    }

    pub(crate) fn sync_pending_picker(&mut self) {
        if matches!(&self.modal, Some(Modal::Picker(picker)) if matches!(picker.kind, PickerKind::Pending))
        {
            let local = self.pending_choices();
            if let Some(Modal::Picker(picker)) = &mut self.modal {
                picker.set_local(local);
            }
        }
    }

    fn dismiss_confirmation(&mut self) {
        if self
            .notice
            .as_ref()
            .is_some_and(|notice| notice.transient())
        {
            self.notice = None;
        }
    }

    fn key(&mut self, key: KeyEvent) {
        self.dismiss_confirmation();
        self.text_selection.clear();
        if Shortcut::ESCAPE.matches(key) && self.modal.take().is_some() {
            self.effects.push(Job::ClosePicker);
            return;
        }
        match self.modal {
            Some(Modal::Picker(_)) => self.picker_key(key),
            Some(Modal::Prompt(_)) => self.prompt_key(key),
            Some(Modal::Output(_)) => self.output_key(key),
            None => self.composer_key(key),
        }
    }

    fn paste(&mut self, text: &str) {
        self.dismiss_confirmation();
        self.text_selection.clear();
        if let Some(Modal::Picker(picker)) = &mut self.modal {
            if picker.kind.searchable() {
                picker.query.insert(&text.replace(['\r', '\n'], " "));
                picker.selected = 0;
                self.search(false);
            }
        } else if let Some(Modal::Prompt(prompt)) = &mut self.modal {
            if matches!(prompt.kind, PromptKind::Question { .. }) {
                prompt.editor.insert(text);
            } else {
                prompt.editor.insert(&text.replace(['\r', '\n'], " "));
            }
        } else if self.modal.is_none() {
            self.pane_mut().draft.insert(text);
        }
    }

    fn wheel(&mut self, amount: isize) {
        self.text_selection.clear();
        match &mut self.modal {
            Some(Modal::Output(viewer)) => {
                if let Some(offset) = viewer.scroll(amount) {
                    self.read_output(offset);
                }
            }
            Some(Modal::Picker(picker)) => {
                if matches!(picker.kind, PickerKind::Server | PickerKind::Permission) {
                    picker.header_top = picker.header_top.saturating_add_signed(amount);
                } else {
                    picker.move_by(amount);
                }
            }
            Some(Modal::Prompt(prompt)) => {
                prompt.instructions_top = prompt.instructions_top.saturating_add_signed(amount)
            }
            None => self.scroll(amount),
        }
    }

    fn composer_key(&mut self, key: KeyEvent) {
        let page = self.transcript_area.1.saturating_sub(1).max(1) as isize;
        match key {
            key if Shortcut::ACTIONS.matches(key) || Shortcut::ACTIONS_ALIAS.matches(key) => {
                self.command_menu()
            }
            key if Shortcut::REPLY.matches(key) => self.open_reply(),
            key if Shortcut::ESCAPE.matches(key) => self.interrupt(),
            key if Shortcut::NEXT_TURN.matches(key) => self.send(Timing::NextTurn),
            key if Shortcut::ENTER.matches(key) => self.send(Timing::Steer),
            key if Shortcut::PAGE_UP.matches(key) => self.scroll(-page),
            key if Shortcut::PAGE_DOWN.matches(key) => self.scroll(page),
            key if Shortcut::LATEST.matches(key) => self.pane_mut().transcript.show_latest(),
            key if Shortcut::RECONNECT.matches(key) => self.reconnect(),
            key if Shortcut::MODEL.matches(key) => self.perform(CommandMenu::Model),
            key if Shortcut::DETAILS.matches(key) => {
                let expanded = self.pane_mut().transcript.toggle_details();
                self.confirmation(if expanded {
                    "Details expanded"
                } else {
                    "Details collapsed"
                });
            }
            key if key.code == KeyCode::Char('/')
                && is_text(key)
                && self.pane().draft.text().is_empty() =>
            {
                self.command_menu()
            }
            key if key.code == KeyCode::Char('@')
                && is_text(key)
                && self.pane().draft.completion_range().is_empty() =>
            {
                let offset = self.pane().draft.cursor();
                self.open_paths(offset..offset, String::new());
            }
            key if Shortcut::TAB.matches(key) => {
                let draft = &self.pane().draft;
                let range = draft.completion_range();
                let query = draft.text()[range.clone()]
                    .trim_start_matches('@')
                    .to_owned();
                self.open_paths(range, query);
            }
            _ => {
                let width = self.editor_width;
                edit(&mut self.pane_mut().draft, key, width, true);
            }
        }
    }

    fn picker_key(&mut self, key: KeyEvent) {
        let page = self.modal_page as isize;
        let details_page = self.modal_info_rows.max(1) as isize;
        let Some(Modal::Picker(picker)) = &mut self.modal else {
            return;
        };
        match key {
            key if Shortcut::ENTER.matches(key) => {
                if let Some(choice) = picker.selected_choice() {
                    let kind = picker.kind.clone();
                    self.choose(kind, choice);
                }
            }
            key if Shortcut::TAB.matches(key) && picker.kind.completes_text() => {
                match (&picker.kind, picker.selected_choice()) {
                    (PickerKind::Commands, Some(ChoiceKind::Command(command))) => {
                        let completed = format!("/{}", command.name());
                        if picker.query.text() != completed {
                            picker.query.set_text(completed);
                            picker.selected = 0;
                            self.search(false);
                        }
                    }
                    (PickerKind::Commands, Some(choice @ ChoiceKind::Skill(_)))
                    | (PickerKind::Paths(_), Some(choice @ ChoiceKind::Path(_))) => {
                        let kind = picker.kind.clone();
                        self.choose(kind, choice);
                    }
                    _ => {}
                }
            }
            key if Shortcut::UP.matches(key) => picker.move_by(-1),
            key if Shortcut::DOWN.matches(key) => picker.move_by(1),
            key if Shortcut::PAGE_UP.matches(key)
                && matches!(picker.kind, PickerKind::Server | PickerKind::Permission) =>
            {
                picker.header_top = picker.header_top.saturating_add_signed(-details_page);
            }
            key if Shortcut::PAGE_DOWN.matches(key)
                && matches!(picker.kind, PickerKind::Server | PickerKind::Permission) =>
            {
                picker.header_top = picker.header_top.saturating_add_signed(details_page);
            }
            key if Shortcut::PAGE_UP.matches(key) => picker.move_by(-page),
            key if Shortcut::PAGE_DOWN.matches(key) => picker.move_by(page),
            _ if picker.kind.searchable()
                && edit(&mut picker.query, key, self.modal_editor_width, false) =>
            {
                picker.selected = 0;
                self.search(false);
            }
            _ => {}
        }
    }

    fn prompt_key(&mut self, key: KeyEvent) {
        let Some(Modal::Prompt(prompt)) = &mut self.modal else {
            return;
        };
        let page = self.modal_info_rows.max(1) as isize;
        if Shortcut::PAGE_UP.matches(key) || Shortcut::PAGE_DOWN.matches(key) {
            let amount = if Shortcut::PAGE_UP.matches(key) {
                -page
            } else {
                page
            };
            prompt.instructions_top = prompt.instructions_top.saturating_add_signed(amount);
            return;
        }
        if Shortcut::ENTER.matches(key) {
            if prompt.editor.text().trim().is_empty() {
                return;
            }
            let Some(Modal::Prompt(prompt)) = self.modal.take() else {
                return;
            };
            let content = prompt.editor.text();
            match prompt.kind {
                PromptKind::Agent => self.issue(
                    Address::new(self.active.session.clone(), content.trim().to_owned()),
                    Action::CreateAgent {
                        config: Box::default(),
                    },
                ),
                PromptKind::Model { connection } => {
                    let mut config = self.route_config();
                    config.connection = Some(connection);
                    config.model = Some(content.trim().to_owned());
                    self.configure(config);
                }
                PromptKind::Question { request_id } => self.issue(
                    self.active.clone(),
                    Action::ReplyQuestion {
                        request_id,
                        text: content.to_owned(),
                    },
                ),
            }
        } else {
            let multiline = matches!(prompt.kind, PromptKind::Question { .. });
            edit(&mut prompt.editor, key, self.modal_editor_width, multiline);
        }
    }

    fn output_key(&mut self, key: KeyEvent) {
        let Some(Modal::Output(viewer)) = &mut self.modal else {
            return;
        };
        let page = viewer.page_size() as isize;
        let offset = match key {
            key if Shortcut::PAGE_UP.matches(key) => viewer.scroll(-page),
            key if Shortcut::PAGE_DOWN.matches(key) => viewer.scroll(page),
            key if Shortcut::UP.matches(key) => viewer.scroll(-1),
            key if Shortcut::DOWN.matches(key) => viewer.scroll(1),
            key if Shortcut::REFRESH_OUTPUT.matches(key) => viewer.retry(),
            _ => None,
        };
        if let Some(offset) = offset {
            self.read_output(offset);
        }
    }

    fn command_menu(&mut self) {
        self.picker(PickerKind::Commands, Choice::commands()).header = format!(
            "Editor · {} Clear draft · {}/{} New line",
            Shortcut::CLEAR.label,
            Shortcut::NEW_LINE.label,
            Shortcut::CONTROL_NEW_LINE.label,
        );
        self.search(true);
    }

    fn search(&mut self, first: bool) {
        let available = self.available();
        let session_id = self.active.session.clone();
        let Some(Modal::Picker(picker)) = &mut self.modal else {
            return;
        };
        let query = picker.query.text().trim().to_owned();
        let request = match &picker.kind {
            PickerKind::Commands if available => Search::Skills(query),
            PickerKind::Paths(_) if available => Search::Paths(query),
            PickerKind::Sessions if available => Search::Sessions(query),
            PickerKind::Agents if first && available => Search::Agents(session_id),
            PickerKind::Models if first && available => Search::Models,
            PickerKind::Server if first => Search::Server,
            _ => {
                picker.clamp_selection();
                if !available
                    && matches!(
                        picker.kind,
                        PickerKind::Sessions
                            | PickerKind::Paths(_)
                            | PickerKind::Models
                            | PickerKind::Agents
                    )
                {
                    picker.problem = Some("Service unavailable · /reconnect".into());
                }
                return;
            }
        };
        self.effects.push(Job::Find(picker.begin_search(), request));
    }

    fn picker(&mut self, kind: PickerKind, local: Vec<Choice>) -> &mut Picker {
        let id = self.modal_id();
        self.modal = Some(Modal::Picker(Picker::new(kind, id, local)));
        if let Some(Modal::Picker(picker)) = &mut self.modal {
            picker
        } else {
            unreachable!("picker was installed")
        }
    }

    fn prompt(
        &mut self,
        kind: PromptKind,
        title: impl Into<String>,
        instructions: impl Into<String>,
    ) {
        self.modal = Some(Modal::Prompt(Prompt {
            kind,
            title: title.into(),
            instructions: instructions.into(),
            instructions_top: 0,
            editor: Editor::default(),
        }));
    }

    fn choose(&mut self, picker: PickerKind, choice: ChoiceKind) {
        self.modal = None;
        self.effects.push(Job::ClosePicker);
        match choice {
            ChoiceKind::Command(command) => self.perform(command),
            ChoiceKind::Skill(skill) => {
                let editor = &mut self.pane_mut().draft;
                let insertion = format!("/skill:{skill} ");
                let replace_to = editor
                    .text()
                    .strip_prefix("/skill:")
                    .map(|text| {
                        let end = text.find(char::is_whitespace).unwrap_or(text.len());
                        let separator = text[end..]
                            .chars()
                            .next()
                            .filter(|character| *character != '\n');
                        "/skill:".len() + end + separator.map_or(0, char::len_utf8)
                    })
                    .unwrap_or(0);
                editor.replace(0..replace_to, &insertion);
            }
            ChoiceKind::Path(path) => {
                if let PickerKind::Paths(range) = picker {
                    self.pane_mut().draft.replace(range, &format!("{path} "));
                }
            }
            ChoiceKind::Session { id, title, exists } => {
                self.select(Address::main(id), exists, title)
            }
            ChoiceKind::Agent { address } => self.select(
                Address::new(self.active.session.clone(), address),
                true,
                self.pane().title.clone(),
            ),
            ChoiceKind::CreateAgent => self.prompt(
                PromptKind::Agent,
                "New agent",
                "Choose the address used to send to this agent",
            ),
            ChoiceKind::Tier(tier) => {
                let mut config = self.route_config();
                config.tier = Some(tier);
                self.configure(config);
            }
            ChoiceKind::Connection(connection) => {
                let title = format!("Model on {connection}");
                self.prompt(
                    PromptKind::Model { connection },
                    title,
                    "Enter the model ID for this connection",
                );
            }
            ChoiceKind::Thinking(effort) => {
                let mut config = self.pane().config().clone();
                config.reasoning_effort = effort;
                self.configure(config);
            }
            ChoiceKind::Withdraw(input_command_id) => {
                self.issue(self.active.clone(), Action::Withdraw { input_command_id })
            }
            ChoiceKind::Pending(id) => self.open_pending_item(&id),
            ChoiceKind::PendingRetry(id) => self.pending_retry(&id),
            ChoiceKind::PendingCheck(id) => self.pending_check(&id),
            ChoiceKind::PendingDiscard(id) => self.pending_discard(&id),
            ChoiceKind::PendingOpen { target, exists } => self.pending_open(target, exists),
            ChoiceKind::RestoreRejected(id) => self.restore_rejected(&id),
            ChoiceKind::Permission { request_id, allow } => self.issue(
                self.active.clone(),
                Action::ReplyPermission { request_id, allow },
            ),
            ChoiceKind::AgentCard(key) => {
                if !self.pane_mut().transcript.toggle_agent_card(&key) {
                    self.problem("This Agent card is no longer available");
                }
            }
            ChoiceKind::Tool(tool) => self.show_output(tool),
            ChoiceKind::Restart => self.restart(),
        }
    }

    fn perform(&mut self, command: CommandMenu) {
        match command {
            CommandMenu::New => self.new_draft(),
            CommandMenu::Resume => {
                self.picker(PickerKind::Sessions, self.unsent_drafts());
                self.search(true);
            }
            CommandMenu::Agents => self.open_agents(),
            CommandMenu::Queue => self.send(Timing::NextTurn),
            CommandMenu::Withdraw => self.open_queue(),
            CommandMenu::Advance => {
                if self.pane().exists {
                    self.issue(self.active.clone(), Action::Advance);
                }
            }
            CommandMenu::Interrupt => self.interrupt(),
            CommandMenu::Reply => self.open_reply(),
            CommandMenu::Pending => {
                self.picker(PickerKind::Pending, self.pending_choices());
            }
            CommandMenu::Model => {
                self.picker(PickerKind::Models, Vec::new());
                self.search(true);
            }
            CommandMenu::Thinking => self.open_thinking(),
            CommandMenu::Copy => self.copy_reply(),
            CommandMenu::Details => self.open_agent_details(),
            CommandMenu::Tools => self.open_tools(),
            CommandMenu::Server => self.open_server(),
            CommandMenu::Reconnect => self.reconnect(),
            CommandMenu::Restart => self.restart(),
            CommandMenu::Web if self.require_service() => {
                self.effects.push(Job::Web(
                    self.pane().exists.then(|| self.selection().target),
                ));
            }
            CommandMenu::Web => {}
            CommandMenu::Quit => self.exit(),
        }
    }

    fn open_paths(&mut self, range: Range<usize>, query: String) {
        let picker = self.picker(PickerKind::Paths(range), Vec::new());
        picker.query.set_text(query);
        self.search(true);
    }

    fn open_agents(&mut self) {
        if !self.pane().exists {
            self.problem("Send the first prompt to establish a Session before creating an Agent");
            return;
        }
        let create = Choice::new(
            "Create agent",
            "Start with independent context",
            ChoiceKind::CreateAgent,
        );
        self.picker(PickerKind::Agents, vec![create]);
        self.search(true);
    }

    fn open_queue(&mut self) {
        let choices = self.pane().snapshot.as_ref().map_or_else(Vec::new, |snap| {
            snap.pending
                .iter()
                .map(|queued| {
                    Choice::new(
                        shorten(&single_line(&queued.input.text), 120),
                        format!(
                            "{} · receipt {}",
                            queued.receipt.agent, queued.receipt.event_id
                        ),
                        ChoiceKind::Withdraw(queued.receipt.command_id.clone()),
                    )
                })
                .collect()
        });
        self.picker(PickerKind::Queue, choices);
    }

    fn open_reply(&mut self) {
        let Some(snapshot) = self.pane().snapshot.as_ref() else {
            self.problem("No request is waiting for this Agent");
            return;
        };
        if let Some(request) = &snapshot.agent.question {
            let kind = PromptKind::Question {
                request_id: request.id.clone(),
            };
            let title = format!("Answer {}", request.agent);
            let instructions = request.text.clone();
            self.prompt(kind, title, instructions);
        } else if let Some(request) = &snapshot.agent.permission {
            let id = request.id.clone();
            let header = format!(
                "{} requests {} · {}\n{}\n{}",
                self.active.agent, request.tool, request.risk, request.summary, request.candidate
            );
            let choices = vec![
                Choice::new(
                    "Deny",
                    "Decline this request",
                    ChoiceKind::Permission {
                        request_id: id.clone(),
                        allow: false,
                    },
                ),
                Choice::new(
                    "Allow once",
                    "Approve the displayed request",
                    ChoiceKind::Permission {
                        request_id: id,
                        allow: true,
                    },
                ),
            ];
            self.picker(PickerKind::Permission, choices).header = header;
        } else {
            self.problem("No request is waiting for this Agent");
        }
    }

    fn open_thinking(&mut self) {
        let pane = self.pane();
        if pane.waiting_for_configuration() {
            self.message("Waiting for the service to report the changed route");
            return;
        }
        let Some(efforts) = pane.reasoning_efforts() else {
            self.problem("Select a resolved model with /model to see its reasoning settings");
            return;
        };
        if efforts.is_empty() {
            self.problem("The selected route does not expose a reasoning setting · /model");
            return;
        }
        let mut choices = vec![Choice::new(
            "Use route default",
            "Clear the explicit reasoning setting",
            ChoiceKind::Thinking(None),
        )];
        choices.extend(efforts.iter().map(|effort| {
            Choice::new(
                effort,
                "Provider-supported reasoning",
                ChoiceKind::Thinking(Some(effort.clone())),
            )
        }));
        let label = pane.effective_model_label().unwrap_or_default();
        self.picker(PickerKind::Thinking, choices).header = format!("Active · {label}");
    }

    fn route_config(&self) -> AgentConfig {
        let mut config = self.pane().config().clone();
        config.connection = None;
        config.tier = None;
        config.model = None;
        config.reasoning_effort = None;
        config.service_tier = None;
        config
    }

    fn copy_reply(&mut self) {
        let pane = self.pane();
        let Some(text) = pane
            .snapshot
            .as_ref()
            .and_then(|snap| snap.agent.last_output.as_deref())
            .or_else(|| pane.transcript.last_reply())
        else {
            self.problem("No assistant reply to copy");
            return;
        };
        self.effects.push(Job::Copy(text.to_owned()));
    }

    fn open_tools(&mut self) {
        let tools = self
            .pane()
            .transcript
            .tools()
            .into_iter()
            .map(|tool| {
                let name = if tool.name.is_empty() {
                    "Tool"
                } else {
                    &tool.name
                };
                let detail = if tool.output_id.is_some() {
                    format!("Saved output · {}", tool.summary)
                } else {
                    tool.summary.clone()
                };
                Choice::new(name, detail, ChoiceKind::Tool(tool.clone()))
            })
            .collect();
        self.picker(PickerKind::Tools, tools);
    }

    fn open_agent_details(&mut self) {
        let cards = self
            .pane()
            .transcript
            .visible_agent_choices()
            .into_iter()
            .map(|(key, address, task)| {
                Choice::new(format!("Agent {address}"), task, ChoiceKind::AgentCard(key))
            })
            .collect();
        self.picker(PickerKind::AgentDetails, cards);
    }

    fn show_output(&mut self, tool: ToolView) {
        let id = self.modal_id();
        self.modal = Some(Modal::Output(Box::new(OutputViewer::new(id, tool))));
        self.read_output(0);
    }

    fn read_output(&mut self, offset: u64) {
        if let Some(Modal::Output(viewer)) = &self.modal {
            if let Some(id) = &viewer.tool.output_id {
                self.effects
                    .push(Job::Output(self.selection(), viewer.id, id.clone(), offset));
            }
        }
    }

    fn open_server(&mut self) {
        let mut actions = vec![Choice::new(
            "Reconnect",
            "Check the connection again",
            ChoiceKind::Command(CommandMenu::Reconnect),
        )];
        if !self.remote {
            actions.push(Choice::new(
                "Restart local service",
                "Restart when current work is settled",
                ChoiceKind::Restart,
            ));
        }
        let status = self.connection_problem.clone().unwrap_or_default();
        self.picker(PickerKind::Server, actions).header = status;
        self.search(true);
    }

    fn restart(&mut self) {
        if self.remote {
            self.problem("Restart the remote service on its host");
        } else if !self.restarting && !self.connecting {
            self.restarting = true;
            self.message("Checking whether the service can restart…");
            self.effects.push(Job::Restart);
        }
    }

    fn pending_choices(&self) -> Vec<Choice> {
        let mut commands: Vec<_> = self.pending.values().collect();
        commands.sort_by(|left, right| left.command.command_id.cmp(&right.command.command_id));
        commands
            .into_iter()
            .map(|item| {
                let command = &item.command;
                Choice::new(
                    pending_label(item),
                    format!(
                        "{} · {} · {}",
                        item.stage.label(),
                        command.target.agent,
                        command.target.session_id
                    ),
                    ChoiceKind::Pending(command.command_id.clone()),
                )
            })
            .collect()
    }

    fn open_pending_item(&mut self, id: &str) {
        let Some(item) = self.pending.get(id) else {
            return;
        };
        let id = id.to_owned();
        let description = match &item.stage {
            PendingStage::Unknown(error) | PendingStage::Rejected(error) => error.clone(),
            _ => item.stage.label().to_owned(),
        };
        let exists = !matches!(
            &item.command.action,
            Action::Send {
                create: Some(_),
                ..
            }
        );
        let mut choices = Vec::new();
        match &item.stage {
            PendingStage::Missing => {
                choices.push(Choice::new(
                    "Retry exact command",
                    "Reuse its saved intent and command ID",
                    ChoiceKind::PendingRetry(id.clone()),
                ));
                choices.push(Choice::new(
                    "Discard saved command",
                    "Remove this pending record",
                    ChoiceKind::PendingDiscard(id),
                ));
            }
            PendingStage::Unknown(_) => choices.push(Choice::new(
                "Check server receipt",
                "Query the server for this command",
                ChoiceKind::PendingCheck(id),
            )),
            PendingStage::Rejected(_) => {
                if matches!(item.command.action, Action::Send { .. }) {
                    choices.push(Choice::new(
                        "Append rejected input to draft",
                        "Keep any text already in the draft",
                        ChoiceKind::RestoreRejected(id.clone()),
                    ));
                }
                choices.push(Choice::new(
                    "Dismiss",
                    "Close this rejected command",
                    ChoiceKind::PendingDiscard(id),
                ));
            }
            PendingStage::Checking
            | PendingStage::Saving
            | PendingStage::Sending
            | PendingStage::Discarding => {}
        }
        choices.push(Choice::new(
            "Open recipient",
            "Review this target in the terminal",
            ChoiceKind::PendingOpen {
                target: item.command.target.clone(),
                exists,
            },
        ));
        let header = format!("{}\n{description}", pending_label(item));
        self.picker(PickerKind::PendingItem, choices).header = header;
    }

    fn unsent_drafts(&self) -> Vec<Choice> {
        let mut drafts: Vec<_> = self
            .panes
            .iter()
            .filter(|(_, pane)| !pane.exists && !pane.draft.text().trim().is_empty())
            .collect();
        drafts.sort_by(|(first, _), (second, _)| first.session.cmp(&second.session));
        drafts
            .into_iter()
            .map(|(address, pane)| {
                let title = shorten(&single_line(pane.draft.text()), 92);
                Choice::new(
                    format!("Draft · {title}"),
                    "Unsent in this window",
                    ChoiceKind::Session {
                        id: address.session.clone(),
                        title,
                        exists: false,
                    },
                )
            })
            .collect()
    }
}

fn pending_label(item: &PendingItem) -> String {
    match &item.command.action {
        Action::Send { text, timing, .. } => {
            let action = if timing == &Timing::NextTurn {
                "Next Turn"
            } else {
                "Input"
            };
            format!("{action} · {}", shorten(&single_line(text), 110))
        }
        Action::CreateAgent { .. } => "Create Agent".to_owned(),
        Action::Configure { .. } => "Change settings".to_owned(),
        Action::ReplyQuestion { .. } => "Answer question".to_owned(),
        Action::ReplyPermission { .. } => "Respond to authorization".to_owned(),
        Action::Withdraw { .. } => "Withdraw input".to_owned(),
        Action::Advance => "Continue Agent".to_owned(),
        Action::Cancel { .. } => "Interrupt execution".to_owned(),
    }
}

fn control(key: KeyEvent, character: char) -> bool {
    key.code == KeyCode::Char(character)
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && !key
            .modifiers
            .intersects(KeyModifiers::ALT | KeyModifiers::SUPER)
}

fn is_text(key: KeyEvent) -> bool {
    !key.modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
}

fn edit(editor: &mut Editor, key: KeyEvent, width: usize, multiline: bool) -> bool {
    if Shortcut::CLEAR.matches(key) {
        return !editor.take_text().is_empty();
    }
    if multiline && (Shortcut::NEW_LINE.matches(key) || Shortcut::CONTROL_NEW_LINE.matches(key)) {
        editor.insert("\n");
        return true;
    }
    match key.code {
        KeyCode::Backspace
            if key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            editor.delete_word()
        }
        KeyCode::Backspace => editor.backspace(),
        KeyCode::Delete => editor.delete(),
        KeyCode::Char('w') if control(key, 'w') => editor.delete_word(),
        KeyCode::Char('d') if control(key, 'd') => editor.delete(),
        KeyCode::Char(character) if is_text(key) => {
            editor.insert(character.encode_utf8(&mut [0; 4]))
        }
        KeyCode::Tab if multiline && Shortcut::TAB.matches(key) => editor.insert(TAB),
        _ => {
            move_cursor(editor, key, width, multiline);
            return false;
        }
    }
    true
}

fn move_cursor(editor: &mut Editor, key: KeyEvent, width: usize, multiline: bool) {
    let words = key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
    match key.code {
        KeyCode::Left if words => editor.word_left(),
        KeyCode::Right if words => editor.word_right(),
        KeyCode::Left => editor.left(),
        KeyCode::Right => editor.right(),
        KeyCode::Home => editor.home(),
        KeyCode::End => editor.end(),
        KeyCode::Char('a') if control(key, 'a') => editor.home(),
        KeyCode::Char('e') if control(key, 'e') => editor.end(),
        KeyCode::Up if multiline => editor.vertical(width, -1),
        KeyCode::Down if multiline => editor.vertical(width, 1),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use kuku_client::Action;
    use serde_json::json;

    use crate::app::{App, Modal};
    use crate::jobs::{Job, Search, Update};
    use crate::menu::{Choice, ChoiceKind, PickerKind, PromptKind};

    fn connected_app() -> App {
        let workspace = PathBuf::from("/workspace");
        let mut app = App::new(workspace.clone(), None);
        app.update(Update::Connected {
            endpoint: "http://127.0.0.1:6789".into(),
            workspace,
        });
        app.effects.clear();
        app
    }

    fn press(app: &mut App, code: KeyCode) {
        app.key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn control_p_f1_and_slash_open_actions() {
        let mut app = connected_app();
        app.paste("unsent draft");
        for key in [
            KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE),
        ] {
            app.key(key);
            assert!(matches!(&app.modal,
                Some(Modal::Picker(picker)) if matches!(picker.kind, PickerKind::Commands)
            ));
            assert_eq!("unsent draft", app.pane().draft.text());
            press(&mut app, KeyCode::Esc);
        }
        app.pane_mut().draft.take_text();
        press(&mut app, KeyCode::Char('/'));
        assert!(matches!(&app.modal,
            Some(Modal::Picker(picker)) if matches!(picker.kind, PickerKind::Commands)
        ));
        assert!(app.pane().draft.text().is_empty());
    }

    #[test]
    fn arrow_keys_move_inside_the_draft_and_recall_history_at_its_edges() {
        let mut app = connected_app();
        app.pane_mut().draft.remember("Earlier prompt");
        app.paste("first\nsecond");
        press(&mut app, KeyCode::Up);
        assert_eq!("first\nsecond", app.pane().draft.text());
        assert_eq!(5, app.pane().draft.cursor());
        press(&mut app, KeyCode::Up);
        assert_eq!("Earlier prompt", app.pane().draft.text());
        app.key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
        assert_eq!("Earlier prompt", app.pane().draft.text());
        press(&mut app, KeyCode::Down);
        assert_eq!("first\nsecond", app.pane().draft.text());
        assert_eq!(5, app.pane().draft.cursor());
        press(&mut app, KeyCode::Down);
        assert_eq!("first\nsecond", app.pane().draft.text());
        assert_eq!(11, app.pane().draft.cursor());
    }

    fn agent_cards(app: &mut App) {
        for (index, mut event) in [
            json!({
                "type":"tool_call", "tool":"create_agent", "tool_call_id":"create",
                "args":{"address":"reviewer","instructions":"Inspect documentation\nUse precise citations"}
            }),
            json!({
                "type":"tool_result", "tool_call_id":"create", "status":"ok",
                "summary":"Session operation committed",
                "structured":{"agent":"reviewer","command_id":"tool:main:7:create","event_id":1}
            }),
            json!({
                "type":"tool_call", "tool":"send_message", "tool_call_id":"send",
                "args":{"to":"reviewer","message":"Review the CLI\nList failing cases"}
            }),
            json!({
                "type":"tool_result", "tool_call_id":"send", "status":"ok",
                "summary":"Session operation committed",
                "structured":{"agent":"reviewer","command_id":"tool:main:7:send","event_id":3}
            }),
        ].into_iter().enumerate() {
            event["id"] = json!(index + 1);
            event["agent"] = json!("main");
            event["turn"] = json!(7);
            event["turn_number"] = json!(1);
            app.pane_mut().transcript.apply(serde_json::from_value(event).unwrap(), None);
        }
    }

    fn transcript_text(app: &mut App) -> String {
        app.pane_mut()
            .transcript
            .project(96, 32)
            .lines
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn open_details(app: &mut App) {
        app.key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
        app.paste("/details");
        press(app, KeyCode::Enter);
        assert!(matches!(&app.modal, Some(Modal::Picker(picker))
            if matches!(picker.kind, PickerKind::AgentDetails)));
        app.effects.clear();
    }

    #[test]
    fn details_search_selects_one_visible_card_and_escape_leaves_previews_alone() {
        let mut app = connected_app();
        agent_cards(&mut app);
        let collapsed = transcript_text(&mut app);
        assert!(!collapsed.contains("Use precise citations"));
        assert!(!collapsed.contains("List failing cases"));
        open_details(&mut app);
        app.paste("reviewer citations");
        let Some(Modal::Picker(picker)) = &app.modal else {
            panic!("details picker expected");
        };
        let choices = picker.visible_choices();
        assert_eq!(1, choices.len());
        assert_eq!("Agent reviewer", choices[0].label);
        assert!(choices[0].detail.contains("Inspect documentation"));
        assert!(app.effects.is_empty(), "search only uses visible cards");
        press(&mut app, KeyCode::Esc);
        assert_eq!(collapsed, transcript_text(&mut app));

        open_details(&mut app);
        app.paste("reviewer");
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert!(app.modal.is_none());
        let expanded = transcript_text(&mut app);
        assert!(expanded.contains("Use precise citations"), "{expanded}");
        assert!(!expanded.contains("List failing cases"), "{expanded}");
        assert!(app.pending.is_empty());
    }

    #[test]
    fn control_o_expands_and_collapses_agent_previews_together() {
        let mut app = connected_app();
        agent_cards(&mut app);
        let collapsed = transcript_text(&mut app);
        let details = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL);
        app.key(details);
        let expanded = transcript_text(&mut app);
        assert!(expanded.contains("Use precise citations"), "{expanded}");
        assert!(expanded.contains("List failing cases"), "{expanded}");
        app.key(details);
        assert_eq!(collapsed, transcript_text(&mut app));
    }

    #[test]
    fn control_c_clears_the_focused_editor_and_keeps_the_client_open() {
        let mut app = connected_app();
        let clear = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        app.paste("draft to keep");
        press(&mut app, KeyCode::F(1));
        app.paste("resume");
        app.effects.clear();
        app.key(clear);
        assert!(
            matches!(&app.modal, Some(Modal::Picker(picker)) if picker.query.text().is_empty())
        );
        assert!(
            matches!(app.effects.as_slice(), [Job::Find(_, Search::Skills(query))] if query.is_empty())
        );
        assert_eq!("draft to keep", app.pane().draft.text());
        press(&mut app, KeyCode::Esc);

        app.prompt(PromptKind::Agent, "New agent", "Enter the address");
        app.paste("reviewer");
        app.key(clear);
        assert!(
            matches!(&app.modal, Some(Modal::Prompt(prompt)) if prompt.editor.text().is_empty())
        );
        assert_eq!("draft to keep", app.pane().draft.text());
        press(&mut app, KeyCode::Esc);

        app.effects.clear();
        app.key(clear);
        app.key(clear);
        assert!(app.pane().draft.text().is_empty());
        assert!(app.effects.is_empty());
        assert!(!app.quit);
    }

    #[test]
    fn tab_completes_a_slash_action_and_enter_runs_it_with_the_draft_intact() {
        let mut app = connected_app();
        app.paste("draft to keep");
        press(&mut app, KeyCode::F(1));
        app.paste("qui");
        press(&mut app, KeyCode::Tab);
        assert!(
            matches!(&app.modal, Some(Modal::Picker(picker)) if picker.query.text() == "/quit")
        );
        assert_eq!("draft to keep", app.pane().draft.text());
        assert!(!app.quit);
        press(&mut app, KeyCode::Enter);
        assert!(app.modal.is_none());
        assert!(app.quit);
    }

    #[test]
    fn tab_inserts_paths_and_skills_in_the_draft() {
        let mut app = connected_app();
        let path = Choice::new("Cargo.toml", "", ChoiceKind::Path("Cargo.toml".into()));
        app.paste("inspect Car");
        let range = app.pane().draft.completion_range();
        app.picker(PickerKind::Paths(range), vec![path]);
        press(&mut app, KeyCode::Tab);
        assert_eq!("inspect Cargo.toml ", app.pane().draft.text());
        assert!(app.modal.is_none());

        let skill = Choice::new("/skill:review", "", ChoiceKind::Skill("review".into()));
        app.picker(PickerKind::Commands, vec![skill]);
        press(&mut app, KeyCode::Tab);
        assert_eq!("/skill:review inspect Cargo.toml ", app.pane().draft.text());
        assert!(app.modal.is_none());
        assert!(app.pending.is_empty());
    }

    #[test]
    fn tab_leaves_action_choices_open_and_enter_confirms_the_highlighted_choice() {
        let mut app = connected_app();
        let choose_permission = |allow| ChoiceKind::Permission {
            request_id: "permit-1".into(),
            allow,
        };
        for (kind, choice) in [
            (PickerKind::Models, ChoiceKind::Tier("review".into())),
            (PickerKind::Queue, ChoiceKind::Withdraw("queued-1".into())),
            (PickerKind::Server, ChoiceKind::Restart),
            (PickerKind::Permission, choose_permission(false)),
        ] {
            app.picker(kind, vec![Choice::new("Selected", "", choice)]);
            press(&mut app, KeyCode::Tab);
            assert!(matches!(app.modal, Some(Modal::Picker(_))));
            assert!(app.effects.is_empty());
        }

        app.picker(
            PickerKind::Permission,
            vec![
                Choice::new("Deny", "", choose_permission(false)),
                Choice::new("Allow", "", choose_permission(true)),
            ],
        );
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Enter);
        assert!(matches!(
            app.effects.as_slice(),
            [Job::ClosePicker, Job::Save(command)] if matches!(
                &command.action,
                Action::ReplyPermission { request_id, allow: true } if request_id == "permit-1"
            )
        ));
    }
}
