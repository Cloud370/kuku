use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use kuku::context::{
    CanonicalMessage, ContextAssembly, DurableRequestEvidenceRecorder, EventStoreContextFactSink,
    RequestEvidenceRecorder, RequestIdAccumulator, RequestSnapshotBuilder, SnapshotInput,
};
use kuku::event::{
    ContextBreakdown, ConversationContextFact, ConversationId, EventPayload, EventStore,
    ExactRequestParameters, ExecutionScope, MessageFact, MessageRoleFact, ProviderFact,
    RequestCause, RequestId, RequestScope, RequestSnapshot, RequestStarted, RevisionToken, RunId,
    TaskEvent, TaskId, TaskLedgerRecord, ThinkingConfig, TurnId, WorkspaceId,
};

fn execution_scope() -> ExecutionScope {
    ExecutionScope {
        workspace_id: WorkspaceId::parse("wsp_000000000000000000000001").unwrap(),
        task_id: TaskId::parse("tsk_000000000000000000000001").unwrap(),
        run_id: RunId::parse("run_000000000000000000000001").unwrap(),
        turn_id: TurnId::parse("trn_000000000000000000000001").unwrap(),
        conversation_id: ConversationId::parse("con_000000000000000000000001").unwrap(),
        turn_index: 1,
    }
}

fn request_id(index: u8) -> RequestId {
    RequestId::parse(format!("req_{index:024x}")).unwrap()
}

fn request_scope(index: u8) -> RequestScope {
    RequestScope {
        execution: execution_scope(),
        request_id: request_id(index),
    }
}

fn assembly(system: String) -> ContextAssembly {
    ContextAssembly {
        system_prompt: system,
        prelude_messages: vec![CanonicalMessage::user_text("policy")],
        history: vec![CanonicalMessage::user_text("history")],
        tools: Vec::new(),
        prompt_asset_sources: Vec::new(),
        project_instruction_sources: Vec::new(),
        memory_sources: Vec::new(),
        runtime_context: None,
        handoff_summary: None,
    }
}

fn breakdown() -> ContextBreakdown {
    ContextBreakdown {
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
        token_estimate: None,
    }
}

