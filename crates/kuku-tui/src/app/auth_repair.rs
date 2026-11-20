use crossterm::event::Event;

use crate::jobs::{Job, Selection};
use crate::setup::{
    authentication_failure, CredentialRepair, CredentialRepairAction, CredentialRepairApplied,
    CredentialRepairRequest, PreparedCredentialRepair,
};

use super::App;

impl App {
    pub(crate) fn credential_failure(&self) -> Option<CredentialRepairRequest> {
        authentication_failure(self.pane().snapshot.as_deref()?)
    }

    pub(crate) fn open_credential_repair(&mut self, request: CredentialRepairRequest) {
        if self.credential_failure().as_ref() != Some(&request)
            || request.target != self.selection().target
            || !self.require_service()
        {
            self.problem("The authentication failure changed; inspect the current Session.");
            return;
        }
        self.focus_composer();
        self.deactivate_context_focus();
        self.text_selection.clear();
        let ticket = self.modal_id();
        let selection = self.selection();
        self.credential_repair = Some(Box::new(CredentialRepair::new(
            ticket,
            selection.clone(),
            request.clone(),
        )));
        self.effects.push(Job::PrepareCredentialRepair {
            ticket,
            selection,
            request,
        });
    }

    pub(crate) fn credential_repair_input(&mut self, event: Event) {
        let Some(mut repair) = self.credential_repair.take() else {
            return;
        };
        if !repair.matches_selection(&self.selection()) {
            repair.invalidate();
        }
        let action = repair.handle(event);
        match action {
            CredentialRepairAction::Close => {
                self.focus_composer();
            }
            CredentialRepairAction::None => self.credential_repair = Some(repair),
            CredentialRepairAction::Save(plan, key) => {
                self.effects.push(Job::SaveCredentialRepair {
                    ticket: repair.ticket,
                    selection: repair.selection.clone(),
                    plan,
                    key,
                });
                self.credential_repair = Some(repair);
            }
            CredentialRepairAction::Restart(saved) => {
                self.effects.push(Job::RestartCredentialRepair {
                    ticket: repair.ticket,
                    selection: repair.selection.clone(),
                    saved,
                });
                self.credential_repair = Some(repair);
            }
            CredentialRepairAction::Reconnect => {
                self.credential_repair = Some(repair);
                self.reconnect();
            }
        }
    }

    pub(super) fn credential_repair_prepared(
        &mut self,
        ticket: u64,
        selection: Selection,
        result: Result<PreparedCredentialRepair, String>,
    ) {
        if selection != self.selection() {
            if let Some(repair) = &mut self.credential_repair {
                if repair.ticket == ticket {
                    repair.invalidate();
                }
            }
            return;
        }
        let failure = self.credential_failure();
        if let Some(repair) = &mut self.credential_repair {
            if repair.ticket == ticket && repair.selection == selection {
                if failure.as_ref() == Some(&repair.request) {
                    repair.prepared(result);
                } else {
                    repair.invalidate();
                }
            }
        }
    }

    pub(super) fn credential_repair_saved(
        &mut self,
        ticket: u64,
        selection: Selection,
        result: Result<CredentialRepairApplied, String>,
    ) {
        if selection != self.selection() {
            if let Some(repair) = &mut self.credential_repair {
                if repair.ticket == ticket {
                    repair.invalidate();
                }
            }
            return;
        }
        let reconnect = self.credential_repair.as_mut().is_some_and(|repair| {
            repair.ticket == ticket && repair.selection == selection && repair.saved(result)
        });
        if reconnect {
            self.reconnect();
        }
    }

    pub(super) fn credential_repair_connected(&mut self) {
        let target = self.selection().target;
        if let Some(repair) = &mut self.credential_repair {
            if repair.request.target == target {
                repair.reconnected();
            }
        }
    }

    pub(super) fn credential_repair_connect_failed(&mut self) {
        if let Some(repair) = &mut self.credential_repair {
            repair.reconnect_failed();
        }
    }
}
