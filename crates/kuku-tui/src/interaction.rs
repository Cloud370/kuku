use std::ops::Range;

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
};
use kuku_client::{Action, AgentConfig, Error, Target, Timing};
use ratatui::buffer::Buffer;

use self::rewind::rewind_instructions;
use crate::app::{Address, App, Modal, PendingItem, PendingStage};
use crate::editor::Editor;
use crate::jobs::{Job, Search, Selection};
use crate::keys::{AgentDirection, Shortcut};
use crate::menu::{Choice, ChoiceKind, Picker, PickerKind, Prompt, PromptKind};
use crate::text::{shorten, single_line, TAB};

mod commands;
mod contextual;
mod inline;
mod input;
mod inspect;
mod messages;
mod picker;
mod pointer;
mod rewind;
mod rewound;
mod thinking;

const MOUSE_SCROLL_LINES: isize = 3;

impl App {
    pub(crate) fn event(&mut self, event: Event, screen: &Buffer) {
        if self.model_editor.is_some() {
            self.model_editor_input(event);
            return;
        }
        if self.credential_repair.is_some() {
            self.credential_repair_input(event);
            return;
        }
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
            Event::Mouse(mouse) => self.pointer(mouse, screen),
            Event::FocusLost => {
                self.text_selection.cancel_drag();
                self.modal_selection.cancel_drag();
            }
            Event::Resize(_, _) | Event::FocusGained | Event::Key(_) => {}
        }
        self.clear_model_default_if_closed();
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
    fn save_question_draft(&mut self, target: &Target, request_id: &str, editor: Editor) {
        let address = Address::new(target.session_id.clone(), target.agent.clone());
        if let Some(pane) = self.panes.get_mut(&address) {
            pane.question_drafts.insert(request_id.to_owned(), editor);
            pane.keyboard_focus = crate::app::KeyboardFocus::Composer;
            pane.focused_request = None;
        }
    }
    fn wheel(&mut self, amount: isize) {
        if self.modal.is_some() {
            self.modal_selection.cancel_drag();
        } else {
            self.text_selection.cancel_drag();
        }
        if self.move_rewind_selection(amount) {
            return;
        }
        match &mut self.modal {
            Some(Modal::Help | Modal::ExitConfirmation) => {}
            Some(Modal::Output(viewer)) => {
                if let Some(offset) = viewer.scroll(amount) {
                    self.read_output(offset);
                }
            }
            Some(Modal::Picker(picker)) => {
                if matches!(picker.kind, PickerKind::Server | PickerKind::ModelDefault) {
                    picker.header_top = picker.header_top.saturating_add_signed(amount);
                } else if !picker
                    .thinking
                    .as_ref()
                    .is_some_and(|thinking| thinking.confirm.is_some())
                {
                    picker.move_by(amount);
                }
            }
            Some(Modal::Prompt(prompt)) => {
                prompt.instructions_top = prompt.instructions_top.saturating_add_signed(amount)
            }
            Some(Modal::StatusLineEditor(editor)) => editor.move_selection(amount),
            None => self.scroll(amount),
        }
    }

    fn composer_key(&mut self, key: KeyEvent) {
        if Shortcut::ACTIONS.matches(key) || Shortcut::ACTIONS_ALIAS.matches(key) {
            self.show_shortcut_help();
            return;
        }
        if Shortcut::CYCLE_THINKING.matches(key) {
            self.cycle_thinking();
            return;
        }
        if self.opens_command_menu(key) {
            self.command_menu();
            return;
        }
        if self.contextual_key(key) {
            return;
        }
        if Shortcut::RECONNECT.matches(key) {
            self.reconnect();
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
                let pane = self.pane_mut();
                let empty = pane.draft.take_text().is_empty() && pane.value_attachments.is_empty();
                pane.value_attachments.clear();
                if empty && key.kind == KeyEventKind::Press {
                    self.exit();
                }
            }
            key if Shortcut::REPLY.matches(key) => self.focus_request(),
            key if Shortcut::ESCAPE.matches(key) => self.rewind_escape(),
            key if editing && Shortcut::NEXT_TURN.matches(key) => {
                self.submit_composer(Timing::NextTurn)
            }
            key if editing && Shortcut::ENTER.matches(key) => self.submit_composer(Timing::Steer),
            key if Shortcut::UP.matches(key)
                && self.pane().draft.text().is_empty()
                && self.has_queued_input() =>
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
            key if Shortcut::MODEL.matches(key) => self.open_models(),
            key if Shortcut::DETAILS.matches(key) => {
                let expanded = self.pane_mut().transcript.toggle_details();
                self.confirmation(if expanded {
                    "Details expanded"
                } else {
                    "Details collapsed"
                });
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
            if let PromptKind::RewindMessage { event_id, text, .. } = &prompt.kind {
                let event_id = *event_id;
                let text = text.clone();
                let restore_files = self.rewind_restore_files;
                self.modal = None;
                self.rewind_restore_files = false;
                self.rewind_message(event_id, text, restore_files);
                return;
            }
            let empty = if matches!(prompt.kind, PromptKind::SavePaste) {
                prompt.editor.text().is_empty()
            } else {
                prompt.editor.text().trim().is_empty()
            };
            if empty {
                return;
            }
            if let PromptKind::SaveValue { output_id } = &prompt.kind {
                let output_id = output_id.clone();
                let name = prompt.editor.text().to_owned();
                if self.save_value(&name, output_id) {
                    self.modal = None;
                }
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
                PromptKind::Agent => self.begin_agent_setup(content.trim().to_owned()),
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
                PromptKind::SavePaste => self.name_paste(content.to_owned()),
                PromptKind::SaveValue { .. } => unreachable!("saved value handled above"),
                PromptKind::QueuedInput { .. } => unreachable!("queued input edited above"),
                PromptKind::RewindMessage { .. } => unreachable!("rewind confirmed above"),
            }
        } else if key.code == KeyCode::Char(' ')
            && key.modifiers.is_empty()
            && matches!(prompt.kind, PromptKind::RewindMessage { .. })
        {
            self.rewind_restore_files = !self.rewind_restore_files;
            if let PromptKind::RewindMessage { text, .. } = &prompt.kind {
                prompt.instructions = rewind_instructions(text, self.rewind_restore_files);
            }
        } else if !matches!(prompt.kind, PromptKind::RewindMessage { .. }) {
            let multiline = prompt.kind.multiline();
            edit(&mut prompt.editor, key, self.modal_editor_width, multiline);
        }
    }

