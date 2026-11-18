use std::path::Path;

use kuku::session::{NamedValue, NamedValueSource};
use kuku::{AgentConfig, Session, SessionOptions};

use super::{RunManager, SessionOperation};
use crate::error_mapping::{ApiError, Result};

pub(crate) struct SaveValue {
    pub(crate) source: NamedValueSource,
    pub(crate) replace: bool,
    pub(crate) create: Option<AgentConfig>,
}

impl RunManager {
    pub(crate) async fn save_named_value(
        &self,
        options: SessionOptions,
        path: &Path,
        agent: &str,
        name: &str,
        value: SaveValue,
    ) -> Result<NamedValue> {
        let id = options.session_id.as_deref().expect("explicit Session ID");
        let slot = self.slot(path, &options.workspace, id);
        let open = slot.open.lock().await;
        let _operation = SessionOperation::begin(&slot.active_operation);
        if let Some(entry) = open.as_ref() {
            return Ok(entry
                .session
                .save_named_value(agent, name, value.source, value.replace)
                .await?);
        }
        let result = save_closed_value(options, path, agent, name, value).await;
        self.release_slot(path, &slot);
        result
    }
}

async fn save_closed_value(
    options: SessionOptions,
    path: &Path,
    agent: &str,
    name: &str,
    value: SaveValue,
) -> Result<NamedValue> {
    if !path.is_file() && value.create.is_none() {
        return Err(ApiError::session_not_found());
    }
    let session = Session::open(options).await?;
    let saved = save_value(&session, agent, name, value).await;
    let closed = session.close().await;
    let value = saved?;
    closed?;
    Ok(value)
}

async fn save_value(
    session: &Session,
    agent: &str,
    name: &str,
    value: SaveValue,
) -> kuku::Result<NamedValue> {
    if let Some(config) = value.create {
        if session.agents().is_empty() {
            session.create_agent("http:agent:named-value", agent, config)?;
        }
    }
    session
        .save_named_value(agent, name, value.source, value.replace)
        .await
}
