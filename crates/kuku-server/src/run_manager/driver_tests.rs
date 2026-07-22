use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use httpmock::prelude::*;
use httpmock::MockServer;
use kuku::conversation::address::ConversationAddress;
use kuku::event::{
    CommandReceipt, CommandResult, ConversationId, EventPayload, ExecutionScope, ObservationKind,
    ObservationRetention, RequestId, RequestScope, RunFact, RunId, RunState, TaskEvent, TaskId,
    TaskLedgerRecord, TaskRevision, TaskTransaction, TurnId, WorkspaceId,
};
use tempfile::tempdir;

use super::{
    completion_metrics, finished_activity, permission_metadata, read_file_observation,
    resolve_product_tier, started_activity, ActivityKindFact, ActivityStatusFact, DriverCommand,
    DriverStart, KukuDriverFactory, PendingDecision, RunDriverFactory,
};

fn run_id() -> RunId {
    RunId::parse("run_0123456789abcdef01234567").unwrap()
}

#[test]
fn delegated_activity_keeps_typed_identity_and_no_detail() {
    let conversation_id = ConversationId::parse("con_0123456789abcdef01234567").unwrap();
    let activity = started_activity(
        &run_id(),
        "tool_1".to_owned(),
        "delegate".to_owned(),
        "summary must not leak into detail".to_owned(),
        kuku::ToolKind::Agent {
            conversation_id: conversation_id.clone(),
            agent: "reviewer".to_owned(),
            tier: "strong".to_owned(),
        },
    );

    assert_eq!(activity.kind, ActivityKindFact::DelegatedAgent);
    assert_eq!(activity.conversation_id, Some(conversation_id));
    assert_eq!(activity.agent.as_deref(), Some("reviewer"));
    assert_eq!(activity.tier.as_deref(), Some("strong"));
    assert_eq!(activity.result_in_main, Some(false));
    assert_eq!(activity.detail, None);

    let activity = finished_activity(
        activity,
        "ok",
        "finished".to_owned(),
        Some("delegated result"),
    );
    assert_eq!(activity.status, ActivityStatusFact::Completed);
    assert_eq!(activity.result_in_main, Some(true));
    assert_eq!(activity.detail, None);
}

#[test]
fn ordinary_tool_keeps_detail_and_null_delegated_fields() {
    let activity = started_activity(
        &run_id(),
        "tool_2".to_owned(),
        "read_file".to_owned(),
        "reading".to_owned(),
        kuku::ToolKind::Simple,
    );
    assert_eq!(activity.kind, ActivityKindFact::Tool);
    assert_eq!(activity.detail.as_deref(), Some("reading"));
    assert_eq!(activity.conversation_id, None);
    assert_eq!(activity.agent, None);
    assert_eq!(activity.tier, None);
    assert_eq!(activity.result_in_main, None);

    let activity = finished_activity(activity, "error", "failed".to_owned(), None);
    assert_eq!(activity.status, ActivityStatusFact::Failed);
    assert_eq!(activity.detail.as_deref(), Some("failed"));
    assert_eq!(activity.result_in_main, None);
}

#[test]
fn permission_decision_preserves_optional_parent_tool() {
    let top_level = PendingDecision::new("request-top".to_owned(), None);
    let nested = PendingDecision::new("request-nested".to_owned(), Some("agent-tool".to_owned()));

    assert_eq!(top_level.request_id, "request-top");
    assert_eq!(top_level.parent_tool_id, None);
    assert_eq!(nested.request_id, "request-nested");
    assert_eq!(nested.parent_tool_id.as_deref(), Some("agent-tool"));
}

fn permission_request(id: &str) -> kuku::PermissionRequest {
    kuku::PermissionRequest {
        id: id.to_owned(),
        conversation: ConversationAddress::MAIN,
        turn: 1,
        tool_call_id: "child-tool".to_owned(),
        tool: "write_file".to_owned(),
        risk: "write".to_owned(),
        summary: "Write a file".to_owned(),
        candidate: "src/lib.rs".to_owned(),
        source: "policy".to_owned(),
    }
}

#[test]
fn permission_metadata_keeps_nested_tool_parent_only_for_tool_output() {
    let top = kuku::UiEvent::PermissionRequested {
        request: permission_request("top"),
    };
    let nested = kuku::UiEvent::ToolOutput {
        id: "parent-agent".to_owned(),
        event: kuku::ToolEvent::PermissionRequested {
            request: permission_request("nested"),
        },
    };

    let (top_request, top_parent) = permission_metadata(&top).unwrap();
    let (nested_request, nested_parent) = permission_metadata(&nested).unwrap();
    assert_eq!(top_request.id, "top");
    assert_eq!(top_parent, None);
    assert_eq!(nested_request.id, "nested");
    assert_eq!(nested_parent, Some("parent-agent"));
}

