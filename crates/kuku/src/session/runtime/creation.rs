use std::path::Path;

use crate::event::{EventPayload, EventStore};
use crate::{Error, Result};

use super::record::{Command, CreationClaim, Record, State, fingerprint, timestamp};
use super::{AgentConfig, Input, Session, SessionOptions, receipts};

impl Session {
    /// Open a session only if the caller owns its original creation intent.
    pub async fn open_for_creation(
        options: SessionOptions,
        input: &Input,
        config: &AgentConfig,
    ) -> Result<Self> {
        if options.session_id.is_none() {
            return Err(Error::InvalidArgument(
                "a creation claim requires an explicit session identity".into(),
            ));
        }
        let command = creation_command(input, config, options.prompts_dir.as_deref())?;
        Self::open_in_background(options, Some((command, config.clone()))).await
    }

    /// Verify a replay of the original creation intent against an open session.
    pub fn verify_creation(
        &self,
        input: &Input,
        config: &AgentConfig,
        prompts_dir: Option<&Path>,
    ) -> Result<()> {
        let command = creation_command(input, config, prompts_dir)?;
        let state = self.inner.state.lock().unwrap();
        verify_claim(&state, self.id(), &command)
    }
}

pub(super) fn prepare_journal(
    writer: &mut EventStore,
    state: &mut State,
    id: &str,
    command: Option<Command>,
    prompts_dir: Option<&Path>,
) -> Result<()> {
    let head = writer.last_id();
    if let Some(command) = command {
        if head == 0 {
            let record = Record::SessionClaimed {
                claim: CreationClaim {
                    command,
                    created_at: timestamp()?,
                    prompts_dir: prompts_dir.map(Path::to_path_buf),
                },
            };
            let event = writer.append(record.payload()?)?;
            state.reduce(event.id, record)?;
        } else {
            verify_claim(state, id, &command)?;
        }
    }
    if head == 0 || head == 1 && state.creation.is_some() {
        let ts = match &state.creation {
            Some(claim) => claim.created_at.clone(),
            None => timestamp()?,
        };
        writer.append(EventPayload::SessionCreated {
            ts: ts.clone(),
            schema_version: 3,
            session_id: id.into(),
            created_at: ts,
            kuku_version: env!("CARGO_PKG_VERSION").into(),
        })?;
    }
    Ok(())
}

fn creation_command(
    input: &Input,
    config: &AgentConfig,
    prompts_dir: Option<&Path>,
) -> Result<Command> {
    let input_digest = receipts::input_fingerprint(input)?;
    Ok(Command {
        command_id: input.command_id.clone(),
        fingerprint: fingerprint(&("session_creation", input_digest, config, prompts_dir))?,
    })
}

fn verify_claim(state: &State, id: &str, command: &Command) -> Result<()> {
    if state.creation.as_ref().is_some_and(|claim| {
        claim.command.command_id == command.command_id
            && claim.command.fingerprint == command.fingerprint
    }) {
        Ok(())
    } else {
        Err(Error::SessionCreationConflict(id.into()))
    }
}
