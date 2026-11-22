use std::fs;
use std::io;
use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent};
use kuku::util::private_directory;

use super::config::{unsafe_storage, Settings};
use super::state::{SetupState, SetupStep};
use super::{CredentialSource, Finish, SetupMode};

impl Settings {
    pub(super) fn key_store_inside_workspace(&self) -> bool {
        self.credentials_dir.starts_with(&self.workspace)
    }

    pub(super) fn use_trial_workspace(&mut self) -> io::Result<()> {
        let data = self.data_dir.canonicalize()?;
        let trial = data.join("trial-workspace");
        private_directory(&trial)?;
        let trial = trial.canonicalize()?;
        if trial.parent() != Some(data.as_path()) {
            return Err(unsafe_storage());
        }
        self.check_storage_at(&trial, false)?;
        self.workspace = trial;
        self.storage_error = None;
        Ok(())
    }

    pub(super) fn can_store_key(&self) -> bool {
        self.storage_error.is_none()
    }

    pub(super) fn storage_problem(&self) -> &'static str {
        self.storage_error
            .unwrap_or("Stored keys are unavailable; use the Environment source instead.")
    }

    pub(super) fn check_storage(&self, create: bool) -> io::Result<()> {
        self.check_storage_at(&self.workspace, create)
    }

    fn check_storage_at(&self, workspace: &Path, create: bool) -> io::Result<()> {
        let candidate = self.credentials_dir.as_path();
        let canonical_data = self.data_dir.canonicalize()?;
        let canonical_workspace = workspace.canonicalize()?;
        let resolved_candidate = match fs::symlink_metadata(candidate) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(unsafe_storage());
                }
                candidate.canonicalize()?
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                canonical_data.join("credentials")
            }
            Err(_) => return Err(unsafe_storage()),
        };
        if resolved_candidate.starts_with(&canonical_workspace) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "credential storage must be outside the workspace",
            ));
        }
        if create {
            private_directory(candidate).map_err(|_| unsafe_storage())?;
        } else if candidate.is_dir() {
            private_directory(candidate).map_err(|_| unsafe_storage())?;
        }
        Ok(())
    }
}

impl SetupState {
    pub(super) fn offers_trial_workspace(settings: &Settings, mode: SetupMode) -> bool {
        mode == SetupMode::FirstRun && settings.key_store_inside_workspace()
    }

    pub(super) fn trial_workspace_opened(&mut self, settings: &Settings) {
        self.can_store_key = settings.can_store_key();
        self.trial_workspace_selected = false;
        self.credential_source = CredentialSource::StoredKey;
        self.saved_key_available = settings.saved_key_available(self.choice());
        self.problem = None;
        self.step = SetupStep::Credential;
    }

    pub(super) fn choose_credential_source(
        &mut self,
        key: KeyEvent,
        settings: &Settings,
        mode: SetupMode,
    ) {
        match key.code {
            KeyCode::Up | KeyCode::Left | KeyCode::Down | KeyCode::Right
                if Self::offers_trial_workspace(settings, mode) =>
            {
                self.trial_workspace_selected = !self.trial_workspace_selected;
                self.problem = None;
            }
            KeyCode::Up | KeyCode::Left | KeyCode::Down | KeyCode::Right => {
                let source = match self.credential_source {
                    CredentialSource::Environment => CredentialSource::StoredKey,
                    CredentialSource::StoredKey => CredentialSource::Environment,
                };
                if source == CredentialSource::StoredKey
                    && !settings.can_store_key()
                    && !self.saved_key_available
                {
                    return;
                }
                self.credential_source = source;
                if source == CredentialSource::Environment {
                    self.refresh_environment_detection();
                }
                self.review_selection = 0;
                self.problem = None;
            }
            KeyCode::Tab | KeyCode::Enter
                if self.trial_workspace_selected
                    && Self::offers_trial_workspace(settings, mode) =>
            {
                self.finish = Some(Finish::UseTrialWorkspace);
            }
            KeyCode::Tab | KeyCode::Enter => {
                self.problem = None;
                self.step = SetupStep::Credential;
            }
            _ => {}
        }
    }
}
