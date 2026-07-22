use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use kuku::event::{
    ActivityFact, ActivityKindFact, ActivityStatusFact, ContextBreakdown, ConversationContextFact,
    ConversationId, ExactContentBlock, ExactMessage, ExactRequest, ExactRequestParameters,
    FileReferenceFact, InteractionChoiceFact, InteractionFact, InteractionId, MessageRole,
    ObservationFact, ObservationKind, ObservationRetention, ProviderFact, ProviderUsage,
    RequestCause, RequestCompleted, RequestId, RequestScope, RequestSnapshot, RequestStarted,
    RevisionToken, RunId, SkillContextFact, SkillLoadFact, SkillLoadOrigin, SourceFact,
    SourceScope, TaskEvent, TaskId, ThinkingConfig, TurnId, WorkspaceId, WorkspaceRelativePath,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, watch};

use crate::run_manager::driver::{
    DriverCommand, DriverEvent as RuntimeDriverEvent, DriverHandle, DriverStart, RunDriverFactory,
    RunFailure, RunResult,
};
use crate::run_manager::DomainError;

use super::{BarrierOutcome, ScenarioControl};

const CORE_TASK_FIXTURE: &str = include_str!("../../tests/fixtures/scenarios/core_task.json");
const FULL_TASK_FIXTURE: &str = include_str!("../../tests/fixtures/scenarios/full_task.json");
const HUMAN_ACCEPTANCE_FIXTURE: &str =
    include_str!("../../tests/fixtures/scenarios/human_acceptance.json");

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
        /// Typed file arguments passed to the tool.
        arguments: ScenarioToolArguments,
    },
    /// A project Skill loaded by the Agent while assembling the request.
    AgentSkillLoaded {
        /// Stable catalog Skill identifier.
        skill_id: String,
        /// Typed Skill source and exact content.
        source: ScenarioSkillSource,
    },
    /// A deterministic sequence of older timeline activities.
    TimelineHistory {
        /// Number of unique timeline items to emit.
        count: u32,
        /// Maximum number of activities sent in one runtime event.
        batch_size: u16,
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

/// Canonical file input for a scenario Tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScenarioToolArguments {
    /// Contained workspace-relative file path.
    pub path: WorkspaceRelativePath,
}

