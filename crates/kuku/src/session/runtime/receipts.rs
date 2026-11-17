use crate::{Error, Result};

use super::record::{fingerprint, validate_address, validate_key};
use super::{AgentConfig, Input, InputSource, Receipt, Session};

impl Session {
    /// Find a committed command receipt without accepting or activating work.
    pub fn receipt(&self, command_id: &str) -> Result<Option<Receipt>> {
        validate_key(command_id)?;
        Ok(self
            .inner
            .state
            .lock()
            .unwrap()
            .commands
            .get(command_id)
            .map(|(_, receipt)| receipt.clone()))
    }

    /// Verify an agent creation receipt against the original requested configuration.
    pub fn create_agent_receipt(
        &self,
        command_id: &str,
        address: &str,
        config: &AgentConfig,
    ) -> Result<Option<Receipt>> {
        validate_key(command_id)?;
        validate_address(address)?;
        let digest = fingerprint(&("create_agent", address, config))?;
        self.inner.state.lock().unwrap().retry(command_id, &digest)
    }

    /// Verify an agent configuration receipt against the original requested configuration.
    pub fn configure_agent_receipt(
        &self,
        command_id: &str,
        address: &str,
        config: &AgentConfig,
    ) -> Result<Option<Receipt>> {
        validate_key(command_id)?;
        validate_address(address)?;
        let digest = fingerprint(&("configure_agent", address, config))?;
        self.inner.state.lock().unwrap().retry(command_id, &digest)
    }

    /// Verify a delivered input's receipt without committing or activating work.
    pub fn input_receipt(&self, input: &Input) -> Result<Option<Receipt>> {
        let digest = input_fingerprint(input)?;
        self.inner
            .state
            .lock()
            .unwrap()
            .retry(&input.command_id, &digest)
    }

    /// Verify an authorization reply receipt without committing or resuming its action.
    pub fn reply_permission_receipt(
        &self,
        command_id: &str,
        address: &str,
        request_id: &str,
        allow: bool,
    ) -> Result<Option<Receipt>> {
        let digest = permission_fingerprint(command_id, address, request_id, allow)?;
        self.inner.state.lock().unwrap().retry(command_id, &digest)
    }

    /// Verify a question reply receipt without committing or activating input.
    pub fn reply_question_receipt(
        &self,
        command_id: &str,
        address: &str,
        request_id: &str,
        text: &str,
    ) -> Result<Option<Receipt>> {
        let digest = question_fingerprint(command_id, address, request_id, text)?;
        self.inner.state.lock().unwrap().retry(command_id, &digest)
    }
}

pub(super) fn input_fingerprint(input: &Input) -> Result<String> {
    validate_key(&input.command_id)?;
    validate_address(&input.target)?;
    if let Some(skill) = &input.bootstrap_skill {
        if !crate::skill::loader::is_valid_skill_name(skill) {
            return Err(Error::InvalidArgument(format!(
                "invalid skill name '{skill}'"
            )));
        }
    }
    if input.text.len() > 262_144 {
        return Err(Error::InvalidArgument(
            "input exceeds the supported byte limit".into(),
        ));
    }
    let identity = match &input.source {
        InputSource::Host(identity)
        | InputSource::Agent(identity)
        | InputSource::External(identity) => identity,
    };
    if identity.len() > 256
        || input
            .correlation
            .as_ref()
            .is_some_and(|value| value.len() > 256)
    {
        return Err(Error::InvalidArgument(
            "input source or correlation exceeds 256 bytes".into(),
        ));
    }
    fingerprint(&("send", input))
}

pub(super) fn permission_fingerprint(
    command_id: &str,
    address: &str,
    request_id: &str,
    allow: bool,
) -> Result<String> {
    validate_reply(command_id, address, request_id)?;
    fingerprint(&("reply_permission", address, request_id, allow))
}

pub(super) fn question_fingerprint(
    command_id: &str,
    address: &str,
    request_id: &str,
    text: &str,
) -> Result<String> {
    validate_reply(command_id, address, request_id)?;
    if text.len() > 262_144 {
        return Err(Error::InvalidArgument(
            "question reply exceeds the supported byte limit".into(),
        ));
    }
    fingerprint(&("reply_question", address, request_id, text))
}

fn validate_reply(command_id: &str, address: &str, request_id: &str) -> Result<()> {
    validate_key(command_id)?;
    validate_address(address)?;
    if request_id.is_empty() || request_id.len() > 262_144 {
        return Err(Error::InvalidArgument(
            "request identity is empty or exceeds its supported byte limit".into(),
        ));
    }
    Ok(())
}