    fn output_key(&mut self, key: KeyEvent) {
        let Some(Modal::Output(viewer)) = &mut self.modal else {
            return;
        };
        if Shortcut::NAME_OUTPUT.matches(key) {
            if let Some(id) = viewer.tool.output_id.clone() {
                self.prompt_value_name(Some(id));
            }
            return;
        }
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

    fn opens_command_menu(&self, key: KeyEvent) -> bool {
        key.code == KeyCode::Char('/') && is_text(key) && self.pane().draft.text().is_empty()
    }

    fn show_shortcut_help(&mut self) {
        self.modal = Some(Modal::Help);
    }

    pub(crate) fn search(&mut self, first: bool) {
        let available = self.available();
        let Some(Modal::Picker(picker)) = &mut self.modal else {
            return;
        };
        let query = picker.query.text().trim().to_owned();
        let request = match &picker.kind {
            PickerKind::Commands if available => {
                Search::Skills(crate::menu::CommandInput::query(&query).text().to_owned())
            }
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
        self.modal = Some(Modal::Picker(Box::new(Picker::new(kind, id, local))));
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
        self.modal = Some(Modal::Prompt(Box::new(Prompt {
            kind,
            title: title.into(),
            instructions: instructions.into(),
            instructions_top: 0,
            editor: Editor::default(),
        })));
    }

    fn choose(&mut self, picker: PickerKind, choice: ChoiceKind, ticket: u64) {
        if matches!(picker, PickerKind::Rewind) && !self.rewind_picker_current(ticket) {
            return;
        }
        if let ChoiceKind::RewindMessage { event_id, turn, .. } = &choice {
            self.load_rewind_source(ticket, *event_id, *turn);
            return;
        }
        if let ChoiceKind::Command(command) = choice {
            self.run_picker_command(command);
            return;
        }
        if let ChoiceKind::Thinking(effort) = choice {
            self.apply_thinking_choice(effort);
            self.close_picker();
            return;
        }
        if let ChoiceKind::AgentSetup(action) = choice {
            self.choose_agent_setup(action);
            return;
        }
        self.close_picker();
        match choice {
            ChoiceKind::Command(_) => unreachable!("commands use the picker query"),
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
            ChoiceKind::AgentSetup(_) => unreachable!("agent setup retains its picker state"),
            ChoiceKind::Tier(tier) => {
                let mut config = self.route_config();
                config.tier = Some(tier);
                self.configure(config);
            }
            ChoiceKind::ModelActions(tier) => self.open_model_actions(*tier),
            ChoiceKind::EditModel(intent) => self.open_model_editor(intent),
            ChoiceKind::ConfirmModelSetting(change) => self.confirm_model_setting(change),
            ChoiceKind::SaveModelSetting(change) => {
                self.effects.push(Job::SaveModelSetting(change))
            }
            ChoiceKind::Close => {}
            ChoiceKind::SetCurrentModelAsDefault => self.prepare_model_default(),
            ChoiceKind::ConfirmModelDefault => self.confirm_model_default(ticket),
            ChoiceKind::CancelModelDefault => self.cancel_model_default(ticket),
            ChoiceKind::Thinking(_) => unreachable!("thinking uses the checked model route"),
            ChoiceKind::RewindMessage { .. } => {
                unreachable!("rewind messages load their source before confirmation")
            }
            ChoiceKind::MessageEntry(message) => self.open_message_actions(message),
            ChoiceKind::MessageAction { message, action } => {
                self.execute_message_action(message, action)
            }
            ChoiceKind::RewindSection => {}
            ChoiceKind::RewindGroup {
                selection,
                rewind_event_id,
                action,
            } => {
                self.activate_selected_readonly_rewind_group(selection, rewind_event_id, action);
            }
            ChoiceKind::Pending(id) => self.open_pending_item(&id),
            ChoiceKind::PendingRetry(id) => self.pending_retry(&id),
            ChoiceKind::PendingCheck(id) => self.pending_check(&id),
            ChoiceKind::PendingDiscard(id) => self.pending_discard(&id),
            ChoiceKind::PendingOpen { target, exists } => self.pending_open(target, exists),
            ChoiceKind::RestoreRejected(id) => self.restore_rejected(&id),
            ChoiceKind::Reconnect => self.reconnect(),
            ChoiceKind::Detail { key, message } => self.toggle_detail_choice(key, message),
            ChoiceKind::Tool(tool) => self.show_output(tool),
            ChoiceKind::BackgroundJob(job) => self.open_background_job(*job),
            ChoiceKind::CancelBackground(id) => self.cancel_background_job(id),
            ChoiceKind::Value(value) => self.open_value(value),
            ChoiceKind::AttachValue(value) => self.attach_value(value),
            ChoiceKind::ReadValue(value) => self.read_value(value),
            ChoiceKind::ClearValues => self.clear_values(),
            ChoiceKind::ConfirmRestart(identity) => self.confirm_restart(*identity),
        }
    }

    fn close_picker(&mut self) {
        self.modal = None;
        self.modal_selection.clear();
        self.effects.push(Job::ClosePicker);
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
        let request_id = request.id.clone();
        let target = self.selection().target;
        let title = format!("Answer {}", request.agent);
        let instructions = request.text.clone();
        let editor = self
            .pane_mut()
            .question_drafts
            .remove(&request_id)
            .unwrap_or_default();
        self.modal = Some(Modal::Prompt(Box::new(Prompt {
            kind: PromptKind::Question { request_id, target },
            title,
            instructions,
            instructions_top: 0,
            editor,
        })));
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

    pub(crate) fn copied_reply(
        &mut self,
        selection: Selection,
        id: u64,
        result: Result<String, Error>,
    ) {
        if selection != self.selection() || self.pane().pending_reply_copy != Some(id) {
            return;
        }
        if self.pane().pending_selected_reply_copy == Some(id)
            && !self
                .pane()
                .transcript
                .has_assistant_message(id, &self.active.agent)
        {
            self.pane_mut().pending_reply_copy = None;
            self.pane_mut().pending_selected_reply_copy = None;
            return;
        }
        match result {
            Ok(text) => self.copy_text(text),
            Err(error) => {
                self.pane_mut().pending_reply_copy = None;
                self.pane_mut().pending_selected_reply_copy = None;
                self.problem(format!("Copy reply · {error}"));
            }
        }
    }

    fn copy_text(&mut self, text: String) {
        self.pane_mut().pending_reply_copy = None;
        self.pane_mut().pending_selected_reply_copy = None;
        self.effects.push(Job::Copy(text));
    }

    fn open_server(&mut self) {
        let actions = vec![Choice::new(
            "Reconnect",
            "Check the kuku service connection again",
            ChoiceKind::Reconnect,
        )];
        let status = self.connection_problem.clone().unwrap_or_default();
        self.picker(PickerKind::Server, actions).header = status;
        self.search(true);
    }

    fn open_restart(&mut self) {
        if self.remote {
            self.problem("Restart the kuku service from its host.");
        } else if !self.restarting && !self.connecting {
            let picker = self.picker(PickerKind::Restart, Vec::new());
            picker.header = "Checking the managed local kuku service…".into();
            picker.loading = true;
            let ticket = picker.ticket.id;
            self.effects.push(Job::PrepareRestart(ticket));
        }
    }

    fn confirm_restart(&mut self, identity: kuku_client::local::ServiceIdentity) {
        if !self.restarting && !self.connecting && !self.remote {
            self.restarting = true;
            self.message("Checking whether the kuku service can restart…");
            self.effects.push(Job::Restart(Box::new(identity)));
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
        Action::RewindMessage { text, .. } => {
            format!("Rewind · {}", shorten(&single_line(text), 110))
        }
        Action::CreateAgent { .. } => "Create Agent".to_owned(),
        Action::Configure { .. } => "Change settings".to_owned(),
        Action::ReplyQuestion { .. } => "Answer question".to_owned(),
        Action::ReplyPermission { .. } => "Respond to authorization".to_owned(),
        Action::Withdraw { .. } => "Withdraw input".to_owned(),
        Action::Compact => "Compact context".to_owned(),
        Action::RestoreContext => "Restore context".to_owned(),
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
mod composer_tests;
#[cfg(test)]
#[path = "interaction/message_tests.rs"]
mod message_tests;
#[cfg(test)]
mod rewind_history_tests;
#[cfg(test)]
mod rewound_copy_tests;
#[cfg(test)]
mod rewound_focus_tests;
#[cfg(test)]
#[path = "interaction/rewound_picker_tests.rs"]
mod rewound_picker_tests;
#[cfg(test)]
mod tests;
