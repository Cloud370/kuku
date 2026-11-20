use kuku_client::{
    Error, NamedValue, NamedValueSource, OutputReference, MAX_INPUT_VALUES, MAX_VALUE_NAME_BYTES,
};

use crate::jobs::{Job, Selection, ValueSave};
use crate::menu::{Choice, ChoiceKind, PickerKind, PromptKind};
use crate::text::shorten;
use crate::transcript::ToolView;

use super::{Address, App, Pane};

impl App {
    pub(crate) fn open_value_save(&mut self) {
        if !self.require_service() {
            return;
        }
        if self.pane().draft.text().is_empty() {
            self.prompt(
                PromptKind::SavePaste,
                "Save pasted text",
                "Paste text and press Enter to name it. The text stays in this Session.",
            );
        } else {
            self.prompt_value_name(None);
        }
    }

    pub(crate) fn name_paste(&mut self, text: String) {
        self.pane_mut().draft.set_text(text);
        self.prompt_value_name(None);
    }

    pub(crate) fn prompt_value_name(&mut self, output_id: Option<String>) {
        self.prompt(
            PromptKind::SaveValue { output_id },
            "Name saved value",
            "Enter a name, or NAME --replace to update an existing value.",
        );
    }

    pub(crate) fn save_value(&mut self, input: &str, output_id: Option<String>) -> bool {
        let Some((name, replace)) = value_name(input) else {
            self.problem(format!(
                "Use up to {MAX_VALUE_NAME_BYTES} letters, digits, underscores, or hyphens. Start with a letter."
            ));
            return false;
        };
        if !self.require_service() {
            return false;
        }
        let pane = self.pane();
        if pane.sending || pane.value_saving {
            self.problem("Wait for the current submission or save");
            return false;
        }
        let source = output_id.map_or_else(
            || NamedValueSource::Text(pane.draft.text().to_owned()),
            NamedValueSource::Output,
        );
        let create = (!pane.exists).then(|| pane.initial_config.clone());
        self.pane_mut().value_saving = true;
        self.effects.push(Job::SaveValue(
            self.selection(),
            Box::new(ValueSave {
                name: name.to_owned(),
                source,
                replace,
                create,
            }),
        ));
        self.message("Saving value…");
        true
    }

    pub(crate) fn value_saved(
        &mut self,
        selection: Selection,
        source_text: Option<String>,
        result: Result<NamedValue, Error>,
    ) {
        let address = Address::from_target(&selection.target);
        let active = address == self.active && selection.target.workspace == self.workspace;
        let pane = self.panes.entry(address).or_default();
        pane.value_saving = false;
        match result {
            Ok(value) => {
                let initial = !pane.exists;
                pane.exists = true;
                if source_text.as_deref() == Some(pane.draft.text()) {
                    pane.draft.take_text();
                }
                if active {
                    if initial {
                        self.begin_observing();
                    }
                    self.confirmation(format!("Saved {} · /values", value.name));
                }
                if self.quit_after_save {
                    self.exit();
                }
            }
            Err(error) => {
                self.quit_after_save = false;
                if active {
                    self.problem(format!("Save value · {error}"));
                }
            }
        }
    }

    pub(crate) fn open_values(&mut self) {
        if !self.require_service() {
            return;
        }
        let mut choices = Vec::new();
        if !self.pane().value_attachments.is_empty() {
            choices.push(Choice::new(
                "Clear draft attachments",
                "",
                ChoiceKind::ClearValues,
            ));
        }
        let exists = self.pane().exists;
        let picker = self.picker(PickerKind::Values, choices);
        if !exists {
            picker.header = "Use /save to name text or paste into this draft.".into();
            return;
        }
        let ticket = picker.begin_search();
        self.effects.push(Job::Find(
            ticket,
            crate::jobs::Search::Values(self.selection().target),
        ));
    }

    pub(crate) fn open_value(&mut self, value: NamedValue) {
        let choices = vec![
            Choice::new(
                "Attach to draft",
                "Send this exact version with your next input",
                ChoiceKind::AttachValue(value.clone()),
            ),
            Choice::new("Read saved value", "", ChoiceKind::ReadValue(value.clone())),
        ];
        self.picker(PickerKind::ValueActions, choices).header = value.name;
    }

    pub(crate) fn read_value(&mut self, value: NamedValue) {
        self.show_output(ToolView {
            name: value.name,
            output_id: Some(value.reference.id),
            ..ToolView::default()
        });
    }

    pub(crate) fn attach_value(&mut self, value: NamedValue) {
        let attachments = &mut self.pane_mut().value_attachments;
        if attachments
            .iter()
            .any(|(_, reference)| reference == &value.reference)
        {
            self.confirmation(format!("{} is already attached", value.name));
        } else if attachments.len() >= MAX_INPUT_VALUES {
            self.problem(format!(
                "Attach up to {MAX_INPUT_VALUES} saved values · /values"
            ));
        } else {
            attachments.push((value.name, value.reference));
            self.focus_composer();
        }
    }

    pub(crate) fn clear_values(&mut self) {
        self.pane_mut().value_attachments.clear();
        self.confirmation("Draft attachments cleared");
    }
}

impl Pane {
    pub(super) fn take_values(&mut self) -> Vec<OutputReference> {
        std::mem::take(&mut self.value_attachments)
            .into_iter()
            .map(|(_, reference)| reference)
            .collect()
    }

    pub(super) fn restore_values(&mut self, values: Vec<OutputReference>) {
        for reference in values {
            if self
                .value_attachments
                .iter()
                .all(|(_, existing)| existing != &reference)
            {
                let label = shorten(&reference.id, 16);
                self.value_attachments.push((label, reference));
            }
        }
    }
}

fn value_name(input: &str) -> Option<(&str, bool)> {
    let input = input.trim();
    let (name, replace) = input
        .strip_suffix(" --replace")
        .map_or((input, false), |name| (name.trim_end(), true));
    if name.len() <= MAX_VALUE_NAME_BYTES
        && name.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        Some((name, replace))
    } else {
        None
    }
}
