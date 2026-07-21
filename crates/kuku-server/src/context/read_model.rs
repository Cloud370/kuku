use std::collections::BTreeMap;
use std::sync::Arc;

use kuku::context::UsageReductionError;
use kuku::event::{
    CapabilityFact, ContextBreakdown, ConversationContextFact, ConversationId, MemoryKind,
    ObservationFact, RequestId, RequestSnapshot, RequestStarted, TaskEvent, TaskId, TaskRevision,
    WorkspaceId,
};

use crate::api::{
    AgentSummary, AgentThread, ApiVersion, CapabilityProjection, CatalogQuery, ContextCatalog,
    ContextHealth, ContextHealthLevel, ContextSections, ContextSnapshot, ContextWarning,
    ContextWarningCode, ConversationContext, DelegatedAgentStatus, DiscoverableContext,
    InstructionContextItem, MemoryContextItem, MessageProjection, ObservationContextItem,
    ObservationDrift, RequestStatus, RequestSummary, SkillContextItem, TierSummary,
};
use crate::run_manager::DomainError;

use super::agent_thread::reduce_agent_thread;
use super::catalog_reducer::CatalogReducer;
use super::observation_contract::ObservationState;
use super::observation_reducer::{ObservationHashProvider, ObservationReducer};
use super::skill_selection::WorkspaceCatalogProvider;
use super::usage_reducer::UsageReducer;

