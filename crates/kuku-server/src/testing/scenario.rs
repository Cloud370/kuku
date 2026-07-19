use std::fmt;

use kuku::event::{ConversationId, InteractionId, RequestId, RunId, TaskId, TurnId, WorkspaceId};
use serde::{Deserialize, Serialize};

const CORE_TASK_FIXTURE: &str = include_str!("../../tests/fixtures/scenarios/core_task.json");

/// Errors raised while loading or validating an acceptance scenario.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScenarioError {
    /// The requested embedded fixture is not part of this scenario packet.
    UnknownFixture(String),
    /// The named fixture did not parse as a valid scenario.
    InvalidFixture(String),
}

impl fmt::Display for ScenarioError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownFixture(name) => write!(formatter, "unknown scenario fixture: {name}"),
            Self::InvalidFixture(message) => {
                write!(formatter, "invalid scenario fixture: {message}")
            }
        }
    }
}

impl std::error::Error for ScenarioError {}

/// A provider response and driver event sequence used by the core task scenario.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DriverEvent {
    /// Text returned by the provider.
    ProviderResponse {
        /// Provider text content.
        text: String,
    },
    /// A typed tool invocation emitted by the provider.
    ToolCall {
        /// Tool name.
        name: String,
        /// JSON arguments passed to the tool.
        arguments: serde_json::Value,
    },
    /// A pending interaction that the command path must preserve.
    Interaction {
        /// Stable interaction kind.
        name: String,
        /// JSON interaction payload.
        payload: serde_json::Value,
    },
    /// A delegated conversation observed during the run.
    DelegatedConversation {
        /// Opaque delegated conversation identity.
        conversation_id: String,
        /// Human-readable summary carried by the driver.
        summary: String,
    },
    /// Usage accounting emitted by the provider boundary.
    Usage {
        /// Input token count.
        input_tokens: u64,
        /// Output token count.
        output_tokens: u64,
    },
}

/// The fixture consumed by [`ScenarioDriverFactory`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScenarioFixture {
    /// Fixture name.
    pub name: String,
    /// Provider label used by the deterministic driver.
    pub provider: String,
    /// Typed driver events emitted in order.
    pub events: Vec<DriverEvent>,
    /// Named barriers available to the authenticated test control surface.
    pub barriers: Vec<String>,
}

impl ScenarioFixture {
    /// Loads an embedded fixture by its stable name.
    pub fn embedded(name: &str) -> Result<Self, ScenarioError> {
        let source = match name {
            "core_task" => CORE_TASK_FIXTURE,
            _ => return Err(ScenarioError::UnknownFixture(name.to_owned())),
        };

        let fixture: Self = serde_json::from_str(source)
            .map_err(|error| ScenarioError::InvalidFixture(error.to_string()))?;
        fixture.validate(name)?;
        Ok(fixture)
    }

    fn validate(&self, requested_name: &str) -> Result<(), ScenarioError> {
        if self.name != requested_name {
            return Err(ScenarioError::InvalidFixture(format!(
                "fixture name is {}, requested {requested_name}",
                self.name
            )));
        }
        if self.provider.is_empty() || self.events.is_empty() {
            return Err(ScenarioError::InvalidFixture(
                "provider and events must be non-empty".to_owned(),
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        if self.barriers.iter().any(|barrier| !seen.insert(barrier)) {
            return Err(ScenarioError::InvalidFixture(
                "barrier names must be unique".to_owned(),
            ));
        }
        Ok(())
    }
}

/// A reproducible logical clock for scenario event sequencing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeterministicClock {
    seed: u64,
    tick: u64,
}

impl DeterministicClock {
    /// Creates a clock whose first timestamp is derived only from `seed`.
    pub fn seeded(seed: u64) -> Self {
        Self { seed, tick: 0 }
    }

    /// Returns the current logical timestamp.
    pub fn now(&self) -> u64 {
        self.seed.saturating_add(self.tick)
    }

    /// Advances the clock by `ticks` and returns its new timestamp.
    pub fn advance(&mut self, ticks: u64) -> u64 {
        self.tick = self.tick.saturating_add(ticks);
        self.now()
    }
}

/// Deterministic opaque SDK identities for a scenario run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScenarioIds {
    seed: u64,
    task: u32,
    run: u32,
    turn: u32,
    request: u32,
    interaction: u32,
    conversation: u32,
    workspace: u32,
}

impl ScenarioIds {
    /// Creates an identity generator from a stable seed.
    pub fn seeded(seed: u64) -> Self {
        Self {
            seed,
            task: 0,
            run: 0,
            turn: 0,
            request: 0,
            interaction: 0,
            conversation: 0,
            workspace: 0,
        }
    }

    fn value(&self, prefix: &str, ordinal: u32) -> String {
        format!("{prefix}{:016x}{ordinal:08x}", self.seed)
    }

