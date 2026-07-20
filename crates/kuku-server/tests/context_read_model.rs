pub mod agent {
    pub mod definition {
        pub use kuku::agent::definition::*;
    }

    pub mod registry {
        pub use kuku::agent::registry::*;
    }
}

pub mod api {
    pub use kuku_server::api::*;
}

pub mod config {
    pub use kuku::config::*;
}

pub mod event {
    pub use kuku::event::*;
}

pub mod prompt {
    pub use kuku::prompt::*;
}

pub mod run_manager {
    pub use kuku_server::run_manager::*;
}

pub mod skill {
    pub mod definition {
        pub use kuku::skill::definition::*;
    }

    pub mod registry {
        pub use kuku::skill::registry::*;
    }
}

pub mod catalog_contract {
    pub use crate::sdk_catalog::*;
}

pub mod observation_contract {
    pub use crate::sdk_observations::{ObservationState, ObservationTracker};
}

#[path = "../../kuku/src/context/catalog.rs"]
mod sdk_catalog;
#[path = "../../kuku/src/context/observations.rs"]
#[allow(dead_code)]
mod sdk_observations;

#[path = "../src/context/agent_thread.rs"]
pub mod agent_thread;
#[path = "../src/context/catalog_reducer.rs"]
pub mod catalog_reducer;
#[path = "../src/context/observation_reducer.rs"]
#[allow(dead_code)]
pub mod observation_reducer;
#[path = "../src/context/read_model.rs"]
pub mod read_model;
#[path = "../src/context/skill_selection.rs"]
#[allow(dead_code)]
pub mod skill_selection;
#[path = "../src/context/usage_reducer.rs"]
pub mod usage_reducer;

pub mod context {
    pub use crate::read_model;
}

#[path = "../src/routes/agents.rs"]
#[allow(dead_code)]
pub mod route_agents;
#[path = "../src/routes/catalog.rs"]
#[allow(dead_code)]
pub mod route_catalog;
#[path = "../src/routes/context.rs"]
#[allow(dead_code)]
pub mod route_context;

pub mod routes {
    pub use crate::route_context as context;
}

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use api::{
    AgentSummary, CatalogQuery, ContextHealthLevel, MessageProjection, MessageRole, TierSummary,
};
use catalog_contract::{
    CatalogCapabilities, CatalogEntries, CatalogEntry, CatalogKind, CatalogSource, TierMetadata,
};
use kuku::event::{
    ContextBreakdown, ConversationContextFact, ExactRequest, ExactRequestParameters, ProviderFact,
    RequestCause, RequestCompleted, RequestScope, RequestSnapshot, RequestStarted, TaskEvent,
    TaskId, TaskRevision, ThinkingConfig, WorkspaceId,
};
use observation_reducer::{CurrentObservationState, ObservationHashProvider};
use read_model::{
    AgentThreadFacts, ContextReadError, ContextReadModel, ContextTaskFacts, TaskContextSource,
};
use sdk_observations::{ObservationBuilder, ToolObservation, ToolObservationData};

struct StaticSource {
    facts: ContextTaskFacts,
}

impl TaskContextSource for StaticSource {
    fn facts(&self, task_id: &TaskId) -> Result<ContextTaskFacts, ContextReadError> {
        (self.facts.task_id == *task_id)
            .then_some(self.facts.clone())
            .ok_or(ContextReadError::TaskNotFound)
    }
}

struct StableHashes;

impl ObservationHashProvider for StableHashes {
    fn current_state(&self, _path: &kuku::event::WorkspaceRelativePath) -> CurrentObservationState {
        CurrentObservationState::Present("sha256:observed".to_owned())
    }
}

struct InaccessibleHashes;

impl ObservationHashProvider for InaccessibleHashes {
    fn current_state(&self, _path: &kuku::event::WorkspaceRelativePath) -> CurrentObservationState {
        CurrentObservationState::Inaccessible
    }
}

fn id<T: serde::de::DeserializeOwned>(value: &str) -> T {
    serde_json::from_value(serde_json::Value::String(value.to_owned())).unwrap()
}

