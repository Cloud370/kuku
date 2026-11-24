use crossterm::event::{KeyCode, KeyEvent};
use kuku::adapter::{Probe, ProbeBatch, ProbeProgress, SetupTextCheck};
use kuku::config::ProviderFormat;
use kuku::ProviderFailureKind;

use super::config::Settings;
use super::state::{SetupIntent, SetupState, SetupStep};
use super::CredentialSource;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RecoveryAction {
    Retry,
    Skip,
    ChangeKey,
    ChangeUrl,
    SignIn,
}

impl RecoveryAction {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Retry => "Retry this model",
            Self::Skip => "Skip this model",
            Self::ChangeKey => "Change API key",
            Self::ChangeUrl => "Change API URL",
            Self::SignIn => "Reauthorize account",
        }
    }
}

pub(super) enum RepairDraft {
    Key {
        source: CredentialSource,
        stored: String,
        environment: String,
    },
    Url {
        address: String,
        full_url: bool,
        protocol: ProviderFormat,
        protocol_fixed: bool,
    },
}

pub(super) fn text_problem(check: SetupTextCheck) -> &'static str {
    match check {
        SetupTextCheck::Failed {
            kind: Some(ProviderFailureKind::Authentication),
            ..
        } => "Key rejected",
        SetupTextCheck::Failed {
            kind: Some(ProviderFailureKind::PermissionDenied),
            ..
        } => "Access denied",
        SetupTextCheck::Failed {
            kind: Some(ProviderFailureKind::Timeout),
            ..
        } => "Text timed out",
        SetupTextCheck::Failed {
            kind: Some(ProviderFailureKind::QuotaExhausted),
            ..
        } => "Quota exceeded",
        SetupTextCheck::Failed {
            kind: Some(ProviderFailureKind::RateLimited),
            ..
        } => "Rate limited",
        SetupTextCheck::Failed {
            status: Some(404 | 405),
            ..
        } => "URL/model absent",
        SetupTextCheck::Failed {
            kind: None,
            status: None,
        } => "No text reply",
        _ => "Text failed",
    }
}

fn repair_for(check: SetupTextCheck) -> Option<RecoveryAction> {
    match check {
        SetupTextCheck::Failed {
            kind: Some(ProviderFailureKind::Authentication | ProviderFailureKind::PermissionDenied),
            ..
        } => Some(RecoveryAction::ChangeKey),
        SetupTextCheck::Failed {
            status: Some(404 | 405),
            ..
        } => Some(RecoveryAction::ChangeUrl),
        _ => None,
    }
}

impl SetupState {
    pub(super) fn failed_models(&self) -> Vec<String> {
        self.model_ids()
            .unwrap_or_default()
            .into_iter()
            .filter(|model| matches!(self.trials.text_check(model), SetupTextCheck::Failed { .. }))
            .collect()
    }

    pub(super) fn primary_failed(&self) -> bool {
        self.primary_model().is_some_and(|model| {
            matches!(
                self.trials.text_check(&model),
                SetupTextCheck::Failed { .. }
            )
        })
    }

    pub(super) fn verified_defaults(&self) -> Vec<String> {
        self.model_ids()
            .unwrap_or_default()
            .into_iter()
            .filter(|model| self.trials.text_check(model) == SetupTextCheck::Ready)
            .collect()
    }

    pub(super) fn open_recovery(&mut self) {
        let failed = self.failed_models();
        self.models.cursor = 0;
        match failed.as_slice() {
            [] => {}
            [model] => {
                self.recovery_model = Some(model.clone());
                self.step = SetupStep::RecoveryActions;
            }
            _ => self.step = SetupStep::RecoveryModels,
        }
    }

    pub(super) fn recovery_actions(&self) -> Vec<RecoveryAction> {
        let Some(model) = self.recovery_model.as_deref() else {
            return Vec::new();
        };
        let check = self.trials.text_check(model);
        let mut actions = Vec::new();
        match (self.subscription.is_some(), check) {
            (
                true,
                SetupTextCheck::Failed {
                    kind: Some(ProviderFailureKind::Authentication),
                    ..
                },
            ) => {
                actions.push(RecoveryAction::SignIn);
            }
            (false, check) => actions.extend(repair_for(check)),
            _ => {}
        }
        actions.push(RecoveryAction::Retry);
        if self.models.selected.len() > 1
            && self.models.selected.contains(model)
            && (self.intent == SetupIntent::Add || self.primary_model().as_deref() != Some(model))
        {
            actions.push(RecoveryAction::Skip);
        }
        actions
    }

