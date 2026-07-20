use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;

use kuku::context::catalog::CatalogEntries;
use kuku::event::{
    ActivityKindFact, ActivityStatusFact, EventPayload, MessageRoleFact, TaskEvent, TaskId,
    TaskLedgerRecord, WorkspaceId,
};
use sha2::{Digest, Sha256};

use crate::platform::{ConfigService, WorkspaceRegistry};
use crate::run_manager::{DomainError, TaskRuntime};

use super::observation_reducer::{CurrentObservationState, ObservationHashProvider};
use super::read_model::{AgentThreadFacts, ContextReadError, ContextTaskFacts, TaskContextSource};
use super::skill_selection::WorkspaceCatalogProvider;

const MAX_OBSERVATION_HASH_BYTES: u64 = 64 * 1024 * 1024;

pub(crate) struct RuntimeContextSource {
    runtime: Arc<TaskRuntime>,
}

impl RuntimeContextSource {
    pub(crate) fn new(runtime: Arc<TaskRuntime>) -> Self {
        Self { runtime }
    }
}

impl TaskContextSource for RuntimeContextSource {
    fn facts(&self, task_id: &TaskId) -> Result<ContextTaskFacts, ContextReadError> {
        let repository = self.runtime.repository();
        let aggregate = repository
            .rebuild(task_id)
            .map_err(ContextReadError::from)?;
        let persisted_task_id = aggregate
            .task_id()
            .cloned()
            .ok_or(ContextReadError::LedgerCorrupt)?;
        let workspace_id = aggregate
            .workspace_id()
            .cloned()
            .ok_or(ContextReadError::LedgerCorrupt)?;
        let mut events = Vec::<TaskEvent>::new();
        for stored in repository.replay(task_id).map_err(ContextReadError::from)? {
            let EventPayload::TaskLedger(record) = stored.payload else {
                continue;
            };
            let record_events = match &record {
                TaskLedgerRecord::Control(transaction) => transaction.events(),
                TaskLedgerRecord::Activity(batch) => batch.events(),
            };
            events.extend(record_events.iter().cloned());
        }
        let agent_threads = reduce_agent_threads(&persisted_task_id, &events);
        Ok(ContextTaskFacts {
            task_id: persisted_task_id,
            workspace_id,
            task_revision: aggregate.revision(),
            events,
            agent_threads,
        })
    }
}

fn reduce_agent_threads(task_id: &TaskId, events: &[TaskEvent]) -> Vec<AgentThreadFacts> {
    let mut delegated = HashMap::new();
    let mut messages = HashMap::new();
    let mut order = HashMap::new();
    for (index, event) in events.iter().enumerate() {
        let order_key = kuku::event::Cursor::try_new(index as u64 + 1).ok();
        match event {
            TaskEvent::ActivityUpserted { activity }
                if activity.kind == ActivityKindFact::DelegatedAgent
                    && activity.conversation_id.is_some() =>
            {
                delegated.insert(activity.conversation_id.clone().unwrap(), activity.clone());
            }
            TaskEvent::MessageAppended { message } => {
                order.insert(message.message_id.clone(), order_key);
                messages.insert(message.message_id.clone(), message.clone());
            }
            TaskEvent::MessagePatched {
                message_id,
                append_text,
                finalized,
                request_ids,
            } => {
                if let Some(message) = messages.get_mut(message_id) {
                    message.text.push_str(append_text);
                    message.finalized = *finalized;
                    if let Some(request_ids) = request_ids {
                        message.request_ids = request_ids.clone();
                    }
                }
            }
            _ => {}
        }
    }
    delegated
        .into_values()
        .map(|activity| {
            let conversation_id = activity
                .conversation_id
                .clone()
                .expect("delegated activity identity is validated");
            let run_id = activity.run_id.clone();
            let agent_name = activity.agent.clone().unwrap_or_default();
            let tier_name = activity.tier.clone().unwrap_or_default();
            let mut thread_messages = messages
                .values()
                .filter(|message| message.run_id.as_ref() == Some(&run_id))
                .map(|message| {
                    let order_key = order
                        .get(&message.message_id)
                        .and_then(|value| *value)
                        .unwrap_or_else(|| kuku::event::Cursor::try_new(0).expect("zero cursor"));
                    crate::api::MessageProjection {
                        message_id: message.message_id.clone(),
                        role: match message.role {
                            MessageRoleFact::User => crate::api::MessageRole::User,
                            MessageRoleFact::Agent => crate::api::MessageRole::Agent,
                        },
                        text: message.text.clone(),
                        finalized: message.finalized,
                        request_ids: message.request_ids.clone(),
                        file_references: message
                            .file_references
                            .iter()
                            .map(|file| crate::api::FileReferenceProjection {
                                workspace_id: file.workspace_id.clone(),
                                relative_path: file.relative_path.as_str().to_owned(),
                                label: file.label.clone(),
                            })
                            .collect(),
                        order_key,
                    }
                })
                .collect::<Vec<_>>();
            thread_messages.sort_by_key(|message| message.order_key);
            AgentThreadFacts {
                task_id: task_id.clone(),
                conversation_id,
                agent: crate::api::AgentSummary {
                    agent_id: format!("agent:system:{agent_name}"),
                    name: agent_name,
                    description: String::new(),
                },
                tier: crate::api::TierSummary {
                    tier_id: tier_name
                        .strip_prefix("tier:")
                        .map_or_else(|| format!("tier:{tier_name}"), ToOwned::to_owned),
                    label: tier_name.clone(),
                    purpose: String::new(),
                    provider: String::new(),
                    model: String::new(),
                    think: None,
                    is_default: false,
                },
                status: match activity.status {
                    ActivityStatusFact::Pending => crate::api::DelegatedAgentStatus::Queued,
                    ActivityStatusFact::Running => crate::api::DelegatedAgentStatus::Running,
                    ActivityStatusFact::Completed => crate::api::DelegatedAgentStatus::Completed,
                    ActivityStatusFact::Failed => crate::api::DelegatedAgentStatus::Failed,
                },
                result_in_main: activity.result_in_main.unwrap_or(false),
                messages: thread_messages,
            }
        })
        .collect()
}

