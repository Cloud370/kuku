use kuku_client::{JobSnapshot, JobStatus};

use crate::jobs::{Job, Search, job_detail};
use crate::menu::{Choice, ChoiceKind, PickerKind};
use crate::transcript::ToolView;

use super::App;

impl App {
    pub(crate) fn open_jobs(&mut self) {
        if !self.require_service() {
            return;
        }
        let exists = self.pane().exists;
        let picker = self.picker(PickerKind::Jobs, Vec::new());
        if !exists {
            picker.header = "No background jobs for this Agent".into();
            return;
        }
        let ticket = picker.begin_search();
        self.effects
            .push(Job::Find(ticket, Search::Jobs(self.selection().target)));
    }

    pub(crate) fn open_background_job(&mut self, job: JobSnapshot) {
        let header = format!("{} · {}", job.id, job_detail(&job));
        let mut choices = Vec::new();
        if job.status == JobStatus::Running {
            choices.push(Choice::new(
                "Cancel job",
                &job.id,
                ChoiceKind::CancelBackground(job.id.clone()),
            ));
        }
        if let Some(output_id) = job.output_id {
            choices.push(Choice::new(
                if job.status == JobStatus::Running {
                    "Read output"
                } else {
                    "Read saved output"
                },
                &job.id,
                ChoiceKind::Tool(ToolView {
                    name: job.id.clone(),
                    summary: job.summary.clone(),
                    output_id: Some(output_id),
                    ..ToolView::default()
                }),
            ));
        }
        self.picker(PickerKind::JobActions, choices).header = header;
    }

    pub(crate) fn cancel_background_job(&mut self, id: String) {
        if !self.require_service() {
            return;
        }
        if !self.pane().exists {
            self.problem("No background jobs for this Agent");
            return;
        }
        self.message(format!("Cancelling {id}…"));
        self.effects
            .push(Job::CancelBackground(self.selection(), id));
    }
}
