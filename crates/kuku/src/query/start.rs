use std::sync::Arc;

use crate::config::{self, load_config};
use crate::error::{Error, Result};
use crate::event::{EventPayload, EventStore};
use crate::session::{
    current_workspace, kuku_home, new_session_id, project_policy_path, session_events_path,
    validate_session_id,
};

use super::helpers::{next_turn, now_timestamp, validate_existing_session};
use super::types::{PendingRun, Query, Run, RunOutput, RunState, UiEvent};

impl Query {
    pub async fn start(self) -> Result<Run> {
        self.start_session().await
    }

    async fn start_session(mut self) -> Result<Run> {
        self.validate()?;

        let kuku_home = kuku_home()?;

        let workspace = match self.workspace_path.take() {
            Some(path) => path,
            None => current_workspace()?,
        };

        let config: Arc<crate::config::Config> = match (self.config_obj.take(), &self.config_path)
        {
            (Some(cfg), _) => Arc::new(cfg),
            (None, Some(path)) => {
                let file = load_config(path)?;
                Arc::new(file.resolve()?)
            }
            (None, None) => {
                return Err(Error::MissingProviderConfig(
                    "no config provided; set .config_path() or .config()".to_string(),
                ));
            }
        };
        let session_id = match self.session_id.as_deref() {
            Some(session_id) => {
                validate_session_id(session_id)?;
                session_id.to_string()
            }
            None => new_session_id(),
        };
        validate_session_id(&session_id)?;

        let events_path = session_events_path(&kuku_home, &workspace, &session_id)?;
        let policy_path = project_policy_path(&kuku_home, &workspace)?;
        let existing_events = EventStore::replay(&events_path)?;
        validate_existing_session(&existing_events)?;
        let turn = next_turn(&existing_events);
        let is_new_session = existing_events.is_empty();

        let mut store = EventStore::open(&events_path)?;
        if is_new_session {
            let created_at = now_timestamp()?;
            store.append(EventPayload::SessionMeta {
                ts: created_at.clone(),
                schema_version: 1,
                session_id: session_id.clone(),
                created_at,
                kuku_version: env!("CARGO_PKG_VERSION").to_string(),
            })?;
        }

        store.append(EventPayload::TurnStart {
            turn,
            ts: now_timestamp()?,
        })?;
        store.append(EventPayload::UserInput {
            turn,
            ts: now_timestamp()?,
            text: self.prompt.clone(),
        })?;

        Ok(Run {
            session_id: session_id.clone(),
            state: RunState::Pending(Box::new(PendingRun {
                session_id,
                query: self,
                events_path,
                kuku_home,
                workspace,
                policy_path,
                turn,
                request_num: 0,
                resolved: None,
                queued_tool_calls: std::collections::VecDeque::new(),
                saved_tool_call: None,
                config,
            })),
        })
    }

    pub async fn run(self) -> Result<RunOutput> {
        let mut run = self.start_session().await?;
        loop {
            match run.next().await? {
                Some(UiEvent::PermissionRequested { .. }) => {
                    run.deny_pending().await?;
                }
                Some(UiEvent::Done { output }) => return Ok(output),
                Some(_) => continue,
                None => {
                    return Err(Error::InvalidEventStream(
                        "run ended without producing Done".to_string(),
                    ))
                }
            }
        }
    }
}