fn request_scope(index: u64) -> RequestScope {
    RequestScope {
        execution: kuku::event::ExecutionScope {
            workspace_id: id("wsp_000000000000000000000001"),
            task_id: id("tsk_000000000000000000000001"),
            run_id: id(&format!("run_{index:024x}")),
            turn_id: id(&format!("trn_{index:024x}")),
            conversation_id: id("con_000000000000000000000001"),
            turn_index: index,
        },
        request_id: id(&format!("req_{index:024x}")),
    }
}

fn snapshot(scope: RequestScope, token_estimate: Option<u64>) -> RequestSnapshot {
    RequestSnapshot {
        scope,
        cause: RequestCause::UserSubmission,
        provider: ProviderFact::Anthropic,
        tier_id: "tier:balanced".to_owned(),
        exact: ExactRequest {
            messages: Vec::new(),
            tools: Vec::new(),
            parameters: ExactRequestParameters {
                model: "fixture-model".to_owned(),
                max_output_tokens: None,
                temperature: None,
                stream: true,
                thinking: ThinkingConfig::Disabled,
            },
        },
        context: ContextBreakdown {
            skills: Vec::new(),
            instructions: Vec::new(),
            memory: Vec::new(),
            conversation: ConversationContextFact {
                retained_turns: 1,
                handoff_boundaries: 0,
                history_summarized: false,
                delegated_results: Vec::new(),
            },
            observations: Vec::new(),
            delegated_results: Vec::new(),
            capabilities: Vec::new(),
            token_estimate,
        },
        catalog_revision: kuku::event::RevisionToken::parse("a".repeat(64)).unwrap(),
        exact_payload_hash: "sha256:exact".to_owned(),
    }
}

fn catalog() -> CatalogEntries {
    let tier = CatalogEntry::new(
        CatalogKind::Tier,
        CatalogSource::System,
        "balanced",
        "Balanced",
        kuku::event::SourceFact {
            scope: kuku::event::SourceScope::System,
            id: "tier-config:balanced".to_owned(),
            relative_path: None,
        },
        "sha256:tier",
        "Balanced",
        CatalogCapabilities::selectable(),
    )
    .unwrap()
    .with_tier_metadata(TierMetadata {
        purpose: "General work".to_owned(),
        provider: "fixture-provider".to_owned(),
        model: "fixture-model".to_owned(),
        think: None,
        is_default: true,
    })
    .unwrap();
    CatalogEntries::new(vec![tier], Vec::new(), Vec::new(), Vec::new(), 1, 1).unwrap()
}

fn facts() -> ContextTaskFacts {
    let first = request_scope(1);
    let second = request_scope(2);
    ContextTaskFacts {
        task_id: id("tsk_000000000000000000000001"),
        workspace_id: id("wsp_000000000000000000000001"),
        task_revision: TaskRevision::try_new(2).unwrap(),
        events: vec![
            TaskEvent::RequestSnapshot(snapshot(first.clone(), Some(1_024))),
            TaskEvent::RequestStarted(RequestStarted {
                scope: first.clone(),
                cause: RequestCause::UserSubmission,
                provider: ProviderFact::Anthropic,
                model: "fixture-model".to_owned(),
                started_at: "2026-07-21T00:00:00Z".to_owned(),
            }),
            TaskEvent::RequestCompleted(RequestCompleted {
                scope: first,
                usage: kuku::event::ProviderUsage {
                    input_tokens: Some(10),
                    output_tokens: Some(5),
                    cached_input_tokens: None,
                    cache_creation_input_tokens: None,
                },
                elapsed_ms: Some(5),
                provider_request_id: Some("provider-1".to_owned()),
                cost: None,
            }),
            TaskEvent::RequestSnapshot(snapshot(second.clone(), Some(2_048))),
            TaskEvent::RequestStarted(RequestStarted {
                scope: second.clone(),
                cause: RequestCause::UserSubmission,
                provider: ProviderFact::Anthropic,
                model: "fixture-model".to_owned(),
                started_at: "2026-07-21T00:01:00Z".to_owned(),
            }),
        ],
        agent_threads: vec![AgentThreadFacts {
            task_id: id("tsk_000000000000000000000001"),
            conversation_id: id("con_000000000000000000000002"),
            agent: AgentSummary {
                agent_id: "agent:project:review".to_owned(),
                name: "Review Agent".to_owned(),
                description: "Reviews project changes".to_owned(),
            },
            tier: TierSummary {
                tier_id: "tier:balanced".to_owned(),
                label: "Balanced".to_owned(),
                purpose: "General work".to_owned(),
                provider: "fixture-provider".to_owned(),
                model: "fixture-model".to_owned(),
                think: None,
                is_default: true,
            },
            status: api::DelegatedAgentStatus::Running,
            result_in_main: false,
            messages: Vec::new(),
        }],
    }
}

