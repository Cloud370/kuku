use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::ErrorKind;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::execution::OutputReference;
use crate::tool::builtin::run_command::output::search::{
    search_open_output, OutputSearch, SearchPage,
};
use crate::tool::builtin::run_command::output::{
    capture_metadata, framed_bytes, read_open_output, validate_output_id, OutputLimits,
    OutputWriter, MAX_FRAME_BYTES,
};
use crate::{Error, Result};

use super::record::{Record, State};
use super::retention::{open_retained_output, output_dir, OutputStore};
use super::{
    ensure_open, load_state, replay_state, Input, InputSource, OutputChunk, OutputRead,
    OutputStream, Session,
};

/// Maximum number of exact output references on a single input.
pub const MAX_INPUT_VALUES: usize = 8;
/// Maximum byte length of an agent-local value name.
pub const MAX_VALUE_NAME_BYTES: usize = 64;

/// A new value or an existing whole output to bind without copying it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum NamedValueSource {
    /// Store a UTF-8 string as raw text.
    Text(String),
    /// Store a serialized JSON value.
    Json(serde_json::Value),
    /// Store literal bytes.
    Bytes(Vec<u8>),
    /// Bind an existing output ID owned by or granted to the selected agent.
    Output(String),
}

/// The format of a named value's retained payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NamedValueKind {
    /// Raw UTF-8 text.
    Text,
    /// Serialized JSON.
    Json,
    /// Literal bytes.
    Bytes,
    /// An existing output captured by the session.
    Output,
}