struct NoWorkspaceUsage;

impl crate::platform::WorkspaceUsagePort for NoWorkspaceUsage {
    fn has_durable_tasks<'a>(
        &'a self,
        _: &'a WorkspaceId,
    ) -> Pin<Box<dyn Future<Output = Result<bool, crate::api::ApiError>> + Send + 'a>> {
        Box::pin(async { Ok(false) })
    }
}

fn test_config() -> kuku::config::Config {
    use std::collections::BTreeMap;

    use kuku::config::{
        Config, DiscoveryConfig, HandoffConfig, LogsConfig, PluginConfig, ProviderConfig,
        ProviderFormat, SecretString, StoredCredential, ThinkLevel, TierConfig, UpdateConfig,
    };

    Config {
        tiers: BTreeMap::from([(
            "balanced".to_owned(),
            TierConfig {
                provider: "anthropic".to_owned(),
                model: "test-model".to_owned(),
                think: ThinkLevel::Off,
                context_window: 4096,
                max_output_tokens: 1024,
                purpose: "balanced".to_owned(),
            },
        )]),
        providers: BTreeMap::from([(
            "anthropic".to_owned(),
            ProviderConfig {
                format: ProviderFormat::Anthropic,
                base_url: "http://127.0.0.1:9".to_owned(),
                credential: StoredCredential::DirectValue(SecretString::new("test-key")),
            },
        )]),
        default_tier: "balanced".to_owned(),
        discovery: DiscoveryConfig::default(),
        handoff: HandoffConfig::default(),
        logs: LogsConfig::default(),
        plugin: PluginConfig::default(),
        update: UpdateConfig::default(),
    }
}

fn task_store(path: &std::path::Path, scope: &ExecutionScope) -> kuku::event::EventStore {
    let mut store = kuku::event::EventStore::open(path).unwrap();
    let receipt = CommandReceipt::new(
        "create",
        "digest",
        CommandResult::TaskCreated {
            task_id: scope.task_id.clone(),
        },
    )
    .unwrap();
    let run = RunFact {
        run_id: scope.run_id.clone(),
        task_id: scope.task_id.clone(),
        state: RunState::Queued,
        started_at: "2026-07-20T00:00:00Z".to_owned(),
        finished_at: None,
        summary: None,
        warnings: Vec::new(),
        checks: None,
        metrics: None,
        workspace_changes: None,
    };
    let transaction = TaskTransaction::try_new(
        TaskRevision::try_new(0).unwrap(),
        receipt,
        vec![
            TaskEvent::TaskCreated {
                task_id: scope.task_id.clone(),
                workspace_id: scope.workspace_id.clone(),
                title: "Task".to_owned(),
                created_at: "2026-07-20T00:00:00Z".to_owned(),
            },
            TaskEvent::RunQueued { run },
        ],
    )
    .unwrap();
    store
        .append_synced(EventPayload::TaskLedger(TaskLedgerRecord::Control(
            transaction,
        )))
        .unwrap();
    store
}

async fn factory_fixture_with_config(
    tier_id: &str,
    config: kuku::config::Config,
) -> (
    KukuDriverFactory,
    DriverStart,
    tempfile::TempDir,
    tempfile::TempDir,
) {
    let home = tempdir().unwrap();
    let allowed = tempdir().unwrap();
    std::fs::create_dir(allowed.path().join("project")).unwrap();
    let roots = crate::platform::RegistrationRootRegistry::from_server_config(
        home.path(),
        vec![crate::platform::RegistrationRootSpec {
            label: "Projects".to_owned(),
            path: allowed.path().to_owned(),
        }],
    )
    .unwrap();
    let registry = crate::platform::WorkspaceRegistry::open(
        home.path(),
        roots,
        Arc::new(NoWorkspaceUsage),
        crate::platform::ServerRevisionCoordinator::open(home.path()),
    )
    .unwrap();
    let root_id = registry.registration_roots().list()[0].root_id.clone();
    let workspace = registry
        .register(crate::api::RegisterWorkspaceRequest {
            root_id,
            relative_path: "project".to_owned(),
            label: "Project".to_owned(),
            expected_revision: registry.revision().await.unwrap(),
        })
        .await
        .unwrap();
    let scope = ExecutionScope {
        workspace_id: workspace.workspace_id.clone(),
        task_id: TaskId::parse("tsk_0123456789abcdef01234567").unwrap(),
        run_id: run_id(),
        turn_id: TurnId::parse("trn_0123456789abcdef01234567").unwrap(),
        conversation_id: ConversationId::for_task_address(
            &TaskId::parse("tsk_0123456789abcdef01234567").unwrap(),
            "main",
        )
        .unwrap(),
        turn_index: 1,
    };
    let store = task_store(&home.path().join("task/events.jsonl"), &scope);
    let start = DriverStart {
        task_id: scope.task_id.clone(),
        run_id: scope.run_id.clone(),
        workspace_id: scope.workspace_id.clone(),
        prompt: "inspect".to_owned(),
        tier_id: tier_id.to_owned(),
        selected_skills: Vec::new(),
        agent_message_id: "msg_agent".to_owned(),
        execution_scope: scope,
        event_store: store,
    };
    (
        KukuDriverFactory::new(registry, Arc::new(config)),
        start,
        home,
        allowed,
    )
}