fn model() -> ContextReadModel {
    ContextReadModel::new(
        Arc::new(StaticSource { facts: facts() }),
        Arc::new(|_: &WorkspaceId| Ok(catalog())),
        Arc::new(StableHashes),
    )
}

#[test]
fn current_and_historical_snapshots_share_complete_shape() {
    let model = model();
    let task_id = id("tsk_000000000000000000000001");
    let current = model.snapshot(&task_id, None).unwrap();
    let historical = model
        .snapshot(&task_id, Some(&id("req_000000000000000000000001")))
        .unwrap();

    assert_eq!(2, current.request_history.len());
    assert_eq!(current.request_history, historical.request_history);
    assert_eq!(
        "req_000000000000000000000002",
        current.selected_request.unwrap().request_id.as_str()
    );
    assert_eq!(
        "req_000000000000000000000001",
        historical.selected_request.unwrap().request_id.as_str()
    );
    assert!(historical.exact_request.is_some());
    assert_eq!(
        Some(10),
        historical.usage.this_request.unwrap().input_tokens
    );
    assert_eq!(ContextHealthLevel::Healthy, current.health.level);
}

#[test]
fn catalog_search_and_missing_request_are_typed() {
    let model = model();
    let catalog = model
        .catalog(
            &id("wsp_000000000000000000000001"),
            CatalogQuery {
                search: Some("balanced".to_owned()),
            },
        )
        .unwrap();
    assert_eq!(1, catalog.tiers.len());
    assert!(catalog.skills.is_empty());
    assert!(matches!(
        model.snapshot(
            &id("tsk_000000000000000000000001"),
            Some(&id("req_000000000000000000000003")),
        ),
        Err(ContextReadError::RequestNotFound)
    ));
    assert!(matches!(
        model.catalog(
            &id("wsp_000000000000000000000001"),
            CatalogQuery {
                search: Some("x".repeat(4_097)),
            },
        ),
        Err(ContextReadError::InvalidRequest)
    ));
}

#[test]
fn agent_thread_rejects_thread_facts_for_another_task() {
    let mut facts = facts();
    facts.agent_threads[0].task_id = id("tsk_000000000000000000000099");
    let model = ContextReadModel::new(
        Arc::new(StaticSource { facts }),
        Arc::new(|_: &WorkspaceId| Ok(catalog())),
        Arc::new(StableHashes),
    );

    assert!(matches!(
        model.agent_thread(
            &id("tsk_000000000000000000000001"),
            &id("con_000000000000000000000002"),
        ),
        Err(ContextReadError::LedgerCorrupt)
    ));
}

#[test]
fn inaccessible_observation_has_distinct_warning() {
    let mut facts = facts();
    let observation = ObservationBuilder::from_tool(
        request_scope(2),
        "call-read".to_owned(),
        ToolObservation {
            summary: "read src/lib.rs".to_owned(),
            truncated: false,
            summarized: false,
            data: ToolObservationData::FileRead {
                path: "src/lib.rs".to_owned(),
                observed_hash: Some("sha256:observed".to_owned()),
                start_line: 1,
                line_count: 1,
            },
        },
    )
    .unwrap();
    facts
        .events
        .push(TaskEvent::ObservationRecorded(observation));
    let model = ContextReadModel::new(
        Arc::new(StaticSource { facts }),
        Arc::new(|_: &WorkspaceId| Ok(catalog())),
        Arc::new(InaccessibleHashes),
    );

    let snapshot = model
        .snapshot(&id("tsk_000000000000000000000001"), None)
        .unwrap();
    assert_eq!(0, snapshot.health.source_drift_count);
    assert!(snapshot
        .warnings
        .iter()
        .any(|warning| warning.code == api::ContextWarningCode::SourceInaccessible));
    assert!(!snapshot
        .warnings
        .iter()
        .any(|warning| warning.code == api::ContextWarningCode::SourceDrift));
}

