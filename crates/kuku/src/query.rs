use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::error::{Error, Result};
use crate::event::{EventPayload, EventStore, StoredEvent};
use crate::session::{
    current_workspace, kuku_home, new_session_id, session_events_path, validate_session_id,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    prompt: String,
    session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutput {
    pub session_id: String,
    pub text: String,
}

/// Host-facing runtime event stream.
///
/// This enum is non-exhaustive; hosts must keep a fallback arm when matching it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiEvent {
    Done { output: RunOutput },
}

/// Host-facing session run handle.
///
/// `Run` intentionally does not implement `Clone` or equality traits, so hosts
/// cannot duplicate or compare the event stream handle.
///
/// ```compile_fail
/// # async fn assert_run_is_not_clone_or_eq() -> kuku::Result<()> {
/// let run = kuku::query("hello").start().await?;
/// let _duplicate = run.clone();
/// let _same = &run == &run;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct Run {
    session_id: String,
    done: Option<RunOutput>,
}

pub fn query(prompt: impl Into<String>) -> Query {
    Query::new(prompt)
}

impl Query {
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            session_id: None,
        }
    }

    pub fn session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Starts a session writer for this query.
    ///
    /// Callers must not start two writers for the same session concurrently.
    pub async fn start(self) -> Result<Run> {
        let kuku_home = kuku_home()?;
        let workspace = current_workspace()?;
        let session_id = match self.session_id {
            Some(session_id) => {
                validate_session_id(&session_id)?;
                session_id
            }
            None => new_session_id(),
        };
        validate_session_id(&session_id)?;

        let events_path = session_events_path(&kuku_home, &workspace, &session_id)?;
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
            text: self.prompt,
        })?;

        let output = RunOutput {
            session_id: session_id.clone(),
            text: String::new(),
        };

        Ok(Run {
            session_id,
            done: Some(output),
        })
    }

    pub async fn run(self) -> Result<RunOutput> {
        let mut run = self.start().await?;
        let mut output = RunOutput {
            session_id: run.session_id().to_string(),
            text: String::new(),
        };

        while let Some(event) = run.next().await? {
            match event {
                UiEvent::Done { output: done } => output = done,
            }
        }

        Ok(output)
    }
}

impl Run {
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub async fn next(&mut self) -> Result<Option<UiEvent>> {
        Ok(self.done.take().map(|output| UiEvent::Done { output }))
    }
}

fn validate_existing_session(events: &[StoredEvent]) -> Result<()> {
    if events.is_empty() {
        return Ok(());
    }

    match events.first().map(|event| &event.payload) {
        Some(EventPayload::SessionMeta { .. }) => Ok(()),
        _ => Err(Error::InvalidEventStream(
            "first event must be session.meta".to_string(),
        )),
    }
}

fn next_turn(events: &[StoredEvent]) -> u64 {
    events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::TurnStart { turn, .. } => Some(*turn),
            _ => None,
        })
        .max()
        .unwrap_or(0)
        + 1
}

fn now_timestamp() -> Result<String> {
    Ok(OffsetDateTime::now_utc().format(&Rfc3339)?)
}