/// An agent-local name bound to immutable retained bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedValue {
    /// Exact, case-sensitive local name.
    pub name: String,
    /// Payload format used when the binding was saved.
    pub kind: NamedValueKind,
    /// Whole retained output to read explicitly by ID.
    pub reference: OutputReference,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct NamedBinding {
    pub(super) value: NamedValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    digest: Option<String>,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub(super) struct ValueState {
    names: HashMap<String, BTreeMap<String, NamedBinding>>,
    grants: HashMap<String, HashSet<String>>,
    turns: HashMap<String, (u64, HashSet<String>)>,
}

#[derive(Clone, Copy)]
pub(crate) enum OutputSelector<'a> {
    Id(&'a str),
    Name(&'a str),
}

impl Session {
    /// Save a named value once, or explicitly replace a different binding after durable capture.
    pub async fn save_named_value(
        &self,
        agent: &str,
        name: &str,
        source: NamedValueSource,
        replace: bool,
    ) -> Result<NamedValue> {
        validate_name(name)?;
        let _write = self.inner.value_writes.lock().await;
        let (kind, bytes) = match source {
            NamedValueSource::Output(id) => return self.bind_output(agent, name, &id, replace),
            NamedValueSource::Text(text) => (NamedValueKind::Text, text.into_bytes()),
            NamedValueSource::Json(json) => (NamedValueKind::Json, serde_json::to_vec(&json)?),
            NamedValueSource::Bytes(bytes) => (NamedValueKind::Bytes, bytes),
        };
        Box::pin(self.save_value_bytes(agent, name, kind, &bytes, replace)).await
    }

    /// Resolve the selected agent's current exact binding without loading its payload.
    pub fn named_value(&self, agent: &str, name: &str) -> Result<NamedValue> {
        validate_name(name)?;
        let state = self.inner.state.lock().unwrap();
        state.agent(agent)?;
        Ok(state.values.named_binding(agent, name)?.value.clone())
    }

    /// List the selected agent's current names and references in name order.
    pub fn named_values(&self, agent: &str) -> Result<Vec<NamedValue>> {
        list_named_values(&self.inner.state.lock().unwrap(), agent)
    }

    /// Read named value metadata from a journal without opening a runtime writer.
    pub fn read_named_values(events_path: &Path, agent: &str) -> Result<Vec<NamedValue>> {
        let (state, _) = load_state(events_path)?;
        list_named_values(&state, agent)
    }

    /// Read retained output owned by or explicitly delivered to this agent.
    pub fn read_agent_output(
        &self,
        agent: &str,
        output_id: &str,
        request: &OutputRead,
    ) -> Result<OutputChunk> {
        self.read_selected_output(agent, OutputSelector::Id(output_id), request)
    }

    pub(crate) fn read_selected_output(
        &self,
        agent: &str,
        selector: OutputSelector<'_>,
        request: &OutputRead,
    ) -> Result<OutputChunk> {
        let (file, id) = self.open_selected_output(agent, selector)?;
        Ok(read_open_output(file, &id, request.clone())?)
    }

    pub(crate) fn search_selected_output(
        &self,
        agent: &str,
        selector: OutputSelector<'_>,
        request: OutputSearch<'_>,
        cancelled: impl Fn() -> bool,
    ) -> Result<SearchPage> {
        let (file, id) = self.open_selected_output(agent, selector)?;
        Ok(search_open_output(file, id, request, cancelled)?)
    }

    fn open_selected_output(
        &self,
        agent: &str,
        selector: OutputSelector<'_>,
    ) -> Result<(std::fs::File, String)> {
        let state = self.inner.state.lock().unwrap();
        let id = state.select_output(agent, selector)?;
        state.check_output_access(agent, id)?;
        Ok((
            open_retained_output(&output_dir(self.events_path())?, id)?,
            id.to_owned(),
        ))
    }

    /// Read an agent's granted output without opening a runtime writer.
    pub fn read_saved_agent_output(
        events_path: &Path,
        agent: &str,
        output_id: &str,
        request: &OutputRead,
    ) -> Result<OutputChunk> {
        let file = open_retained_output(&output_dir(events_path)?, output_id)?;
        let (state, _) = replay_state(
            events_path,
            State {
                outputs: OutputStore::one(output_id, file.metadata()?),
                ..State::default()
            },
        )?;
        state.check_output_access(agent, output_id)?;
        Ok(read_open_output(file, output_id, request.clone())?)
    }

    pub(crate) fn pin_readable_output(
        &self,
        agent: &str,
        turn: u64,
        selector: OutputSelector<'_>,
    ) -> Result<String> {
        let mut state = self.inner.state.lock().unwrap();
        ensure_open(&state)?;
        let id = state.select_output(agent, selector)?.to_owned();
        state.check_output_access(agent, &id)?;
        self.pin_outputs(&mut state, agent, turn, [id.as_str()])?;
        Ok(id)
    }

    pub(crate) fn pin_output_selectors(
        &self,
        agent: &str,
        turn: u64,
        selectors: &[OutputSelector<'_>],
    ) -> Result<Vec<OutputReference>> {
        let mut state = self.inner.state.lock().unwrap();
        ensure_open(&state)?;
        let references = selectors
            .iter()
            .map(|selector| {
                self.output_reference(&state, agent, state.select_output(agent, *selector)?)
            })
            .collect::<Result<Vec<_>>>()?;
        self.pin_outputs(
            &mut state,
            agent,
            turn,
            references.iter().map(|reference| reference.id.as_str()),
        )?;
        Ok(references)
    }

    fn pin_outputs<'a>(
        &self,
        state: &mut State,
        agent: &str,
        turn: u64,
        ids: impl IntoIterator<Item = &'a str>,
    ) -> Result<()> {
        let ids = ids
            .into_iter()
            .filter(|id| !state.values.is_pinned_for_turn(agent, turn, id))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if !ids.is_empty() {
            self.commit(
                state,
                Record::OutputsPinned {
                    agent: agent.into(),
                    turn,
                    ids,
                },
            )?;
        }
        Ok(())
    }

    pub(super) fn validate_input_values(
        &self,
        state: &State,
        input: &Input,
        values: &[OutputReference],
    ) -> Result<()> {
        let source = match &input.source {
            InputSource::Agent(agent) => agent,
            InputSource::Host(_) | InputSource::External(_) => &input.target,
        };
        let mut seen = HashSet::new();
        for reference in values {
            if !seen.insert(reference.id.as_str()) {
                return Err(Error::InvalidArgument(format!(
                    "output '{}' occurs more than once in the input",
                    reference.id
                )));
            }
            if self.output_reference(state, source, &reference.id)? != *reference {
                return Err(Error::InvalidArgument(format!(
                    "input reference '{}' must match the entire captured extent",
                    reference.id
                )));
            }
        }
        Ok(())
    }

    fn bind_output(&self, agent: &str, name: &str, id: &str, replace: bool) -> Result<NamedValue> {
        let mut state = self.inner.state.lock().unwrap();
        ensure_open(&state)?;
        let reference = self.output_reference(&state, agent, id)?;
        if let Some(binding) = state.values.find_binding(agent, name) {
            if binding.value.reference == reference {
                return Ok(binding.value.clone());
            }
        }
        self.require_binding_slot(&state, agent, name, replace)?;
        self.commit_binding(
            &mut state,
            agent,
            NamedValueKind::Output,
            name,
            reference,
            None,
        )
    }

    async fn save_value_bytes(
        &self,
        agent: &str,
        name: &str,
        kind: NamedValueKind,
        bytes: &[u8],
        replace: bool,
    ) -> Result<NamedValue> {
        let digest = format!("{:x}", Sha256::digest(bytes));
        let id = {
            let state = self.inner.state.lock().unwrap();
            ensure_open(&state)?;
            state.agent(agent)?;
            if let Some(binding) = state.values.find_binding(agent, name) {
                if binding.value.kind == kind && binding.digest.as_deref() == Some(&digest) {
                    self.output_reference(&state, agent, &binding.value.reference.id)?;
                    return Ok(binding.value.clone());
                }
            }
            self.require_binding_slot(&state, agent, name, replace)?;
            let writer = self.inner.writer.lock().unwrap();
            let event_id = writer
                .as_ref()
                .expect("open session owns its writer")
                .next_id();
            let identity = Sha256::digest(format!("{}:{event_id}:{digest}", self.id()).as_bytes());
            format!("value_{identity:x}")
        };
        let mut reservation =
            self.reserve_retained_output(agent, &id, Some(framed_bytes(bytes.len() as u64)), None)?;
        let mut writer = OutputWriter::create(
            &output_dir(self.events_path())?,
            &id,
            OutputLimits {
                max_output_bytes: reservation.max_bytes(),
                max_preview_bytes: self.inner.options.limits.max_preview_bytes,
            },
        )
        .await?;
        for chunk in bytes.chunks(MAX_FRAME_BYTES) {
            writer.write(OutputStream::Stdout, chunk).await?;
        }
        let metadata = writer.finish(false).await?;
        let mut state = self.inner.state.lock().unwrap();
        ensure_open(&state)?;
        reservation.keep();
        let reference = OutputReference {
            id,
            offset: 0,
            bytes: metadata.stored_bytes,
        };
        self.commit_binding(&mut state, agent, kind, name, reference, Some(digest))
    }

    fn output_reference(&self, state: &State, agent: &str, id: &str) -> Result<OutputReference> {
        state.check_output_access(agent, id)?;
        if state.outputs.writing(id) {
            return Err(Error::InvalidArgument(format!(
                "output '{id}' is still being captured"
            )));
        }
        let metadata = capture_metadata(&output_dir(self.events_path())?, id).map_err(|error| {
            if error.kind() == ErrorKind::NotFound {
                Error::OutputUnavailable(id.into())
            } else {
                error.into()
            }
        })?;
        if !metadata.complete {
            return Err(Error::InvalidArgument(format!(
                "output '{id}' has not been finalized"
            )));
        }
        Ok(OutputReference {
            id: id.into(),
            offset: 0,
            bytes: metadata.stored_bytes,
        })
    }

    fn require_binding_slot(
        &self,
        state: &State,
        agent: &str,
        name: &str,
        replace: bool,
    ) -> Result<()> {
        if state.values.find_binding(agent, name).is_some() {
            if !replace {
                return Err(Error::InvalidArgument(format!(
                    "value '{name}' already binds a different source; set replace to update it"
                )));
            }
        } else if state
            .values
            .names
            .values()
            .map(BTreeMap::len)
            .sum::<usize>()
            >= self.inner.options.limits.max_named_values
        {
            return Err(Error::InvalidArgument(
                "session named value limit reached".into(),
            ));
        }
        Ok(())
    }

    fn commit_binding(
        &self,
        state: &mut State,
        agent: &str,
        kind: NamedValueKind,
        name: &str,
        reference: OutputReference,
        digest: Option<String>,
    ) -> Result<NamedValue> {
        let value = NamedValue {
            name: name.into(),
            kind,
            reference,
        };
        let binding = NamedBinding {
            value: value.clone(),
            digest,
        };
        self.commit(
            state,
            Record::NamedValueSaved {
                agent: agent.into(),
                binding,
            },
        )?;
        Ok(value)
    }
}

impl State {
    fn select_output<'a>(&'a self, agent: &str, selector: OutputSelector<'a>) -> Result<&'a str> {
        match selector {
            OutputSelector::Id(id) => Ok(id),
            OutputSelector::Name(name) => {
                validate_name(name)?;
                Ok(&self.values.named_binding(agent, name)?.value.reference.id)
            }
        }
    }

    fn check_output_access(&self, agent: &str, id: &str) -> Result<()> {
        validate_output_id(id).map_err(|error| Error::InvalidArgument(error.to_string()))?;
        self.agent(agent)?;
        if self.outputs.owned_by(agent, id) || self.values.is_granted(agent, id) {
            return Ok(());
        }
        Err(Error::OutputUnavailable(id.into()))
    }
}

impl ValueState {
    pub(super) fn save(&mut self, agent: String, binding: NamedBinding) {
        self.names
            .entry(agent)
            .or_default()
            .insert(binding.value.name.clone(), binding);
    }

    pub(super) fn grant(&mut self, agent: &str, references: &[OutputReference]) {
        if !references.is_empty() {
            self.grants
                .entry(agent.into())
                .or_default()
                .extend(references.iter().map(|reference| reference.id.clone()));
        }
    }

    pub(super) fn pin(&mut self, agent: String, turn: u64, ids: Vec<String>) {
        let entry = self
            .turns
            .entry(agent)
            .or_insert_with(|| (turn, HashSet::new()));
        if entry.0 != turn {
            *entry = (turn, HashSet::new());
        }
        entry.1.extend(ids);
    }

    pub(super) fn release_turn(&mut self, agent: &str, turn: Option<u64>) {
        if self
            .turns
            .get(agent)
            .is_some_and(|entry| Some(entry.0) == turn)
        {
            self.turns.remove(agent);
        }
    }

    pub(super) fn pinned_outputs(&self) -> HashSet<String> {
        self.names
            .values()
            .flat_map(|names| {
                names
                    .values()
                    .map(|binding| binding.value.reference.id.clone())
            })
            .chain(self.grants.values().flatten().cloned())
            .chain(self.turns.values().flat_map(|(_, ids)| ids.iter().cloned()))
            .collect()
    }

    fn is_granted(&self, agent: &str, id: &str) -> bool {
        self.grants.get(agent).is_some_and(|ids| ids.contains(id))
    }

    fn is_pinned_for_turn(&self, agent: &str, turn: u64, id: &str) -> bool {
        self.turns
            .get(agent)
            .is_some_and(|entry| entry.0 == turn && entry.1.contains(id))
    }

    fn find_binding(&self, agent: &str, name: &str) -> Option<&NamedBinding> {
        self.names.get(agent)?.get(name)
    }

    fn named_binding(&self, agent: &str, name: &str) -> Result<&NamedBinding> {
        self.find_binding(agent, name).ok_or_else(|| {
            Error::InvalidArgument(format!("agent '{agent}' has no named value '{name}'"))
        })
    }
}

fn list_named_values(state: &State, agent: &str) -> Result<Vec<NamedValue>> {
    state.agent(agent)?;
    Ok(state
        .values
        .names
        .get(agent)
        .into_iter()
        .flat_map(BTreeMap::values)
        .map(|binding| binding.value.clone())
        .collect())
}

fn validate_name(name: &str) -> Result<()> {
    let mut bytes = name.bytes();
    if name.len() <= MAX_VALUE_NAME_BYTES
        && bytes
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        Ok(())
    } else {
        Err(Error::InvalidArgument(format!(
            "value name must contain 1-{MAX_VALUE_NAME_BYTES} ASCII letters, digits, '-' or '_' and begin with a letter"
        )))
    }
}