#[test]
fn delegated_thread_keeps_whole_messages_under_both_bounds() {
    let mut facts = facts();
    facts.agent_threads[0].messages = (0..501)
        .map(|index| MessageProjection {
            message_id: format!("msg-{index}"),
            role: MessageRole::Agent,
            text: "message".to_owned(),
            finalized: true,
            request_ids: Vec::new(),
            file_references: Vec::new(),
            order_key: kuku::event::Cursor::try_new(index + 1).unwrap(),
        })
        .collect();
    let model = ContextReadModel::new(
        Arc::new(StaticSource { facts }),
        Arc::new(|_: &WorkspaceId| Ok(catalog())),
        Arc::new(StableHashes),
    );

    let thread = model
        .agent_thread(
            &id("tsk_000000000000000000000001"),
            &id("con_000000000000000000000002"),
        )
        .unwrap();
    assert_eq!(500, thread.messages.len());
    assert!(thread.messages_truncated_before);
    assert_eq!("msg-1", thread.messages[0].message_id);
    assert!(serde_json::to_vec(&thread).unwrap().len() <= agent_thread::MAX_AGENT_THREAD_BYTES);
}

#[tokio::test]
async fn feature_context_routes_are_typed_and_no_store() {
    let model = Arc::new(model());
    let app = route_context::router::<()>(model.clone())
        .merge(route_catalog::router::<()>(model.clone()))
        .merge(route_agents::router::<()>(model));

    let response = app
        .clone()
        .oneshot(
            Request::get("/tasks/tsk_000000000000000000000001/context")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(StatusCode::OK, response.status());
    assert_eq!(
        "no-store",
        response.headers()[axum::http::header::CACHE_CONTROL]
    );
    let snapshot: api::ContextSnapshot = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(2, snapshot.request_history.len());

    let malformed = app
        .clone()
        .oneshot(
            Request::get("/tasks/not-a-task/context")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(StatusCode::BAD_REQUEST, malformed.status());
    assert_eq!(
        "no-store",
        malformed.headers()[axum::http::header::CACHE_CONTROL]
    );
    let error: api::ApiError = serde_json::from_slice(
        &axum::body::to_bytes(malformed.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(api::ApiErrorCode::InvalidRequest, error.code);

    let method_rejected = app
        .oneshot(
            Request::post("/tasks/tsk_000000000000000000000001/context")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(StatusCode::METHOD_NOT_ALLOWED, method_rejected.status());
    assert_eq!(
        "no-store",
        method_rejected.headers()[axum::http::header::CACHE_CONTROL]
    );
}

#[test]
fn frozen_context_fixtures_are_canonical_and_public() {
    let current = include_str!("fixtures/context/v1/current.json");
    let historical = include_str!("fixtures/context/v1/historical.json");
    let thread = include_str!("fixtures/context/v1/agent_thread.json");
    let current_value: serde_json::Value = serde_json::from_str(current).unwrap();
    let historical_value: serde_json::Value = serde_json::from_str(historical).unwrap();
    let _: api::ContextSnapshot = serde_json::from_str(current).unwrap();
    let _: api::ContextSnapshot = serde_json::from_str(historical).unwrap();
    let _: api::AgentThread = serde_json::from_str(thread).unwrap();
    assert_eq!(
        current_value
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        historical_value
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>()
    );
    for forbidden in [
        "raw_events",
        "absolute_path",
        "credential",
        "provider-secret",
    ] {
        assert!(!current.contains(forbidden));
        assert!(!historical.contains(forbidden));
        assert!(!thread.contains(forbidden));
    }

    let reduced = serde_json::to_value(
        model()
            .snapshot(&id("tsk_000000000000000000000001"), None)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(current_value, reduced);
}