fn snapshot(index: u8, cause: RequestCause, assembly: &ContextAssembly) -> RequestSnapshot {
    let mut final_assembly = assembly.clone();
    final_assembly
        .history
        .push(CanonicalMessage::user_text(format!("input {index}")));
    RequestSnapshotBuilder::build(SnapshotInput {
        scope: request_scope(index),
        cause,
        provider: ProviderFact::Anthropic,
        tier_id: "tier:default",
        assembly: &final_assembly,
        handoff_context_template: None,
        allowlisted_provider_parameters: ExactRequestParameters {
            model: "model-a".to_string(),
            max_output_tokens: Some(1_024),
            temperature: None,
            stream: true,
            thinking: ThinkingConfig::Disabled,
        },
        breakdown: breakdown(),
        catalog_revision: RevisionToken::parse(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .unwrap(),
    })
    .unwrap()
}

fn started(snapshot: &RequestSnapshot) -> RequestStarted {
    RequestStarted {
        scope: snapshot.scope.clone(),
        cause: snapshot.cause.clone(),
        provider: snapshot.provider,
        model: snapshot.exact.parameters.model.clone(),
        started_at: "2026-07-21T00:00:00Z".to_string(),
    }
}

#[test]
fn two_provider_calls_persist_two_exact_snapshots_with_parent_causality() {
    let directory = tempfile::tempdir().unwrap();
    let events_path = directory.path().join("events.jsonl");
    let store = EventStore::open(&events_path).unwrap();
    let recorder = DurableRequestEvidenceRecorder::new(Arc::new(
        EventStoreContextFactSink::new(store).unwrap(),
    ));
    let assembly = assembly("system".to_string());
    let first = snapshot(1, RequestCause::UserSubmission, &assembly);
    let second = snapshot(
        2,
        RequestCause::ToolContinuation {
            parent_request_id: first.scope.request_id.clone(),
        },
        &assembly,
    );

    recorder
        .record_before_provider(first.clone(), started(&first))
        .unwrap();
    recorder
        .record_before_provider(second.clone(), started(&second))
        .unwrap();

    let stored = EventStore::replay(events_path).unwrap();
    assert_eq!(2, stored.len());
    for event in &stored {
        let EventPayload::TaskLedger(TaskLedgerRecord::Activity(batch)) = &event.payload else {
            panic!("expected task activity batch");
        };
        assert!(matches!(
            batch.events(),
            [TaskEvent::RequestSnapshot(_), TaskEvent::RequestStarted(_)]
        ));
    }
    assert_ne!(first.scope.request_id, second.scope.request_id);
    assert!(!first.exact.messages.is_empty());
    assert!(!second.exact.messages.is_empty());
}

#[test]
fn one_agent_message_references_all_ordered_unique_requests_in_a_turn() {
    let mut request_ids = RequestIdAccumulator::default();
    request_ids.record(request_id(1));
    request_ids.record(request_id(1));
    request_ids.record(request_id(2));

    let message = MessageFact {
        message_id: "message-1".to_string(),
        task_id: execution_scope().task_id,
        run_id: Some(execution_scope().run_id),
        role: MessageRoleFact::Agent,
        text: "done".to_string(),
        finalized: true,
        request_ids: request_ids.into_vec(),
        file_references: Vec::new(),
    };

    assert_eq!(vec![request_id(1), request_id(2)], message.request_ids);
}

#[test]
fn persisted_snapshot_replay_does_not_follow_later_context_changes() {
    let directory = tempfile::tempdir().unwrap();
    let events_path = directory.path().join("events.jsonl");
    let store = EventStore::open(&events_path).unwrap();
    let recorder = DurableRequestEvidenceRecorder::new(Arc::new(
        EventStoreContextFactSink::new(store).unwrap(),
    ));
    let original = snapshot(
        1,
        RequestCause::UserSubmission,
        &assembly("original system".to_string()),
    );
    recorder
        .record_before_provider(original.clone(), started(&original))
        .unwrap();

    let changed = snapshot(
        1,
        RequestCause::UserSubmission,
        &assembly("changed system".to_string()),
    );
    assert_ne!(original.exact_payload_hash, changed.exact_payload_hash);

    let stored = EventStore::replay(events_path).unwrap();
    let EventPayload::TaskLedger(TaskLedgerRecord::Activity(batch)) = &stored[0].payload else {
        panic!("expected task activity batch");
    };
    let TaskEvent::RequestSnapshot(replayed) = &batch.events()[0] else {
        panic!("expected request snapshot");
    };
    assert_eq!(original.exact_payload_hash, replayed.exact_payload_hash);
    assert_eq!(original.exact, replayed.exact);
}

#[tokio::test]
async fn oversized_exact_request_never_reaches_provider_transport() {
    let provider_calls = AtomicUsize::new(0);
    let oversized = assembly("x".repeat(kuku::context::MAX_REQUEST_SNAPSHOT_BYTES));

    let result = RequestSnapshotBuilder::build(SnapshotInput {
        scope: request_scope(1),
        cause: RequestCause::UserSubmission,
        provider: ProviderFact::Anthropic,
        tier_id: "tier:default",
        assembly: &oversized,
        handoff_context_template: None,
        allowlisted_provider_parameters: ExactRequestParameters {
            model: "model-a".to_string(),
            max_output_tokens: None,
            temperature: None,
            stream: true,
            thinking: ThinkingConfig::Disabled,
        },
        breakdown: breakdown(),
        catalog_revision: RevisionToken::parse(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .unwrap(),
    });
    if result.is_ok() {
        provider_calls.fetch_add(1, Ordering::SeqCst);
    }

    assert!(result.is_err());
    assert_eq!(0, provider_calls.load(Ordering::SeqCst));
}
