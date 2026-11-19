use std::ops::Range;

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
};
use kuku_client::{Action, AgentConfig, Error, Target, Timing};
use ratatui::buffer::Buffer;

use crate::app::{Address, App, Modal, PendingItem, PendingStage};
use crate::editor::Editor;
use crate::jobs::{Job, Search, Selection};
use crate::keys::{AgentDirection, Shortcut};
use crate::menu::{Choice, ChoiceKind, CommandMenu, Picker, PickerKind, Prompt, PromptKind};
use crate::output::OutputViewer;
use crate::text::{shorten, single_line, TAB};
use crate::transcript::{ReplySource, ToolView};

mod contextual;
mod inline;
mod pointer;

const MOUSE_SCROLL_LINES: isize = 3;

impl App {
    pub(crate) fn event(&mut self, event: Event, screen: &Buffer) {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => self.key(key),
            Event::Paste(text) => self.paste(&text),
            Event::Mouse(
                mouse @ MouseEvent {
                    kind: MouseEventKind::ScrollUp,
                    ..
                },
            ) => self.wheel(mouse, -MOUSE_SCROLL_LINES),
            Event::Mouse(
                mouse @ MouseEvent {
                    kind: MouseEventKind::ScrollDown,
                    ..
                },
            ) => self.wheel(mouse, MOUSE_SCROLL_LINES),
            Event::Mouse(mouse) => self.pointer(mouse, screen),
            Event::FocusLost => self.text_selection.cancel_drag(),
            Event::Resize(_, _) | Event::FocusGained | Event::Key(_) => {}
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

    fn restore_question_composer(&mut self, target: &Target) {
        let address = Address::new(target.session_id.clone(), target.agent.clone());
        if let Some(pane) = self.panes.get_mut(&address) {
            pane.keyboard_focus = crate::app::KeyboardFocus::Composer;
            pane.focused_request = None;
        }
    }

    fn key(&mut self, key: KeyEvent) {
        self.dismiss_confirmation();
        self.text_selection.cancel_drag();
        if Shortcut::ESCAPE.matches(key) {
            if let Some(modal) = self.modal.take() {
                if let Modal::Prompt(prompt) = &modal {
                    if let PromptKind::Question { target, .. } = &prompt.kind {
                        self.restore_question_composer(target);
                    }
                }
                if !matches!(modal, Modal::Help) {
                    self.effects.push(Job::ClosePicker);
                }
                return;
            }
        }
        match self.modal {
            Some(Modal::Help) => {}
            Some(Modal::Picker(_)) => self.picker_key(key),
            Some(Modal::Prompt(_)) => self.prompt_key(key),
            Some(Modal::Output(_)) => self.output_key(key),
            Some(Modal::StatusLineEditor(_)) => self.statusline_key(key),
            None => self.composer_key(key),
        }
    }

    fn paste(&mut self, text: &str) {
        self.dismiss_confirmation();
        self.text_selection.cancel_drag();
        if matches!(&self.modal, Some(Modal::Help)) {
            return;
        }
        if let Some(Modal::Picker(picker)) = &mut self.modal {
            if picker.kind.searchable() {
                picker.query.insert(&text.replace(['\r', '\n'], " "));
                picker.selected = 0;
                self.search(false);
            }
        } else if let Some(Modal::Prompt(prompt)) = &mut self.modal {
            if prompt.kind.multiline() {
                prompt.editor.insert(text);
            } else {
                prompt.editor.insert(&text.replace(['\r', '\n'], " "));
            }
        } else if self.modal.is_none() {
            self.deactivate_context_focus();
            self.focus_composer();
            self.pane_mut().draft.insert(text);
        }
    }

    fn wheel(&mut self, mouse: MouseEvent, amount: isize) {
        self.text_selection.cancel_drag();
        match &mut self.modal {
            Some(Modal::Help) => {}
            Some(Modal::Output(viewer)) => {
                if let Some(offset) = viewer.scroll(amount) {
                    self.read_output(offset);
                }
            }
            Some(Modal::Picker(picker)) => {
                if matches!(picker.kind, PickerKind::Server) {
                    picker.header_top = picker.header_top.saturating_add_signed(amount);
                } else {
                    picker.move_by(amount);
                }
            }
            Some(Modal::Prompt(prompt)) => {
                prompt.instructions_top = prompt.instructions_top.saturating_add_signed(amount)
            }
            Some(Modal::StatusLineEditor(editor)) => editor.move_selection(amount),
            None => {
                if !self
                    .text_selection
                    .transcript_cell(mouse)
                    .is_some_and(|(row, _)| self.pane().transcript.notes_at_row(row))
                {
                    self.pane_mut().transcript.close_notes();
                }
                self.scroll(amount);
            }
        }
    }

    fn composer_key(&mut self, key: KeyEvent) {
        if self.contextual_key(key) {
            return;
        }
        if self.inline_key(key) {
            return;
        }
        if let Some(direction) = AgentDirection::from_key(key) {
            self.cycle_agent(direction);
            return;
        }
        if self.reading_key(key) {
            return;
        }
        if matches!(key.code, KeyCode::Char(_)) && is_text(key)
            || Shortcut::NEW_LINE.matches(key)
            || Shortcut::CONTROL_NEW_LINE.matches(key)
        {
            self.focus_composer();
        }
        let editing = self.composer_focused();
        match key {
            key if Shortcut::CLEAR.matches(key) => {
                if self.pane_mut().draft.take_text().is_empty() && key.kind == KeyEventKind::Press {
                    self.exit();
                }
            }
            key if Shortcut::ACTIONS.matches(key) || Shortcut::ACTIONS_ALIAS.matches(key) => {
                self.show_shortcut_help()
            }
            key if Shortcut::REPLY.matches(key) => self.focus_request(),
            key if Shortcut::ESCAPE.matches(key) => self.interrupt(),
            key if editing && Shortcut::NEXT_TURN.matches(key) => self.send(Timing::NextTurn),
            key if editing && Shortcut::ENTER.matches(key) => self.send(Timing::Steer),
            key if Shortcut::UP.matches(key)
                && self.pane().draft.text().is_empty()
                && self.queued_for_edit().next().is_some() =>
            {
                self.focus_queued()
            }
            key if Shortcut::PREVIOUS_PROMPT.matches(key) || Shortcut::NEXT_PROMPT.matches(key) => {
                self.focus_composer();
                self.pane_mut()
                    .draft
                    .recall(if key.code == KeyCode::Up { -1 } else { 1 });
            }
            key if Shortcut::UP.matches(key) || Shortcut::DOWN.matches(key) => {
                let direction = if key.code == KeyCode::Up { -1 } else { 1 };
                let width = self.editor_width;
                if !self.pane_mut().draft.vertical(width, direction) {
                    self.scroll(direction);
                }
            }
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
            key if editing && Shortcut::TAB.matches(key) => {
                let draft = &self.pane().draft;
                let range = draft.completion_range();
                let query = draft.text()[range.clone()]
                    .trim_start_matches('@')
                    .to_owned();
                self.open_paths(range, query);
            }
            _ if editing => {
                let width = self.editor_width;
                edit(&mut self.pane_mut().draft, key, width, true);
            }
            _ => {}
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
            key if Shortcut::PAGE_UP.matches(key) && matches!(picker.kind, PickerKind::Server) => {
                picker.header_top = picker.header_top.saturating_add_signed(-details_page);
            }
            key if Shortcut::PAGE_DOWN.matches(key)
                && matches!(picker.kind, PickerKind::Server) =>
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
            if let PromptKind::QueuedInput { input_command_id } = &prompt.kind {
                let id = input_command_id.clone();
                let text = prompt.editor.text().to_owned();
                if self.replace_queued(id, text) {
                    self.modal = None;
                }
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
                PromptKind::Question { request_id, target } => {
                    self.restore_question_composer(&target);
                    if self.selection().target != target
                        || !self.pane().snapshot.as_ref().is_some_and(|snapshot| {
                            snapshot
                                .agent
                                .question
                                .as_ref()
                                .is_some_and(|request| request.id == request_id)
                        })
                    {
                        self.problem("This question is no longer waiting for this Agent");
                    } else {
                        self.issue(
                            self.active.clone(),
                            Action::ReplyQuestion {
                                request_id,
                                text: content.to_owned(),
                            },
                        );
                    }
                }
                PromptKind::QueuedInput { .. } => unreachable!("queued input edited above"),
            }
        } else {
            let multiline = prompt.kind.multiline();
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
            "Editor · {} Clear or exit · {}/{} New line",
            Shortcut::CLEAR.label,
            Shortcut::NEW_LINE.label,
            Shortcut::CONTROL_NEW_LINE.label,
        );
        self.search(true);
    }

    fn show_shortcut_help(&mut self) {
        self.modal = Some(Modal::Help);
    }

    fn search(&mut self, first: bool) {
        let available = self.available();
        let Some(Modal::Picker(picker)) = &mut self.modal else {
            return;
        };
        let query = picker.query.text().trim().to_owned();
        let request = match &picker.kind {
            PickerKind::Commands if available => Search::Skills(query),
            PickerKind::Paths(_) if available => Search::Paths(query),
            PickerKind::Sessions if available => Search::Sessions(query),
            PickerKind::Models if first && available => Search::Models,
            PickerKind::Server if first => Search::Server,
            _ => {
                picker.clamp_selection();
                if !available
                    && matches!(
                        picker.kind,
                        PickerKind::Sessions | PickerKind::Paths(_) | PickerKind::Models
                    )
                {
                    picker.problem = Some("Service unavailable · Ctrl+R to reconnect".into());
                }
                return;
            }
        };
        self.effects.push(Job::Find(picker.begin_search(), request));
    }

    pub(crate) fn picker(&mut self, kind: PickerKind, local: Vec<Choice>) -> &mut Picker {
        let id = self.modal_id();
        self.modal = Some(Modal::Picker(Picker::new(kind, id, local)));
        if let Some(Modal::Picker(picker)) = &mut self.modal {
            picker
        } else {
            unreachable!("picker was installed")
        }
    }

    pub(crate) fn prompt(
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
            ChoiceKind::Agent { target } => self.select_agent(target),
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
            ChoiceKind::Pending(id) => self.open_pending_item(&id),
            ChoiceKind::PendingRetry(id) => self.pending_retry(&id),
            ChoiceKind::PendingCheck(id) => self.pending_check(&id),
            ChoiceKind::PendingDiscard(id) => self.pending_discard(&id),
            ChoiceKind::PendingOpen { target, exists } => self.pending_open(target, exists),
            ChoiceKind::RestoreRejected(id) => self.restore_rejected(&id),
            ChoiceKind::Reconnect => self.reconnect(),
            ChoiceKind::Detail(key) => {
                if !self.toggle_transcript_detail(&key) {
                    self.problem("These details are no longer available");
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
            CommandMenu::Model => {
                self.picker(PickerKind::Models, Vec::new());
                self.search(true);
            }
            CommandMenu::Thinking => self.open_thinking(),
            CommandMenu::StatusLine => self.open_statusline(),
            CommandMenu::Copy => self.copy_reply(),
            CommandMenu::Source => {
                let showing_source = self.pane_mut().transcript.toggle_source();
                self.confirmation(if showing_source {
                    "Markdown source · /source for preview"
                } else {
                    "Markdown preview"
                });
            }
            CommandMenu::Notes => self.read_latest_notes(),
            CommandMenu::Details => self.open_details(),
            CommandMenu::Tools => self.open_tools(),
            CommandMenu::Server => self.open_server(),
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

    fn answer_question(&mut self) {
        let Some(request) = self
            .pane()
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.agent.question.as_ref())
        else {
            self.problem("No request is waiting for this Agent");
            return;
        };
        let kind = PromptKind::Question {
            request_id: request.id.clone(),
            target: self.selection().target,
        };
        let title = format!("Answer {}", request.agent);
        let instructions = request.text.clone();
        self.prompt(kind, title, instructions);
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
        self.pane_mut().pending_reply_copy = None;
        let pane = self.pane();
        let source = pane.transcript.last_reply().or_else(|| {
            pane.snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.agent.last_output.as_deref())
                .map(ReplySource::Local)
        });
        match source {
            Some(ReplySource::Local(text)) => self.copy_text(text.to_owned()),
            Some(ReplySource::History(id)) => {
                if self.require_service() {
                    self.pane_mut().pending_reply_copy = Some(id);
                    self.effects.push(Job::ReplySource(self.selection(), id));
                }
            }
            None => self.problem("No assistant reply to copy"),
        }
    }

    pub(crate) fn copied_reply(
        &mut self,
        selection: Selection,
        id: u64,
        result: Result<String, Error>,
    ) {
        if selection != self.selection() || self.pane().pending_reply_copy != Some(id) {
            return;
        }
        match result {
            Ok(text) => self.copy_text(text),
            Err(error) => {
                self.pane_mut().pending_reply_copy = None;
                self.problem(format!("Copy reply · {error}"));
            }
        }
    }

    fn copy_text(&mut self, text: String) {
        self.pane_mut().pending_reply_copy = None;
        self.effects.push(Job::Copy(text));
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

    fn open_details(&mut self) {
        let choices = self
            .pane()
            .transcript
            .visible_detail_choices()
            .into_iter()
            .map(|(key, label, summary)| Choice::new(label, summary, ChoiceKind::Detail(key)))
            .collect();
        self.picker(PickerKind::Details, choices);
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
            ChoiceKind::Reconnect,
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
                if matches!(item.command.action, Action::ReplaceQueued { .. }) {
                    choices.push(Choice::new(
                        "Copy saved edit to draft",
                        "Keep your draft and the saved replacement",
                        ChoiceKind::RestoreRejected(id.clone()),
                    ));
                }
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
                if matches!(
                    item.command.action,
                    Action::Send { .. } | Action::ReplaceQueued { .. }
                ) {
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
        Action::ReplaceQueued { text, .. } => {
            format!("Queued edit · {}", shorten(&single_line(text), 110))
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
        KeyCode::Up | KeyCode::Down if multiline => {
            editor.vertical(width, if key.code == KeyCode::Up { -1 } else { 1 });
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests;
