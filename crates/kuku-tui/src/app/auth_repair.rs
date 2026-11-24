use crossterm::event::Event;

use crate::jobs::{Job, Selection};
use crate::setup::{
    CredentialRepair, CredentialRepairAction, CredentialRepairApplied, CredentialRepairRequest,
    PreparedCredentialRepair,
};

use super::App;

impl App {
    pub(crate) fn credential_failure(&self) -> Option<CredentialRepairRequest> {
        let pane = self.pane();
        CredentialRepairRequest::from_retry(pane.snapshot.as_deref()?, pane.retry_metadata()?)
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
        if self.connection_problem.is_some() {
            self.credential_repair_connect_failed();
            return;
        }
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

#[cfg(test)]
mod tests {
    use kuku_client::{HistoryPage, RetryFailureKind, Snapshot};
    use serde_json::json;

    use super::App;
    use crate::contextual::ContextControl;
    use crate::jobs::{Job, Update};

    fn stopped_after_live_failure(kind: RetryFailureKind) -> App {
        let workspace = std::env::current_dir().unwrap();
        let mut app = App::new(workspace.clone(), None);
        app.update(Update::Connected {
            endpoint: "http://127.0.0.1:6789".into(),
            workspace,
        });
        let snapshot: Snapshot = serde_json::from_value(json!({
            "workspace":app.workspace, "session_id":app.active.session, "cursor":5,
            "agent":{"address":"main", "status":"running", "turn":7,
                "execution_id":5, "config":{}, "pending_inputs":0,
                "usage":{"model_requests":1,"unreported_requests":0}},
            "agents":[], "pending":[], "progress":null
        }))
        .unwrap();
        app.update(Update::Loaded(
            app.selection(),
            Box::new(snapshot.clone()),
            HistoryPage {
                entries: Vec::new(),
                rewind_groups: Vec::new(),
                next_before: None,
            },
        ));
        for event in [
            json!({
                "type":"committed", "id":6, "agent":"main", "kind":"runtime.model_retry",
                "entry":{"id":6,"agent":"main","turn":7,"type":"retry","retry":{
                    "source":"runtime.model_retry", "phase":"stopped", "failed_kind":kind,
                    "will_retry":false,"request_id":"auth-request"
                }}
            }),
            json!({"type":"committed", "id":7, "agent":"main", "kind":"model.error"}),
            json!({"type":"committed", "id":8, "agent":"main", "kind":"runtime.operation",
                "operation":"turn_settled", "state_changed":true}),
        ] {
            app.update(Update::Observation(
                app.selection(),
                serde_json::from_value(event).unwrap(),
            ));
        }
        assert!(app.credential_failure().is_none());
        let mut snapshot = snapshot;
        snapshot.cursor = 8;
        snapshot.agent.status = kuku_client::AgentStatus::Interrupted;
        snapshot.agent.execution_id = None;
        app.update(Update::Refreshed(app.selection(), Ok(Box::new(snapshot))));
        app.effects.clear();
        app
    }

    #[test]
    fn live_authentication_failure_keeps_repair_after_a_progress_free_refresh() {
        let mut app = stopped_after_live_failure(RetryFailureKind::Authentication);
        assert!(app.pane().snapshot.as_ref().unwrap().progress.is_none());
        assert_eq!(
            Some(RetryFailureKind::Authentication),
            app.pane()
                .retry_metadata()
                .and_then(|retry| retry.failed_kind)
        );
        let request = app
            .credential_failure()
            .expect("live failure offers repair");
        assert_eq!(7, request.turn);
        assert_eq!("auth-request", request.request_id);
        assert!(app.contextual_controls().iter().any(|control| matches!(
            control,
            ContextControl::RepairCredentials { request: offered } if offered == &request
        )));
        app.open_credential_repair(request.clone());
        assert!(app.credential_repair.is_some());
        assert!(
            matches!(app.effects.as_slice(), [Job::PrepareCredentialRepair {
            request: prepared, ..
        }] if prepared == &request)
        );
    }

    #[test]
    fn non_auth_failures_and_new_executions_do_not_offer_stale_repair() {
        for kind in [
            RetryFailureKind::RateLimited,
            RetryFailureKind::Transport,
            RetryFailureKind::PermissionDenied,
        ] {
            let app = stopped_after_live_failure(kind);
            assert!(app.credential_failure().is_none());
        }
        let mut app = stopped_after_live_failure(RetryFailureKind::Authentication);
        app.update(Update::Observation(
            app.selection(),
            serde_json::from_value(json!({
                "type":"committed", "id":9, "agent":"main", "kind":"runtime.operation",
                "operation":"execution_started", "state_changed":true
            }))
            .unwrap(),
        ));
        assert!(app.credential_failure().is_none());
        assert!(
            !app.contextual_controls()
                .iter()
                .any(|control| { matches!(control, ContextControl::RepairCredentials { .. }) })
        );
    }
}