    /// Returns the next deterministic task identity.
    pub fn task_id(&mut self) -> TaskId {
        self.task = self.task.saturating_add(1);
        TaskId::parse(self.value("tsk_", self.task)).expect("deterministic task id is valid")
    }

    /// Returns the next deterministic run identity.
    pub fn run_id(&mut self) -> RunId {
        self.run = self.run.saturating_add(1);
        RunId::parse(self.value("run_", self.run)).expect("deterministic run id is valid")
    }

    /// Returns the next deterministic turn identity.
    pub fn turn_id(&mut self) -> TurnId {
        self.turn = self.turn.saturating_add(1);
        TurnId::parse(self.value("trn_", self.turn)).expect("deterministic turn id is valid")
    }

    /// Returns the next deterministic request identity.
    pub fn request_id(&mut self) -> RequestId {
        self.request = self.request.saturating_add(1);
        RequestId::parse(self.value("req_", self.request))
            .expect("deterministic request id is valid")
    }

    /// Returns the next deterministic interaction identity.
    pub fn interaction_id(&mut self) -> InteractionId {
        self.interaction = self.interaction.saturating_add(1);
        InteractionId::parse(self.value("int_", self.interaction))
            .expect("deterministic interaction id is valid")
    }

    /// Returns the next deterministic conversation identity.
    pub fn conversation_id(&mut self) -> ConversationId {
        self.conversation = self.conversation.saturating_add(1);
        ConversationId::parse(self.value("con_", self.conversation))
            .expect("deterministic conversation id is valid")
    }

    /// Returns the next deterministic workspace identity.
    pub fn workspace_id(&mut self) -> WorkspaceId {
        self.workspace = self.workspace.saturating_add(1);
        WorkspaceId::parse(self.value("wsp_", self.workspace))
            .expect("deterministic workspace id is valid")
    }
}

/// Deterministic event source used by the real task runtime in scenario builds.
#[derive(Debug, Clone)]
pub struct ScenarioDriverFactory {
    fixture: ScenarioFixture,
    fixture_source: &'static str,
    clock: DeterministicClock,
    ids: ScenarioIds,
    position: usize,
}

impl ScenarioDriverFactory {
    /// Loads a named fixture and seeds all generated time and identities.
    pub fn from_fixture(name: &str, seed: u64) -> Result<Self, ScenarioError> {
        let fixture = ScenarioFixture::embedded(name)?;
        let fixture_source = match name {
            "core_task" => CORE_TASK_FIXTURE,
            _ => return Err(ScenarioError::UnknownFixture(name.to_owned())),
        };
        Ok(Self {
            fixture,
            fixture_source,
            clock: DeterministicClock::seeded(seed),
            ids: ScenarioIds::seeded(seed),
            position: 0,
        })
    }

    /// Returns the immutable fixture definition.
    pub fn fixture(&self) -> &ScenarioFixture {
        &self.fixture
    }

    /// Returns the original fixture JSON for provenance checks.
    pub fn fixture_source(&self) -> &'static str {
        self.fixture_source
    }

    /// Returns the scenario clock.
    pub fn clock(&self) -> DeterministicClock {
        self.clock
    }

    /// Returns the scenario identity generator.
    pub fn ids(&mut self) -> &mut ScenarioIds {
        &mut self.ids
    }

    /// Advances the clock and emits the next typed provider event.
    pub fn next_event(&mut self) -> Option<(u64, DriverEvent)> {
        let event = self.fixture.events.get(self.position)?.clone();
        self.position += 1;
        Some((self.clock.advance(1), event))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_contains_driver_inputs_not_projection_records() {
        let factory = ScenarioDriverFactory::from_fixture("core_task", 7).unwrap();
        assert!(!factory.fixture_source().contains("TaskProjection"));
        assert!(!factory.fixture_source().contains("ledger"));
        assert!(factory.fixture().events.len() >= 4);
    }

    #[test]
    fn equal_seed_produces_equal_driver_trace_and_ids() {
        let mut left = ScenarioDriverFactory::from_fixture("core_task", 7).unwrap();
        let mut right = ScenarioDriverFactory::from_fixture("core_task", 7).unwrap();
        let left_events: Vec<_> = std::iter::from_fn(|| left.next_event()).collect();
        let right_events: Vec<_> = std::iter::from_fn(|| right.next_event()).collect();
        assert_eq!(left_events, right_events);
        assert_eq!(left.ids().task_id(), right.ids().task_id());
        assert_eq!(left.ids().run_id(), right.ids().run_id());
    }

    #[test]
    fn different_seeds_keep_ids_distinct() {
        let mut left = ScenarioIds::seeded(7);
        let mut right = ScenarioIds::seeded(8);
        assert_ne!(left.task_id(), right.task_id());
    }
}