pub(crate) struct LiveCatalogProvider {
    kuku_home: PathBuf,
    config: Arc<ConfigService>,
    workspaces: Arc<WorkspaceRegistry>,
}

impl LiveCatalogProvider {
    pub(crate) fn new(
        kuku_home: PathBuf,
        config: Arc<ConfigService>,
        workspaces: Arc<WorkspaceRegistry>,
    ) -> Self {
        Self {
            kuku_home,
            config,
            workspaces,
        }
    }
}

impl WorkspaceCatalogProvider for LiveCatalogProvider {
    fn for_workspace(&self, workspace_id: &WorkspaceId) -> Result<CatalogEntries, DomainError> {
        let capability = self
            .workspaces
            .capability(workspace_id)
            .map_err(|_| DomainError::WorkspaceNotFound)?;
        let config = self
            .config
            .resolved_now()
            .ok_or(DomainError::InvalidRequest)?;
        let prompts = kuku::prompt::builtin_prompt_catalog();
        let skills = kuku::skill::build_registry_snapshot_for_host(
            &self.kuku_home,
            capability.process_path(),
            &config,
        )
        .map_err(|_| DomainError::InvalidRequest)?;
        let agents = kuku::agent::registry::AgentRegistry::builder()
            .builtins_for_tiers(&prompts, config.tiers.keys())
            .build_with_discovery(capability.process_path(), &config.discovery)
            .map_err(|_| DomainError::InvalidRequest)?
            .build();
        CatalogEntries::from_registries(
            &config,
            &skills,
            &agents,
            &prompts,
            kuku::tool::builtin_catalog_entries(true, !skills.is_empty())
                .map_err(|_| DomainError::InvalidRequest)?,
            capability.process_path(),
            self.workspaces.generation_now(),
            self.config.generation_now(),
        )
        .map_err(|_| DomainError::InvalidRequest)
    }
}

pub(crate) struct WorkspaceObservationHashes {
    workspaces: Arc<WorkspaceRegistry>,
}

impl WorkspaceObservationHashes {
    pub(crate) fn new(workspaces: Arc<WorkspaceRegistry>) -> Self {
        Self { workspaces }
    }
}

impl ObservationHashProvider for WorkspaceObservationHashes {
    fn current_state(
        &self,
        workspace_id: &WorkspaceId,
        path: &kuku::event::WorkspaceRelativePath,
    ) -> CurrentObservationState {
        let Ok(capability) = self.workspaces.capability(workspace_id) else {
            return CurrentObservationState::Inaccessible;
        };
        let Ok(relative) = capability.resolve(path.as_str()) else {
            return CurrentObservationState::Inaccessible;
        };
        let Ok(file) = capability.open_file(&relative) else {
            return CurrentObservationState::Inaccessible;
        };
        let mut reader = file.take(MAX_OBSERVATION_HASH_BYTES + 1);
        let mut bytes = Vec::new();
        if reader.read_to_end(&mut bytes).is_err()
            || bytes.len() as u64 > MAX_OBSERVATION_HASH_BYTES
        {
            return CurrentObservationState::Inaccessible;
        }
        CurrentObservationState::Present(format!("sha256:{:x}", Sha256::digest(&bytes)))
    }
}

#[cfg(test)]
mod tests {
    use super::reduce_agent_threads;
    use kuku::event::{
        ActivityFact, ActivityKindFact, ActivityStatusFact, ConversationId, MessageFact,
        MessageRoleFact, RunId, TaskEvent, TaskId,
    };

    #[test]
    fn delegated_activity_and_run_messages_restore_a_read_only_thread() {
        let task_id = TaskId::parse("tsk_000000000000000000000001").unwrap();
        let run_id = RunId::parse("run_000000000000000000000001").unwrap();
        let conversation_id = ConversationId::parse("con_000000000000000000000001").unwrap();
        let message = MessageFact {
            message_id: "message-1".to_owned(),
            task_id: task_id.clone(),
            run_id: Some(run_id.clone()),
            role: MessageRoleFact::Agent,
            text: "delegated result".to_owned(),
            finalized: true,
            request_ids: Vec::new(),
            file_references: Vec::new(),
        };
        let activity = ActivityFact {
            activity_id: "activity-1".to_owned(),
            run_id,
            title: "Review".to_owned(),
            kind: ActivityKindFact::DelegatedAgent,
            status: ActivityStatusFact::Completed,
            detail: None,
            conversation_id: Some(conversation_id.clone()),
            agent: Some("review".to_owned()),
            tier: Some("balanced".to_owned()),
            result_in_main: Some(true),
            file_references: Vec::new(),
        };

        let threads = reduce_agent_threads(
            &task_id,
            &[
                TaskEvent::MessageAppended { message },
                TaskEvent::ActivityUpserted { activity },
            ],
        );

        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].conversation_id, conversation_id);
        assert_eq!(threads[0].messages[0].text, "delegated result");
        assert!(threads[0].result_in_main);
    }
}