pub(super) async fn factory_fixture(
    tier_id: &str,
) -> (
    KukuDriverFactory,
    DriverStart,
    tempfile::TempDir,
    tempfile::TempDir,
) {
    factory_fixture_with_config(tier_id, test_config()).await
}

#[test]
fn completion_metrics_omit_unavailable_usage_without_inventing_zero() {
    let usage = kuku::ProviderUsage {
        input_tokens: Some(8),
        output_tokens: None,
        cache_read_input_tokens: Some(3),
        cache_creation_input_tokens: None,
    };

    let metrics = completion_metrics(Some(&usage), 2, 125);

    assert_eq!(
        metrics
            .iter()
            .map(|metric| metric.name.as_str())
            .collect::<Vec<_>>(),
        vec![
            "input_tokens",
            "cache_read_input_tokens",
            "model_request_count",
            "thinking_duration_ms",
        ]
    );
}

#[tokio::test]
async fn real_driver_maps_completion_usage_and_timing_to_metrics() {
    let provider = MockServer::start_async().await;
    provider.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .body(kuku::test_support::anthropic_sse_response(
                serde_json::json!({
                    "id": "msg_metrics",
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "text", "text": "Measured."}],
                    "stop_reason": "end_turn",
                    "usage": {
                        "input_tokens": 8,
                        "output_tokens": 2
                    }
                }),
            ));
    });
    let mut config = test_config();
    config.providers.get_mut("anthropic").unwrap().base_url = provider.base_url();
    let (factory, start, _home, _allowed) =
        factory_fixture_with_config("tier:balanced", config).await;
    let mut handle = factory.start(start).await.unwrap();

    let completed = loop {
        match tokio::time::timeout(Duration::from_secs(5), handle.events.recv())
            .await
            .expect("driver completion event")
            .expect("driver event stream")
        {
            super::DriverEvent::Completed(result) => break result,
            super::DriverEvent::Failed(failure) => panic!("driver failed: {}", failure.summary),
            _ => {}
        }
    };

    let metrics = completed.metrics.expect("completion metrics");
    let metric = |name: &str| {
        metrics
            .iter()
            .find(|metric| metric.name == name)
            .expect("named completion metric")
    };
    assert_eq!(metric("input_tokens").value.get(), 8.0);
    assert_eq!(metric("input_tokens").unit.as_deref(), Some("tokens"));
    assert_eq!(metric("output_tokens").value.get(), 2.0);
    assert_eq!(metric("cache_read_input_tokens").value.get(), 0.0);
    assert_eq!(metric("cache_creation_input_tokens").value.get(), 0.0);
    assert_eq!(metric("model_request_count").value.get(), 1.0);
    assert_eq!(
        metric("model_request_count").unit.as_deref(),
        Some("requests")
    );
    assert_eq!(metric("thinking_duration_ms").unit.as_deref(), Some("ms"));
}