const MAX_REQUEST_HISTORY: usize = 100;
const MAX_CATALOG_SEARCH_BYTES: usize = 4_096;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AgentThreadFacts {
    pub(crate) task_id: TaskId,
    pub(crate) conversation_id: ConversationId,
    pub(crate) agent: AgentSummary,
    pub(crate) tier: TierSummary,
    pub(crate) status: DelegatedAgentStatus,
    pub(crate) result_in_main: bool,
    pub(crate) messages: Vec<MessageProjection>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ContextTaskFacts {
    pub(crate) task_id: TaskId,
    pub(crate) workspace_id: WorkspaceId,
    pub(crate) task_revision: TaskRevision,
    pub(crate) events: Vec<TaskEvent>,
    pub(crate) agent_threads: Vec<AgentThreadFacts>,
}

pub(crate) trait TaskContextSource: Send + Sync {
    fn facts(&self, task_id: &TaskId) -> Result<ContextTaskFacts, ContextReadError>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ContextReadError {
    #[error("invalid context request")]
    InvalidRequest,
    #[error("task does not exist")]
    TaskNotFound,
    #[error("request does not exist")]
    RequestNotFound,
    #[error("conversation does not exist")]
    ConversationNotFound,
    #[error("context ledger facts are corrupt")]
    LedgerCorrupt,
    #[error("workspace does not exist")]
    WorkspaceNotFound,
    #[error("context usage facts are corrupt")]
    UsageCorrupt,
}

impl From<DomainError> for ContextReadError {
    fn from(error: DomainError) -> Self {
        match error {
            DomainError::TaskNotFound | DomainError::TaskNotCreated => Self::TaskNotFound,
            DomainError::WorkspaceNotFound => Self::WorkspaceNotFound,
            _ => Self::LedgerCorrupt,
        }
    }
}

impl From<UsageReductionError> for ContextReadError {
    fn from(_error: UsageReductionError) -> Self {
        Self::UsageCorrupt
    }
}

pub(crate) struct ContextReadModel {
    tasks: Arc<dyn TaskContextSource>,
    catalogs: Arc<dyn WorkspaceCatalogProvider>,
    hashes: Arc<dyn ObservationHashProvider>,
}

impl ContextReadModel {
    pub(crate) fn new(
        tasks: Arc<dyn TaskContextSource>,
        catalogs: Arc<dyn WorkspaceCatalogProvider>,
        hashes: Arc<dyn ObservationHashProvider>,
    ) -> Self {
        Self {
            tasks,
            catalogs,
            hashes,
        }
    }

    pub(crate) fn snapshot(
        &self,
        task_id: &TaskId,
        selected_request: Option<&RequestId>,
    ) -> Result<ContextSnapshot, ContextReadError> {
        let facts = self.tasks.facts(task_id)?;
        if facts.task_id != *task_id {
            return Err(ContextReadError::LedgerCorrupt);
        }
        let entries = self.catalogs.for_workspace(&facts.workspace_id)?;
        let catalog = CatalogReducer::reduce(&entries);
        let requests = Requests::reduce(&facts.events)?;
        let selected = requests.select(selected_request)?;
        let latest = requests.latest();
        let selected_snapshot = selected
            .as_ref()
            .and_then(|request| requests.snapshots.get(&request.request_id));
        let next_request_base = latest
            .as_ref()
            .and_then(|request| requests.snapshots.get(&request.request_id))
            .map(|snapshot| snapshot.context.clone())
            .unwrap_or_else(empty_breakdown);
        let observation_facts = facts
            .events
            .iter()
            .filter_map(|event| match event {
                TaskEvent::ObservationRecorded(fact) => Some(fact.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let (sections, observation_states) = sections(
            selected_snapshot,
            &facts.workspace_id,
            &observation_facts,
            &catalog,
            &facts.agent_threads,
            self.hashes.as_ref(),
        );
        let usage = UsageReducer::from_lifecycle(facts.events.clone())?
            .for_context(selected.as_ref().map(|value| &value.request_id))?;
        let (health, warnings) = health(selected_snapshot, &sections, &observation_states);
        let request_history_truncated = requests.history.len() > MAX_REQUEST_HISTORY;
        let request_history = requests
            .history
            .iter()
            .skip(requests.history.len().saturating_sub(MAX_REQUEST_HISTORY))
            .cloned()
            .collect();
        Ok(ContextSnapshot {
            api_version: ApiVersion,
            task_id: task_id.clone(),
            task_revision: facts.task_revision,
            selected_request: selected,
            request_history,
            request_history_truncated,
            sections,
            next_request_base,
            discoverable: DiscoverableContext {
                catalog_revision: catalog.revision.clone(),
                skill_count: catalog.skills.len() as u64,
                agent_count: catalog.agents.len() as u64,
                tool_count: catalog.tools.len() as u64,
            },
            usage,
            health,
            warnings,
            exact_request: selected_snapshot.map(|snapshot| snapshot.exact.clone()),
            exact_payload_hash: selected_snapshot
                .map(|snapshot| snapshot.exact_payload_hash.clone()),
        })
    }

    pub(crate) fn catalog(
        &self,
        workspace_id: &WorkspaceId,
        query: CatalogQuery,
    ) -> Result<ContextCatalog, ContextReadError> {
        if query
            .search
            .as_ref()
            .is_some_and(|search| search.len() > MAX_CATALOG_SEARCH_BYTES)
        {
            return Err(ContextReadError::InvalidRequest);
        }
        let entries = self.catalogs.for_workspace(workspace_id)?;
        let mut catalog = CatalogReducer::reduce(&entries);
        if let Some(search) = query.search.map(|value| value.trim().to_lowercase()) {
            if !search.is_empty() {
                catalog.tiers.retain(|entry| {
                    contains(&entry.tier.tier_id, &search)
                        || contains(&entry.tier.label, &search)
                        || contains(&entry.tier.purpose, &search)
                });
                catalog.skills.retain(|entry| {
                    contains(&entry.skill_id, &search)
                        || contains(&entry.name, &search)
                        || contains(&entry.description, &search)
                });
                catalog.agents.retain(|entry| {
                    contains(&entry.agent.agent_id, &search)
                        || contains(&entry.agent.name, &search)
                        || contains(&entry.agent.description, &search)
                });
                catalog.tools.retain(|entry| {
                    contains(&entry.tool_id, &search)
                        || contains(&entry.name, &search)
                        || contains(&entry.description, &search)
                });
            }
        }
        Ok(catalog)
    }

    pub(crate) fn agent_thread(
        &self,
        task_id: &TaskId,
        conversation_id: &ConversationId,
    ) -> Result<AgentThread, ContextReadError> {
        let facts = self.tasks.facts(task_id)?;
        if facts.task_id != *task_id {
            return Err(ContextReadError::LedgerCorrupt);
        }
        let thread = facts
            .agent_threads
            .iter()
            .find(|thread| thread.conversation_id == *conversation_id)
            .ok_or(ContextReadError::ConversationNotFound)?;
        if thread.task_id != *task_id {
            return Err(ContextReadError::LedgerCorrupt);
        }
        reduce_agent_thread(task_id, thread)
    }
}

struct Requests {
    history: Vec<RequestSummary>,
    snapshots: BTreeMap<RequestId, RequestSnapshot>,
}

impl Requests {
    fn reduce(events: &[TaskEvent]) -> Result<Self, ContextReadError> {
        let mut starts = BTreeMap::<RequestId, RequestStarted>::new();
        let mut order = Vec::new();
        let mut snapshots = BTreeMap::new();
        let mut statuses = BTreeMap::new();
        for event in events {
            match event {
                TaskEvent::RequestSnapshot(snapshot) => {
                    insert_identical(
                        &mut snapshots,
                        snapshot.scope.request_id.clone(),
                        snapshot.clone(),
                    )?;
                }
                TaskEvent::RequestStarted(started) => {
                    if !starts.contains_key(&started.scope.request_id) {
                        order.push(started.scope.request_id.clone());
                    }
                    insert_identical(
                        &mut starts,
                        started.scope.request_id.clone(),
                        started.clone(),
                    )?;
                    statuses
                        .entry(started.scope.request_id.clone())
                        .or_insert(RequestStatus::Started);
                }
                TaskEvent::RequestCompleted(completed) => {
                    insert_status(
                        &mut statuses,
                        completed.scope.request_id.clone(),
                        RequestStatus::Completed,
                    )?;
                }
                TaskEvent::RequestFailed(failed) => {
                    insert_status(
                        &mut statuses,
                        failed.scope.request_id.clone(),
                        RequestStatus::Failed,
                    )?;
                }
                _ => {}
            }
        }
        if snapshots
            .keys()
            .any(|request_id| !starts.contains_key(request_id))
            || statuses
                .keys()
                .any(|request_id| !starts.contains_key(request_id))
        {
            return Err(ContextReadError::LedgerCorrupt);
        }
        let history = order
            .into_iter()
            .map(|request_id| {
                let started = starts
                    .get(&request_id)
                    .ok_or(ContextReadError::LedgerCorrupt)?;
                let snapshot = snapshots
                    .get(&request_id)
                    .ok_or(ContextReadError::LedgerCorrupt)?;
                if snapshot.scope != started.scope
                    || snapshot.cause != started.cause
                    || snapshot.provider != started.provider
                    || snapshot.exact.parameters.model != started.model
                {
                    return Err(ContextReadError::LedgerCorrupt);
                }
                Ok(RequestSummary {
                    request_id: request_id.clone(),
                    run_id: started.scope.execution.run_id.clone(),
                    turn_id: started.scope.execution.turn_id.clone(),
                    conversation_id: started.scope.execution.conversation_id.clone(),
                    status: statuses
                        .get(&request_id)
                        .copied()
                        .unwrap_or(RequestStatus::Started),
                    cause: started.cause.clone(),
                    provider: started.provider,
                    model: started.model.clone(),
                    started_at: started.started_at.clone(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { history, snapshots })
    }

    fn latest(&self) -> Option<RequestSummary> {
        self.history.last().cloned()
    }

    fn select(
        &self,
        selected: Option<&RequestId>,
    ) -> Result<Option<RequestSummary>, ContextReadError> {
        match selected {
            Some(request_id) => self
                .history
                .iter()
                .find(|request| request.request_id == *request_id)
                .cloned()
                .map(Some)
                .ok_or(ContextReadError::RequestNotFound),
            None => Ok(self.latest()),
        }
    }
}

fn sections(
    snapshot: Option<&RequestSnapshot>,
    workspace_id: &WorkspaceId,
    observations: &[ObservationFact],
    catalog: &ContextCatalog,
    agent_threads: &[AgentThreadFacts],
    hashes: &dyn ObservationHashProvider,
) -> (ContextSections, Vec<ObservationState>) {
    let breakdown = snapshot.map(|value| &value.context);
    let skills = breakdown
        .into_iter()
        .flat_map(|value| &value.skills)
        .map(|fact| {
            let entry = catalog
                .skills
                .iter()
                .find(|entry| entry.skill_id == fact.skill_id);
            SkillContextItem {
                skill_id: fact.skill_id.clone(),
                name: entry.map_or_else(|| fact.skill_id.clone(), |entry| entry.name.clone()),
                description: entry.map_or_else(String::new, |entry| entry.description.clone()),
                source: fact.source.clone(),
                origin: fact.origin,
                content_hash: fact.content_hash.clone(),
            }
        })
        .collect();
    let instructions = breakdown
        .into_iter()
        .flat_map(|value| &value.instructions)
        .map(|fact| InstructionContextItem {
            kind: fact.kind,
            source: fact.source.clone(),
            content_hash: fact.content_hash.clone(),
            label: format!("{:?} instructions", fact.kind),
        })
        .collect();
    let memory = breakdown
        .into_iter()
        .flat_map(|value| &value.memory)
        .map(|fact| MemoryContextItem {
            kind: fact.kind,
            source: fact.source.clone(),
            content_hash: fact.content_hash.clone(),
            label: memory_label(fact.kind).to_owned(),
        })
        .collect();
    let conversation = breakdown
        .map(|value| ConversationContext {
            retained_turns: value.conversation.retained_turns,
            handoff_boundaries: value.conversation.handoff_boundaries,
            history_summarized: value.conversation.history_summarized,
            delegated_results: value.conversation.delegated_results.clone(),
        })
        .unwrap_or_else(empty_conversation);
    let projected_observations = snapshot
        .map(|snapshot| {
            let request_observations = if observations.is_empty() {
                snapshot.context.observations.clone()
            } else {
                observations.to_vec()
            };
            ObservationReducer::for_request(
                &snapshot.scope,
                workspace_id,
                &request_observations,
                hashes,
            )
        })
        .unwrap_or_default();
    let observation_states = projected_observations
        .iter()
        .map(|value| value.current_drift)
        .collect();
    let observations = projected_observations
        .into_iter()
        .map(|value| ObservationContextItem {
            request_id: value.fact.scope.request_id,
            tool_call_id: value.fact.tool_call_id,
            kind: value.fact.kind,
            relative_path: value.fact.relative_path,
            retention: value.fact.retention,
            current_drift: observation_drift(value.current_drift),
            summary: value.fact.summary,
        })
        .collect();
    let agents = agent_threads
        .iter()
        .map(|thread| crate::api::DelegatedAgentProjection {
            conversation_id: thread.conversation_id.clone(),
            agent: thread.agent.clone(),
            tier: thread.tier.clone(),
            status: thread.status,
            result_in_main: thread.result_in_main,
        })
        .collect();
    let capabilities = breakdown
        .into_iter()
        .flat_map(|value| &value.capabilities)
        .map(capability)
        .collect();
    (
        ContextSections {
            skills,
            instructions,
            memory,
            conversation,
            observations,
            agents,
            capabilities,
        },
        observation_states,
    )
}

fn observation_drift(state: ObservationState) -> ObservationDrift {
    match state {
        ObservationState::Present => ObservationDrift::Present,
        ObservationState::ChangedSinceObservation => ObservationDrift::ChangedSinceObservation,
        ObservationState::NoLongerPresent => ObservationDrift::NoLongerPresent,
        ObservationState::Inaccessible => ObservationDrift::Inaccessible,
        ObservationState::NotApplicable => ObservationDrift::NotApplicable,
    }
}

fn health(
    snapshot: Option<&RequestSnapshot>,
    sections: &ContextSections,
    states: &[ObservationState],
) -> (ContextHealth, Vec<ContextWarning>) {
    let source_drift_count = states
        .iter()
        .filter(|state| {
            matches!(
                state,
                ObservationState::ChangedSinceObservation | ObservationState::NoLongerPresent
            )
        })
        .count() as u64;
    let source_inaccessible = states.contains(&ObservationState::Inaccessible);
    let truncated_observation_count = sections
        .observations
        .iter()
        .filter(|item| item.retention == kuku::event::ObservationRetention::Truncated)
        .count() as u64;
    let summarized = sections.conversation.history_summarized;
    let mut warnings = Vec::new();
    if summarized {
        warnings.push(warning(
            ContextWarningCode::HistorySummarized,
            "Conversation history was summarized",
        ));
    }
    if truncated_observation_count > 0 {
        warnings.push(warning(
            ContextWarningCode::ObservationTruncated,
            "One or more observations were truncated",
        ));
    }
    if source_drift_count > 0 {
        warnings.push(warning(
            ContextWarningCode::SourceDrift,
            "One or more observed sources changed",
        ));
    }
    if source_inaccessible {
        warnings.push(warning(
            ContextWarningCode::SourceInaccessible,
            "One or more observed sources are inaccessible",
        ));
    }
    let context_tokens_used = snapshot.and_then(|value| value.context.token_estimate);
    let level = if !warnings.is_empty() {
        ContextHealthLevel::Warning
    } else if context_tokens_used.is_some() {
        ContextHealthLevel::Healthy
    } else {
        ContextHealthLevel::Unavailable
    };
    (
        ContextHealth {
            level,
            context_tokens_used,
            context_token_limit: None,
            context_tokens_remaining: None,
            summarized,
            source_drift_count,
            truncated_observation_count,
        },
        warnings,
    )
}

fn warning(code: ContextWarningCode, summary: &str) -> ContextWarning {
    ContextWarning {
        code,
        summary: summary.to_owned(),
        request_id: None,
        source: None,
    }
}

fn capability(fact: &CapabilityFact) -> CapabilityProjection {
    CapabilityProjection {
        kind: fact.kind,
        state: fact.state,
    }
}

fn empty_breakdown() -> ContextBreakdown {
    ContextBreakdown {
        skills: Vec::new(),
        instructions: Vec::new(),
        memory: Vec::new(),
        conversation: ConversationContextFact {
            retained_turns: 0,
            handoff_boundaries: 0,
            history_summarized: false,
            delegated_results: Vec::new(),
        },
        observations: Vec::new(),
        delegated_results: Vec::new(),
        capabilities: Vec::new(),
        token_estimate: None,
    }
}

fn empty_conversation() -> ConversationContext {
    ConversationContext {
        retained_turns: 0,
        handoff_boundaries: 0,
        history_summarized: false,
        delegated_results: Vec::new(),
    }
}

fn memory_label(kind: MemoryKind) -> &'static str {
    match kind {
        MemoryKind::Global => "Global memory",
        MemoryKind::Project => "Project memory",
    }
}

fn contains(value: &str, search: &str) -> bool {
    value.to_lowercase().contains(search)
}

fn insert_identical<K, V>(
    values: &mut BTreeMap<K, V>,
    key: K,
    value: V,
) -> Result<(), ContextReadError>
where
    K: Ord,
    V: PartialEq,
{
    if values.get(&key).is_some_and(|existing| existing != &value) {
        return Err(ContextReadError::LedgerCorrupt);
    }
    values.entry(key).or_insert(value);
    Ok(())
}

fn insert_status(
    statuses: &mut BTreeMap<RequestId, RequestStatus>,
    request_id: RequestId,
    status: RequestStatus,
) -> Result<(), ContextReadError> {
    if statuses
        .get(&request_id)
        .is_some_and(|existing| !matches!(existing, RequestStatus::Started) && *existing != status)
    {
        return Err(ContextReadError::LedgerCorrupt);
    }
    statuses.insert(request_id, status);
    Ok(())
}
