use crossterm::event::{KeyCode, KeyEvent};
use kuku_client::Timing;

use crate::app::{App, Modal, Notice};
use crate::jobs::Job;
use crate::menu::{Choice, CommandInput, CommandMenu, PickerKind};

use super::is_text;

enum Invocation {
    Command(CommandMenu),
    RestoreContext,
    CancelJob(String),
}

impl Invocation {
    fn parse(command: CommandMenu, arguments: &str) -> Result<Self, String> {
        let mut words = arguments.split_whitespace();
        let Some(operation) = words.next() else {
            return Ok(Self::Command(command));
        };
        if command == CommandMenu::Compact {
            return match (operation, words.next()) {
                ("undo", None) => Ok(Self::RestoreContext),
                ("retry", None) => Ok(Self::Command(CommandMenu::Compact)),
                _ => Err("Use /compact, /compact retry, or /compact undo".into()),
            };
        }
        if command != CommandMenu::Jobs {
            return Err(format!("Use /{}", command.name()));
        }
        match (operation, words.next(), words.next()) {
            ("cancel", Some(id), None) => Ok(Self::CancelJob(id.into())),
            _ => Err("Use /jobs or /jobs cancel <id>".into()),
        }
    }
}

impl App {
    pub(super) fn command_menu(&mut self) {
        self.picker(PickerKind::Commands, Choice::commands())
            .query
            .set_text("/".into());
        self.search(true);
    }

    pub(super) fn submit_composer(&mut self, timing: Timing) {
        if self.bare_exit_draft() {
            self.modal = Some(Modal::ExitConfirmation);
            return;
        }
        let text = self.pane().draft.text();
        let Some((input, command)) = CommandInput::draft(text)
            .and_then(|input| input.command().map(|command| (input, command)))
            .filter(|(input, command)| {
                matches!(command, CommandMenu::Jobs | CommandMenu::Compact)
                    || input.arguments.trim().is_empty()
            })
        else {
            self.send(timing);
            return;
        };
        match Invocation::parse(command, input.arguments) {
            Ok(invocation) => {
                let teach = input.name != command.name() && command != CommandMenu::Quit;
                let hint = teach.then(|| format!("/{} → /{}", input.name, command.name()));
                self.pane_mut().draft.take_text();
                self.perform(invocation);
                if let Some(hint) = hint {
                    if self.taught_commands.insert(command) {
                        self.alias_hint = Some(Notice::confirmation(hint));
                    }
                }
            }
            Err(error) => self.problem(error),
        }
    }

    pub(super) fn bare_exit_draft(&self) -> bool {
        let pane = self.pane();
        pane.value_attachments.is_empty()
            && matches!(pane.draft.text().trim_matches(' '), "exit" | "quit")
    }

    pub(super) fn exit_confirmation_key(&mut self, key: KeyEvent) {
        if matches!(key.code, KeyCode::Char('y' | 'Y')) && is_text(key) {
            self.modal = None;
            self.exit();
        }
    }

    pub(super) fn run_picker_command(&mut self, command: CommandMenu) {
        let Some(Modal::Picker(picker)) = &mut self.modal else {
            return;
        };
        let input = CommandInput::query(picker.query.text());
        match Invocation::parse(command, input.arguments_for(command)) {
            Ok(invocation) => {
                self.close_picker();
                self.perform(invocation);
            }
            Err(error) => picker.problem = Some(error),
        }
    }

    pub(super) fn complete_command(&mut self, command: CommandMenu) {
        let insert_in_draft = self.pane().draft.text().is_empty();
        let Some(Modal::Picker(picker)) = &mut self.modal else {
            return;
        };
        let completed = CommandInput::query(picker.query.text()).complete(command);
        if insert_in_draft {
            self.pane_mut().draft.set_text(completed);
            self.close_picker();
            self.focus_composer();
        } else if picker.query.text() != completed {
            picker.query.set_text(completed);
            picker.selected = 0;
            self.search(false);
        }
    }

    fn perform(&mut self, invocation: Invocation) {
        let command = match invocation {
            Invocation::Command(command) => command,
            Invocation::CancelJob(id) => return self.cancel_background_job(id),
            Invocation::RestoreContext => {
                if self.pane().exists {
                    self.issue(self.active.clone(), kuku_client::Action::RestoreContext);
                } else {
                    self.problem("Open a conversation before restoring context");
                }
                return;
            }
        };
        match command {
            CommandMenu::New => self.new_draft(),
            CommandMenu::Resume => {
                self.picker(PickerKind::Sessions, self.unsent_drafts());
                self.search(true);
            }
            CommandMenu::Agents => self.open_agents(),
            CommandMenu::Jobs => self.open_jobs(),
            CommandMenu::Compact => {
                if self.pane().exists {
                    self.issue(self.active.clone(), kuku_client::Action::Compact);
                } else {
                    self.problem("Open a conversation before compacting context");
                }
            }
            CommandMenu::Model => self.open_models(),
            CommandMenu::Thinking => self.open_thinking(),
            CommandMenu::StatusLine => self.open_statusline(),
            CommandMenu::Save => self.open_value_save(),
            CommandMenu::Values => self.open_values(),
            CommandMenu::Server => self.open_server(),
            CommandMenu::Restart => self.open_restart(),
            CommandMenu::Web if self.require_service() => {
                self.effects.push(Job::Web(
                    self.pane().exists.then(|| self.selection().target),
                ));
            }
            CommandMenu::Web => {}
            CommandMenu::Quit => self.exit(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CommandInput, CommandMenu, Invocation};

    #[test]
    fn compaction_aliases_share_retry_and_undo() {
        for alias in ["/compact", "/compress", "/summarize"] {
            let input = CommandInput::draft(alias).unwrap();
            assert_eq!(Some(CommandMenu::Compact), input.command());
            assert!(matches!(
                Invocation::parse(CommandMenu::Compact, " retry"),
                Ok(Invocation::Command(CommandMenu::Compact))
            ));
            assert!(matches!(
                Invocation::parse(CommandMenu::Compact, " undo"),
                Ok(Invocation::RestoreContext)
            ));
        }
        assert!(Invocation::parse(CommandMenu::Compact, " undo extra").is_err());
    }
}