#[tokio::test]
async fn real_driver_waits_for_permission_decision_before_polling_again() {
    let provider = MockServer::start_async().await;
    provider.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .body_contains("permission gate denied this tool call");
        then.status(200)
            .body(kuku::test_support::anthropic_sse_response(
                serde_json::json!({
                    "id": "msg_final",
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "text", "text": "Denied."}],
                    "stop_reason": "end_turn",
                    "usage": {"input_tokens": 8, "output_tokens": 2}
                }),
            ));
    });
    provider.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .body(kuku::test_support::anthropic_sse_response(
                serde_json::json!({
                    "id": "msg_permission",
                    "type": "message",
                    "role": "assistant",
                    "content": [{
                        "type": "tool_use",
                        "id": "toolu_command",
                        "name": "run_command",
                        "input": {"command": "printf blocked", "brief": "print marker"}
                    }],
                    "stop_reason": "tool_use",
                    "usage": {"input_tokens": 5, "output_tokens": 4}
                }),
            ));
    });
    let mut config = test_config();
    config.providers.get_mut("anthropic").unwrap().base_url = provider.base_url();
    let (factory, start, _home, _allowed) =
        factory_fixture_with_config("tier:balanced", config).await;
    let mut handle = factory.start(start).await.unwrap();

    let interaction = loop {
        match tokio::time::timeout(Duration::from_secs(5), handle.events.recv())
            .await
            .expect("driver event")
            .expect("driver event stream")
        {
            super::DriverEvent::InteractionOpened(interaction) => break interaction,
            super::DriverEvent::Failed(failure) => panic!("driver failed: {}", failure.summary),
            _ => {}
        }
    };
    let unexpected = tokio::time::timeout(Duration::from_millis(100), async {
        loop {
            match handle.events.recv().await {
                Some(super::DriverEvent::InteractionOpened(_)) => break "duplicate interaction",
                Some(super::DriverEvent::Failed(_)) => break "driver failure",
                Some(_) => {}
                None => break "closed event stream",
            }
        }
    })
    .await;
    assert!(unexpected.is_err(), "{unexpected:?}");

    handle
        .commands
        .send(DriverCommand::Resolve {
            interaction_id: interaction.interaction_id.clone(),
            choice_id: "deny".to_owned(),
        })
        .await
        .unwrap();
    let mut closed = false;
    loop {
        match tokio::time::timeout(Duration::from_secs(5), handle.events.recv())
            .await
            .expect("driver completion event")
            .expect("driver event stream")
        {
            super::DriverEvent::InteractionClosed(interaction_id) => {
                assert_eq!(interaction_id, interaction.interaction_id);
                closed = true;
            }
            super::DriverEvent::Completed(_) => break,
            super::DriverEvent::InteractionOpened(_) => panic!("duplicate interaction"),
            super::DriverEvent::Failed(failure) => panic!("driver failed: {}", failure.summary),
            _ => {}
        }
    }
    assert!(closed);
}

#[tokio::test]
async fn real_read_file_tool_emits_canonical_observation_activity() {
    let provider = MockServer::start_async().await;
    provider.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .body_contains("tool_result");
        then.status(200)
            .body(kuku::test_support::anthropic_sse_response(
                serde_json::json!({
                    "id": "msg_final",
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "text", "text": "Inspected."}],
                    "stop_reason": "end_turn",
                    "usage": {"input_tokens": 8, "output_tokens": 2}
                }),
            ));
    });
    provider.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .body(kuku::test_support::anthropic_sse_response(
                serde_json::json!({
                    "id": "msg_read",
                    "type": "message",
                    "role": "assistant",
                    "content": [{
                        "type": "tool_use",
                        "id": "toolu_read_notes",
                        "name": "read_file",
                        "input": {"path": "notes.txt"}
                    }],
                    "stop_reason": "tool_use",
                    "usage": {"input_tokens": 5, "output_tokens": 4}
                }),
            ));
    });
    let mut config = test_config();
    config.providers.get_mut("anthropic").unwrap().base_url = provider.base_url();
    let (factory, start, _home, allowed) =
        factory_fixture_with_config("tier:balanced", config).await;
    std::fs::write(
        allowed.path().join("project/notes.txt"),
        "first line\nsecond line\n",
    )
    .unwrap();
    let event_store = start.event_store.clone();
    let mut handle = factory.start(start).await.unwrap();
    let mut observation = None;
    loop {
        match tokio::time::timeout(Duration::from_secs(5), handle.events.recv())
            .await
            .expect("driver event")
            .expect("driver event stream")
        {
            super::DriverEvent::Activity(events) => {
                observation = observation.or_else(|| {
                    events.into_iter().find_map(|event| match event {
                        TaskEvent::ObservationRecorded(fact) => Some(fact),
                        _ => None,
                    })
                });
            }
            super::DriverEvent::Completed(_) => break,
            super::DriverEvent::Failed(failure) => panic!("driver failed: {}", failure.summary),
            _ => {}
        }
    }

    let observation = observation.expect("read_file observation activity");
    let call_scope = event_store
        .read_all()
        .unwrap()
        .into_iter()
        .find_map(|stored| match stored.payload {
            EventPayload::ToolCall {
                request,
                tool_call_id,
                ..
            } if tool_call_id == "toolu_read_notes" => Some(request),
            _ => None,
        })
        .expect("canonical tool call scope");
    assert_eq!(observation.scope, call_scope);
    assert_eq!(observation.tool_call_id, "toolu_read_notes");
    assert_eq!(observation.kind, ObservationKind::FileRead);
    assert_eq!(
        observation.relative_path.as_ref().map(|path| path.as_str()),
        Some("notes.txt")
    );
    assert!(observation.observed_hash.is_some());
    assert_eq!(observation.range.unwrap().start_line, 1);
    assert_eq!(observation.retention, ObservationRetention::Retained);
}