/// Canonical project Skill input for a scenario request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScenarioSkillSource {
    /// Contained project-relative Skill definition path.
    pub relative_path: WorkspaceRelativePath,
    /// Exact Skill definition loaded by the Agent.
    pub content: String,
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
        let fixture = match name {
            "core_task" => serde_json::from_str(CORE_TASK_FIXTURE)
                .map_err(|error| ScenarioError::InvalidFixture(error.to_string()))?,
            "full_task" => feature_fixture(FULL_TASK_FIXTURE)?,
            "human_acceptance" => feature_fixture(HUMAN_ACCEPTANCE_FIXTURE)?,
            _ => return Err(ScenarioError::UnknownFixture(name.to_owned())),
        };
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FeatureFixture {
    name: String,
    provider: FeatureProvider,
    workspace: FeatureWorkspace,
    task: FeatureTask,
    review: serde_json::Value,
    barriers: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FeatureProvider {
    model: String,
    response: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FeatureWorkspace {
    kind: serde_json::Value,
    git_branch: Option<String>,
    baseline_files: Vec<FeatureFile>,
    working_files: Vec<FeatureFile>,
    revision_update: FeatureFile,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FeatureFile {
    path: String,
    content: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FeatureTask {
    message: String,
}

fn feature_fixture(source: &'static str) -> Result<ScenarioFixture, ScenarioError> {
    let input: FeatureFixture = serde_json::from_str(source)
        .map_err(|error| ScenarioError::InvalidFixture(error.to_string()))?;
    let file = input.workspace.working_files.first();
    let path = file.map_or("README.md", |file| file.path.as_str());
    let arguments = ScenarioToolArguments {
        path: WorkspaceRelativePath::parse(path)
            .map_err(|error| ScenarioError::InvalidFixture(error.to_string()))?,
    };
    let observation = file.map_or_else(String::new, |file| file.content.clone());
    let skill = input
        .workspace
        .working_files
        .iter()
        .find(|file| file.path == ".agents/skills/status/SKILL.md")
        .ok_or_else(|| ScenarioError::InvalidFixture("status Skill is missing".to_owned()))?;
    let skill_source = ScenarioSkillSource {
        relative_path: WorkspaceRelativePath::parse(&skill.path)
            .map_err(|error| ScenarioError::InvalidFixture(error.to_string()))?,
        content: skill.content.clone(),
    };
    let _ = (
        input.workspace.kind,
        input.workspace.git_branch,
        input.workspace.baseline_files,
        input.workspace.revision_update,
    );
    let _ = input.review;
    Ok(ScenarioFixture {
        name: input.name,
        provider: input.provider.model,
        events: vec![
            DriverEvent::AgentSkillLoaded {
                skill_id: "skill:project:status".to_owned(),
                source: skill_source,
            },
            DriverEvent::ProviderResponse {
                text: format!("I will inspect the workspace for: {}", input.task.message),
            },
            DriverEvent::ToolCall {
                name: "read_file".to_owned(),
                arguments,
            },
            DriverEvent::Interaction {
                name: "permission".to_owned(),
                payload: serde_json::json!({
                    "prompt": "Permission request"
                }),
            },
            DriverEvent::TimelineHistory {
                count: 10_000,
                batch_size: 250,
            },
            DriverEvent::DelegatedConversation {
                conversation_id: "scenario-helper".to_owned(),
                summary: if observation.is_empty() {
                    "The workspace observation is empty.".to_owned()
                } else {
                    "A deterministic helper inspected the workspace.".to_owned()
                },
            },
            DriverEvent::Usage {
                input_tokens: 5,
                output_tokens: 10,
            },
            DriverEvent::ProviderResponse {
                text: input.provider.response,
            },
        ],
        barriers: input.barriers,
    })
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
    control: ScenarioControl,
    starts: Arc<Mutex<Vec<DriverStart>>>,
}

impl ScenarioDriverFactory {
    /// Loads a named fixture and seeds all generated time and identities.
    pub fn from_fixture(name: &str, seed: u64) -> Result<Self, ScenarioError> {
        let fixture = ScenarioFixture::embedded(name)?;
        let fixture_source = match name {
            "core_task" => CORE_TASK_FIXTURE,
            "full_task" => FULL_TASK_FIXTURE,
            "human_acceptance" => HUMAN_ACCEPTANCE_FIXTURE,
            _ => return Err(ScenarioError::UnknownFixture(name.to_owned())),
        };
        let control = ScenarioControl::new(fixture.barriers.clone());
        Ok(Self {
            fixture,
            fixture_source,
            clock: DeterministicClock::seeded(seed),
            ids: ScenarioIds::seeded(seed),
            position: 0,
            control,
            starts: Arc::new(Mutex::new(Vec::new())),
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

    /// Returns the controls bound to this seeded fixture.
    pub fn control(&self) -> ScenarioControl {
        self.control.clone()
    }

    /// Returns whether the real runtime started the named Run with its scoped Query input.
    pub fn started_with_scope(&self, run_id: &RunId) -> bool {
        self.starts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .any(|start| start.run_id == *run_id && start.execution_scope.run_id == *run_id)
    }

    /// Advances the clock and emits the next typed provider event.
    pub fn next_event(&mut self) -> Option<(u64, DriverEvent)> {
        let event = self.fixture.events.get(self.position)?.clone();
        self.position += 1;
        Some((self.clock.advance(1), event))
    }
}

impl RunDriverFactory for ScenarioDriverFactory {
    fn start(
        &self,
        start: DriverStart,
    ) -> Pin<Box<dyn Future<Output = Result<DriverHandle, DomainError>> + Send>> {
        let fixture = self.fixture.clone();
        let control = self.control.clone();
        let seed = self.clock.seed;
        self.starts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(start.clone());
        Box::pin(async move {
            let (commands, command_rx) = mpsc::channel(8);
            let (cancel, mut cancellation) = watch::channel(false);
            let (event_tx, events) = mpsc::channel(64);
            event_tx
                .send(RuntimeDriverEvent::Started)
                .await
                .map_err(|_| DomainError::RunNotActive)?;
            tokio::spawn(async move {
                let driver = run_scenario_driver(
                    fixture,
                    control,
                    seed,
                    start,
                    command_rx,
                    event_tx.clone(),
                );
                tokio::pin!(driver);
                tokio::select! {
                    _ = &mut driver => {}
                    changed = cancellation.changed() => {
                        if changed.is_ok() && *cancellation.borrow() {
                            let _ = event_tx.send(RuntimeDriverEvent::Stopped).await;
                        }
                    }
                }
            });
            Ok(DriverHandle {
                cancel,
                commands,
                events,
            })
        })
    }
}

async fn run_scenario_driver(
    fixture: ScenarioFixture,
    control: ScenarioControl,
    seed: u64,
    start: DriverStart,
    mut commands: mpsc::Receiver<DriverCommand>,
    events: mpsc::Sender<RuntimeDriverEvent>,
) {
    let request_scope = RequestScope {
        execution: start.execution_scope.clone(),
        request_id: request_id_for_run(&start.execution_scope.run_id),
    };
    let initial = request_events(&fixture, &start, &request_scope);
    if events
        .send(RuntimeDriverEvent::Activity(initial))
        .await
        .is_err()
    {
        return;
    }

    let mut summary = String::new();
    let mut usage = ProviderUsage {
        input_tokens: None,
        output_tokens: None,
        cached_input_tokens: None,
        cache_creation_input_tokens: None,
    };
    let mut ids = ScenarioIds::seeded(seed);
    for (index, event) in fixture.events.into_iter().enumerate() {
        match event {
            DriverEvent::ProviderResponse { text } => {
                summary = text.clone();
                if events
                    .send(RuntimeDriverEvent::Activity(vec![
                        TaskEvent::MessagePatched {
                            message_id: start.agent_message_id.clone(),
                            append_text: text,
                            finalized: false,
                            request_ids: None,
                        },
                    ]))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            DriverEvent::ToolCall { name, arguments } => {
                let activity_id = format!("scenario-tool-{seed}-{index}");
                let file_reference = FileReferenceFact {
                    workspace_id: start.workspace_id.clone(),
                    relative_path: arguments.path.clone(),
                    label: arguments.path.as_str().to_owned(),
                };
                let activity = ActivityFact {
                    activity_id: activity_id.clone(),
                    run_id: start.run_id.clone(),
                    title: name,
                    kind: ActivityKindFact::Tool,
                    status: ActivityStatusFact::Running,
                    detail: Some(
                        serde_json::json!({ "path": arguments.path.as_str() }).to_string(),
                    ),
                    conversation_id: None,
                    agent: None,
                    tier: None,
                    result_in_main: None,
                    file_references: vec![file_reference],
                };
                let observation = ObservationFact {
                    scope: request_scope.clone(),
                    tool_call_id: activity_id.clone(),
                    kind: ObservationKind::FileRead,
                    relative_path: Some(arguments.path.clone()),
                    observed_hash: None,
                    range: None,
                    retention: ObservationRetention::Retained,
                    summary: format!("Observed {}", arguments.path.as_str()),
                };
                if events
                    .send(RuntimeDriverEvent::Activity(vec![
                        TaskEvent::ActivityUpserted {
                            activity: activity.clone(),
                        },
                        TaskEvent::ObservationRecorded(observation),
                    ]))
                    .await
                    .is_err()
                {
                    return;
                }
                let mut completed = activity;
                completed.status = ActivityStatusFact::Completed;
                if events
                    .send(RuntimeDriverEvent::Activity(vec![
                        TaskEvent::ActivityUpserted {
                            activity: completed,
                        },
                    ]))
                    .await
                    .is_err()
                {
                    return;
                }
                if fixture.barriers.iter().any(|name| name == "after-tool")
                    && !wait_for_barrier(&control, "after-tool", &mut commands, &events).await
                {
                    return;
                }
            }
            DriverEvent::AgentSkillLoaded { .. } => {}
            DriverEvent::TimelineHistory { count, batch_size } => {
                if !send_timeline_history(&start, seed, count, batch_size, &events).await {
                    return;
                }
            }
            DriverEvent::Interaction { name, payload } => {
                let interaction_id = ids.interaction_id();
                let interaction = InteractionFact {
                    interaction_id: interaction_id.clone(),
                    run_id: start.run_id.clone(),
                    prompt: payload
                        .get("prompt")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(&name)
                        .to_owned(),
                    choices: vec![InteractionChoiceFact {
                        choice_id: "approve".to_owned(),
                        label: "Approve".to_owned(),
                    }],
                    selected_choice_id: None,
                };
                if events
                    .send(RuntimeDriverEvent::InteractionOpened(interaction))
                    .await
                    .is_err()
                    || !wait_for_interaction(&interaction_id, &mut commands, &events).await
                {
                    return;
                }
            }
            DriverEvent::DelegatedConversation {
                summary: detail, ..
            } => {
                let activity = ActivityFact {
                    activity_id: format!("scenario-agent-{seed}-{index}"),
                    run_id: start.run_id.clone(),
                    title: "Delegated scenario check".to_owned(),
                    kind: ActivityKindFact::DelegatedAgent,
                    status: ActivityStatusFact::Completed,
                    detail: None,
                    conversation_id: Some(ids.conversation_id()),
                    agent: Some("scenario-agent".to_owned()),
                    tier: Some(start.tier_id.clone()),
                    result_in_main: Some(!detail.is_empty()),
                    file_references: Vec::new(),
                };
                if events
                    .send(RuntimeDriverEvent::Activity(vec![
                        TaskEvent::ActivityUpserted { activity },
                    ]))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            DriverEvent::Usage {
                input_tokens,
                output_tokens,
            } => {
                usage.input_tokens = Some(input_tokens);
                usage.output_tokens = Some(output_tokens);
            }
        }
    }

    if fixture
        .barriers
        .iter()
        .any(|name| name == "continuity-before-finish")
        && !wait_for_barrier(&control, "continuity-before-finish", &mut commands, &events).await
    {
        return;
    }
    let completed = vec![
        TaskEvent::RequestCompleted(RequestCompleted {
            scope: request_scope.clone(),
            usage,
            elapsed_ms: Some(1),
            provider_request_id: Some(format!("scenario-{seed}")),
            cost: None,
        }),
        TaskEvent::MessagePatched {
            message_id: start.agent_message_id,
            append_text: String::new(),
            finalized: true,
            request_ids: Some(vec![request_scope.request_id]),
        },
    ];
    if events
        .send(RuntimeDriverEvent::Activity(completed))
        .await
        .is_err()
    {
        return;
    }
    let _ = events
        .send(RuntimeDriverEvent::Completed(RunResult {
            summary,
            warnings: Vec::new(),
            checks: None,
            metrics: None,
            workspace_changes: None,
        }))
        .await;
}

fn request_id_for_run(run_id: &RunId) -> RequestId {
    let suffix = run_id
        .as_str()
        .strip_prefix("run_")
        .expect("validated run id has the canonical prefix");
    RequestId::parse(format!("req_{suffix}"))
        .expect("run-derived deterministic request id is valid")
}

async fn send_timeline_history(
    start: &DriverStart,
    seed: u64,
    count: u32,
    batch_size: u16,
    events: &mpsc::Sender<RuntimeDriverEvent>,
) -> bool {
    let batch_size = batch_size.max(1);
    for batch_start in (0..count).step_by(usize::from(batch_size)) {
        let batch_end = count.min(batch_start.saturating_add(u32::from(batch_size)));
        let batch = (batch_start..batch_end)
            .map(|ordinal| TaskEvent::ActivityUpserted {
                activity: ActivityFact {
                    activity_id: format!("scenario-history-{seed}-{ordinal:05}"),
                    run_id: start.run_id.clone(),
                    title: format!("Scenario history item {ordinal:05}"),
                    kind: ActivityKindFact::System,
                    status: ActivityStatusFact::Completed,
                    detail: None,
                    conversation_id: None,
                    agent: None,
                    tier: None,
                    result_in_main: None,
                    file_references: Vec::new(),
                },
            })
            .collect();
        if events
            .send(RuntimeDriverEvent::Activity(batch))
            .await
            .is_err()
        {
            return false;
        }
    }
    true
}

fn request_events(
    fixture: &ScenarioFixture,
    start: &DriverStart,
    scope: &RequestScope,
) -> Vec<TaskEvent> {
    let loaded_skills = fixture
        .events
        .iter()
        .filter_map(|event| match event {
            DriverEvent::AgentSkillLoaded { skill_id, source } => {
                let content_hash =
                    format!("sha256:{:x}", Sha256::digest(source.content.as_bytes()));
                Some(SkillLoadFact {
                    execution: start.execution_scope.clone(),
                    caused_by_request_id: Some(scope.request_id.clone()),
                    skill_id: skill_id.clone(),
                    source: SourceFact {
                        scope: SourceScope::Project,
                        id: "skill-source:project:status".to_owned(),
                        relative_path: Some(source.relative_path.clone()),
                    },
                    origin: SkillLoadOrigin::Agent,
                    content_hash,
                })
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut context_skills = start.selected_skills.clone();
    context_skills.extend(loaded_skills.iter().map(|skill| SkillContextFact {
        skill_id: skill.skill_id.clone(),
        source: skill.source.clone(),
        origin: skill.origin,
        content_hash: skill.content_hash.clone(),
    }));
    let exact = ExactRequest {
        messages: vec![ExactMessage {
            role: MessageRole::User,
            content: vec![ExactContentBlock::Text {
                text: start.prompt.clone(),
            }],
        }],
        tools: Vec::new(),
        parameters: ExactRequestParameters {
            model: fixture.provider.clone(),
            max_output_tokens: None,
            temperature: None,
            stream: true,
            thinking: ThinkingConfig::Disabled,
        },
    };
    let hash = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&exact).expect("scenario request serializes"))
    );
    let cause = RequestCause::UserSubmission;
    loaded_skills
        .into_iter()
        .map(TaskEvent::SkillLoaded)
        .chain([
            TaskEvent::RequestSnapshot(RequestSnapshot {
                scope: scope.clone(),
                cause: cause.clone(),
                provider: ProviderFact::Anthropic,
                tier_id: start.tier_id.clone(),
                exact,
                context: ContextBreakdown {
                    skills: context_skills,
                    instructions: Vec::new(),
                    memory: Vec::new(),
                    conversation: ConversationContextFact {
                        retained_turns: start.execution_scope.turn_index.saturating_sub(1),
                        handoff_boundaries: 0,
                        history_summarized: false,
                        delegated_results: Vec::new(),
                    },
                    observations: Vec::new(),
                    delegated_results: Vec::new(),
                    capabilities: Vec::new(),
                    token_estimate: None,
                },
                catalog_revision: RevisionToken::parse("0".repeat(64))
                    .expect("zero revision is valid"),
                exact_payload_hash: hash,
            }),
            TaskEvent::RequestStarted(RequestStarted {
                scope: scope.clone(),
                cause,
                provider: ProviderFact::Anthropic,
                model: fixture.provider.clone(),
                started_at: format!(
                    "2026-07-20T00:00:{:02}Z",
                    start.execution_scope.turn_index % 60
                ),
            }),
        ])
        .collect()
}

async fn wait_for_barrier(
    control: &ScenarioControl,
    name: &str,
    commands: &mut mpsc::Receiver<DriverCommand>,
    events: &mpsc::Sender<RuntimeDriverEvent>,
) -> bool {
    loop {
        tokio::select! {
            outcome = control.wait(name) => match outcome {
                Ok(BarrierOutcome::Released) => return true,
                Ok(BarrierOutcome::Failed(summary)) => {
                    let _ = events.send(RuntimeDriverEvent::Failed(RunFailure { summary })).await;
                    return false;
                }
                Err(error) => {
                    let _ = events.send(RuntimeDriverEvent::Failed(RunFailure {
                        summary: error.to_string(),
                    })).await;
                    return false;
                }
            },
            command = commands.recv() => match command {
                Some(DriverCommand::Stop) | None => {
                    let _ = events.send(RuntimeDriverEvent::Stopped).await;
                    return false;
                }
                Some(DriverCommand::Resolve { .. }) => {}
            }
        }
    }
}

async fn wait_for_interaction(
    expected: &InteractionId,
    commands: &mut mpsc::Receiver<DriverCommand>,
    events: &mpsc::Sender<RuntimeDriverEvent>,
) -> bool {
    loop {
        match commands.recv().await {
            Some(DriverCommand::Resolve { interaction_id, .. }) if interaction_id == *expected => {
                return events
                    .send(RuntimeDriverEvent::InteractionClosed(interaction_id))
                    .await
                    .is_ok();
            }
            Some(DriverCommand::Resolve { .. }) => {}
            Some(DriverCommand::Stop) | None => {
                let _ = events.send(RuntimeDriverEvent::Stopped).await;
                return false;
            }
        }
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
