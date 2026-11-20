use kuku_client::{JobSnapshot, JobStatus, Timing};

use crate::jobs::{job_status_label, Job, Search};
use crate::menu::{Choice, ChoiceKind, PickerKind};
use crate::text::single_line;
use crate::transcript::ToolView;

use super::App;

pub(crate) enum JobCommand {
    List,
    Cancel(String),
    Invalid,
}

impl JobCommand {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let mut parts = text.split_whitespace();
        if !matches!(parts.next()?, "jobs" | "/jobs") {
            return None;
        }
        Some(match (parts.next(), parts.next(), parts.next()) {
            (None, None, None) => Self::List,
            (Some("cancel"), Some(id), None) => Self::Cancel(id.to_owned()),
            _ => Self::Invalid,
        })
    }
}

impl App {
    pub(crate) fn submit_composer(&mut self) {
        let draft = self.pane().draft.text().trim_start();
        if let Some(command) = JobCommand::parse(draft).filter(|_| draft.starts_with('/')) {
            if !matches!(command, JobCommand::Invalid) {
                self.pane_mut().draft.take_text();
            }
            self.run_job_command(command);
        } else {
            self.send(Timing::Steer);
        }
    }

    pub(crate) fn run_job_command(&mut self, command: JobCommand) {
        match command {
            JobCommand::List => self.open_jobs(),
            JobCommand::Cancel(id) => self.cancel_background_job(id),
            JobCommand::Invalid => self.problem("Use /jobs or /jobs cancel <id>"),
        }
    }

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
        let mut choices = Vec::new();
        if job.status == JobStatus::Running {
            choices.push(Choice::new(
                "Cancel job",
                &job.id,
                ChoiceKind::CancelBackground(job.id.clone()),
            ));
        } else if let Some(output_id) = job.output_id {
            choices.push(Choice::new(
                "Read saved output",
                &job.id,
                ChoiceKind::Tool(ToolView {
                    name: job.id.clone(),
                    summary: job.summary.clone(),
                    output_id: Some(output_id),
                    ..ToolView::default()
                }),
            ));
        }
        self.picker(PickerKind::JobActions, choices).header = format!(
            "{} · {} · {}",
            job.id,
            job_status_label(job.status),
            single_line(&job.summary)
        );
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