#[test]
fn observation_translation_rejects_non_file_failed_and_incomplete_results() {
    for (tool, status, structured) in [
        (
            "write_file",
            "ok",
            serde_json::json!({
                "kind": "file_content",
                "path": "notes.txt",
                "content_hash": "sha256:test",
                "start_line": 1,
                "line_count": 1
            }),
        ),
        (
            "read_file",
            "error",
            serde_json::json!({
                "kind": "file_content",
                "path": "notes.txt",
                "content_hash": "sha256:test",
                "start_line": 1,
                "line_count": 1
            }),
        ),
        (
            "read_file",
            "ok",
            serde_json::json!({
                "kind": "file_content",
                "content_hash": "sha256:test",
                "start_line": 1,
                "line_count": 1
            }),
        ),
    ] {
        let dir = tempdir().unwrap();
        let mut store = kuku::event::EventStore::open(dir.path().join("events.jsonl")).unwrap();
        let execution = ExecutionScope {
            workspace_id: WorkspaceId::parse("wsp_0123456789abcdef01234567").unwrap(),
            task_id: TaskId::parse("tsk_0123456789abcdef01234567").unwrap(),
            run_id: run_id(),
            turn_id: TurnId::parse("trn_0123456789abcdef01234567").unwrap(),
            conversation_id: ConversationId::parse("con_0123456789abcdef01234567").unwrap(),
            turn_index: 1,
        };
        store
            .append(EventPayload::ToolCall {
                request: RequestScope {
                    execution: execution.clone(),
                    request_id: RequestId::parse("req_0123456789abcdef01234567").unwrap(),
                },
                turn: 1,
                ts: "2026-07-21T00:00:00Z".to_owned(),
                conversation: None,
                tool_call_id: "toolu_candidate".to_owned(),
                index: 0,
                tool: tool.to_owned(),
                args: serde_json::json!({"path": "notes.txt"}),
            })
            .unwrap();
        store
            .append(EventPayload::ToolResult {
                execution: execution.clone(),
                turn: 1,
                ts: "2026-07-21T00:00:01Z".to_owned(),
                conversation: None,
                tool_call_id: "toolu_candidate".to_owned(),
                status: status.to_owned(),
                summary: "candidate result".to_owned(),
                model_content: String::new(),
                truncated: false,
                files_read: Vec::new(),
                files_changed: Vec::new(),
                commands_run: Vec::new(),
                memory_changed: None,
                structured: Some(structured),
            })
            .unwrap();

        assert!(
            read_file_observation(&store, &execution, "toolu_candidate")
                .unwrap()
                .is_none(),
            "tool={tool} status={status}"
        );
    }
}

#[tokio::test]
async fn real_factory_maps_default_product_tier_to_sdk_tier() {
    let (factory, start, _home, _allowed) = factory_fixture("tier:default").await;
    let handle = factory.start(start).await.unwrap();
    handle.commands.send(DriverCommand::Stop).await.unwrap();
}

#[test]
fn product_tier_resolution_maps_default_and_named_catalog_ids() {
    let config = test_config();
    assert_eq!(
        resolve_product_tier(&config, "tier:default").unwrap(),
        "balanced"
    );
    assert_eq!(
        resolve_product_tier(&config, "tier:balanced").unwrap(),
        "balanced"
    );
}

#[tokio::test]
async fn real_factory_rejects_noncanonical_or_unknown_product_tiers() {
    for tier_id in ["balanced", "tier:", "tier:missing"] {
        let (factory, start, _home, _allowed) = factory_fixture(tier_id).await;
        assert!(matches!(
            factory.start(start).await,
            Err(super::DomainError::InvalidRequest)
        ));
    }
}
