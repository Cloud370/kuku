use crate::app::{App, Modal};
use crate::jobs::Job;
use crate::menu::{Choice, ChoiceKind, PickerKind};
use crate::output::OutputViewer;
use crate::transcript::ToolView;

impl App {
    pub(crate) fn show_outputs(&mut self, mut outputs: Vec<ToolView>) {
        if outputs.len() == 1 {
            self.show_output(outputs.remove(0));
            return;
        }
        let choices = outputs
            .into_iter()
            .map(|tool| {
                let name = if tool.name.is_empty() {
                    "Tool".to_owned()
                } else {
                    tool.name.clone()
                };
                let detail = if tool.output_id.is_some() {
                    format!("Saved output · {}", tool.summary)
                } else {
                    tool.summary.clone()
                };
                Choice::new(name, detail, ChoiceKind::Tool(tool))
            })
            .collect();
        self.picker(PickerKind::Tools, choices).header = "Outputs for selected message".into();
    }

    pub(crate) fn show_output(&mut self, tool: ToolView) {
        let offset = tool.output_range.as_ref().map_or(0, |range| range.start);
        let id = self.modal_id();
        self.modal = Some(Modal::Output(Box::new(OutputViewer::new(id, tool))));
        self.read_output(offset);
    }

    pub(super) fn read_output(&mut self, offset: u64) {
        if let Some(Modal::Output(viewer)) = &self.modal {
            if let Some((id, limit)) = viewer.read_request(offset) {
                self.effects
                    .push(Job::Output(self.selection(), viewer.id, id, offset, limit));
            }
        }
    }
}
