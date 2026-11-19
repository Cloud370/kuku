use crate::app::{App, Modal};
use crate::jobs::Job;
use crate::menu::{Choice, ChoiceKind, PickerKind};
use crate::output::OutputViewer;
use crate::transcript::ToolView;

impl App {
    pub(super) fn open_tools(&mut self) {
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

    pub(super) fn open_details(&mut self) {
        let choices = self
            .pane()
            .transcript
            .visible_detail_choices()
            .into_iter()
            .map(|(key, label, summary)| Choice::new(label, summary, ChoiceKind::Detail(key)))
            .collect();
        self.picker(PickerKind::Details, choices);
    }

    pub(crate) fn show_output(&mut self, tool: ToolView) {
        let id = self.modal_id();
        self.modal = Some(Modal::Output(Box::new(OutputViewer::new(id, tool))));
        self.read_output(0);
    }

    pub(super) fn read_output(&mut self, offset: u64) {
        if let Some(Modal::Output(viewer)) = &self.modal {
            if let Some(id) = &viewer.tool.output_id {
                self.effects
                    .push(Job::Output(self.selection(), viewer.id, id.clone(), offset));
            }
        }
    }
}