    pub(super) fn recovery_input(&mut self, key: KeyEvent, settings: &Settings) {
        match self.step {
            SetupStep::RecoveryModels | SetupStep::RecoveryDefault => {
                let choices = if self.step == SetupStep::RecoveryModels {
                    self.failed_models()
                } else {
                    self.verified_defaults()
                };
                if key.code != KeyCode::Enter {
                    self.models.move_cursor(key.code, choices.len());
                    return;
                }
                let Some(model) = choices.get(self.models.cursor).cloned() else {
                    return;
                };
                if self.step == SetupStep::RecoveryModels {
                    self.recovery_model = Some(model);
                    self.models.cursor = 0;
                    self.step = SetupStep::RecoveryActions;
                } else {
                    self.models.default = Some(model);
                    self.reasoning_changed = false;
                    self.output_changed = false;
                    self.return_to_review();
                }
            }
            SetupStep::RecoveryActions => {
                let actions = self.recovery_actions();
                if key.code != KeyCode::Enter {
                    self.models.move_cursor(key.code, actions.len());
                    return;
                }
                if let Some(action) = actions.get(self.models.cursor).copied() {
                    self.apply_recovery(action, settings);
                }
            }
            _ => {}
        }
    }

    fn apply_recovery(&mut self, action: RecoveryAction, settings: &Settings) {
        let Some(model) = self.recovery_model.clone() else {
            return;
        };
        match action {
            RecoveryAction::Retry => {
                let Ok(connection) = self.connection(settings) else {
                    return;
                };
                self.return_to_review();
                if self.trials.retry_text(&model, &connection) {
                    self.probe_repaint_at = Some(std::time::Instant::now());
                } else if !self.protocol_fixed {
                    self.start_api_check(settings);
                }
            }
            RecoveryAction::Skip => {
                let primary = self.primary_model().as_deref() == Some(model.as_str());
                self.models.selected.remove(&model);
                if primary {
                    self.models.default = None;
                    self.reasoning_changed = false;
                    self.output_changed = false;
                }
                self.trials.skip_model(&model);
                self.return_to_review();
            }
            RecoveryAction::ChangeKey => {
                self.repair = Some(RepairDraft::Key {
                    source: self.credential_source,
                    stored: self.stored_key.text().to_owned(),
                    environment: self.environment.text().to_owned(),
                });
                self.step = SetupStep::CredentialSource;
            }
            RecoveryAction::ChangeUrl => {
                self.repair = Some(RepairDraft::Url {
                    address: self.address.text().to_owned(),
                    full_url: self.full_url,
                    protocol: self.protocol,
                    protocol_fixed: self.protocol_fixed,
                });
                self.step = SetupStep::Address;
            }
            RecoveryAction::SignIn => {
                if let Some(account) = &self.subscription {
                    self.problem = Some(format!(
                        "On the service host, run kuku auth login {} {}, then retry.",
                        account.channel().as_str(),
                        account.name().unwrap_or("ACCOUNT")
                    ));
                }
            }
        }
    }

    pub(super) fn cancel_repair_at_start(&mut self) -> bool {
        if !matches!(
            (&self.repair, self.step),
            (Some(RepairDraft::Key { .. }), SetupStep::CredentialSource)
                | (Some(RepairDraft::Url { .. }), SetupStep::Address)
        ) {
            return false;
        }
        if let Some(repair) = self.repair.take() {
            match repair {
                RepairDraft::Key {
                    source,
                    stored,
                    environment,
                } => {
                    self.credential_source = source;
                    self.stored_key.set_text(stored);
                    self.environment.set_text(environment);
                    self.refresh_environment_detection();
                }
                RepairDraft::Url {
                    address,
                    full_url,
                    protocol,
                    protocol_fixed,
                } => {
                    self.address.set_text(address);
                    self.full_url = full_url;
                    self.protocol = protocol;
                    self.protocol_fixed = protocol_fixed;
                }
            }
        }
        self.models.cursor = 0;
        self.step = SetupStep::RecoveryActions;
        true
    }

    pub(super) fn apply_connection_repair(&mut self, settings: &Settings) -> bool {
        if self.repair.take().is_none() {
            return false;
        }
        self.trials.stop();
        self.trials = ProbeBatch::default();
        self.return_to_review();
        self.review_models(settings);
        true
    }

    pub(super) fn show_api_check_failure(&mut self) {
        let Some(row) = self.trials.rows().first() else {
            return;
        };
        if let Probe::ApiType { model, .. } = &row.probe {
            if let (
                ProbeProgress::Finished(_, Err(_)),
                SetupTextCheck::Failed { kind: Some(_), .. },
            ) = (&row.progress, self.trials.text_check(model))
            {
                self.return_to_review();
            }
        }
    }

    fn return_to_review(&mut self) {
        self.review_selection = 0;
        self.models.cursor = 0;
        self.problem = None;
        self.step = SetupStep::Review;
    }
}
