pub mod context {
    pub use kuku::context::*;
}

pub mod event {
    pub use kuku::event::*;
}

#[path = "../src/context/provider_hook.rs"]
mod provider_hook;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use context::{
    CanonicalMessage, ContextAssembly, RequestEvidenceRecorder, SnapshotInput, ToolSchema,
};
use kuku::event::{
    ContextBreakdown, ConversationContextFact, ConversationId, ExactRequestParameters,
    ExecutionScope, ProviderFact, RequestCause, RequestId, RequestScope, RequestSnapshot,
    RequestStarted, RevisionToken, RunId, TaskId, ThinkingConfig, TurnId, WorkspaceId,
};

#[derive(Debug, Default)]
struct Recorder {
    calls: AtomicUsize,
    snapshot: Mutex<Option<RequestSnapshot>>,
}

impl RequestEvidenceRecorder for Recorder {
    fn record_before_provider(
        &self,
        snapshot: RequestSnapshot,
        _started: RequestStarted,
    ) -> Result<kuku::event::Cursor, context::ContextFactSinkError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.snapshot.lock().unwrap() = Some(snapshot);
        kuku::event::Cursor::try_new(1).map_err(|_| {
            context::ContextFactSinkError::InvalidCursor(
                kuku::event::StorageExhaustionError::OutOfRange {
                    kind: "cursor",
                    value: 1,
                },
            )
        })
    }
}

fn input() -> SnapshotInput<'static> {
    let assembly = Box::leak(Box::new(ContextAssembly {
        system_prompt: "system".to_owned(),
        prelude_messages: vec![CanonicalMessage::user_text("policy")],
        history: Vec::new(),
        tools: vec![ToolSchema {
            name: "read_file".to_owned(),
            description: "read".to_owned(),
            input_schema: serde_json::json!({"type": "object"}),
        }],
        prompt_asset_sources: Vec::new(),
        project_instruction_sources: Vec::new(),
        memory_sources: Vec::new(),
        runtime_context: None,
        handoff_summary: None,
    }));
    SnapshotInput {
        scope: RequestScope {
            execution: ExecutionScope {
                workspace_id: WorkspaceId::try_new().unwrap(),
                task_id: TaskId::try_new().unwrap(),
                run_id: RunId::try_new().unwrap(),
                turn_id: TurnId::try_new().unwrap(),
                conversation_id: ConversationId::try_new().unwrap(),
                turn_index: 1,
            },
            request_id: RequestId::try_new().unwrap(),
        },
        cause: RequestCause::UserSubmission,
        provider: ProviderFact::Anthropic,
        tier_id: "tier:default",
        assembly,
        handoff_context_template: None,
        allowlisted_provider_parameters: ExactRequestParameters {
            model: "model".to_owned(),
            max_output_tokens: Some(128),
            temperature: None,
            stream: true,
            thinking: ThinkingConfig::Disabled,
        },
        breakdown: ContextBreakdown {
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
        },
        catalog_revision: RevisionToken::parse("a".repeat(64)).unwrap(),
    }
}

fn started(scope: &RequestScope) -> RequestStarted {
    RequestStarted {
        scope: scope.clone(),
        cause: RequestCause::UserSubmission,
        provider: ProviderFact::Anthropic,
        model: "model".to_owned(),
        started_at: "2026-07-21T00:00:00Z".to_owned(),
    }
}

#[tokio::test]
async fn provider_hook_records_snapshot_before_polling_transport() {
    let recorder = Arc::new(Recorder::default());
    let snapshot_input = input();
    let started = started(&snapshot_input.scope);
    let polled = Arc::new(AtomicUsize::new(0));
    let polled_for_future = Arc::clone(&polled);
    let result = provider_hook::begin_provider_request(
        recorder.as_ref(),
        snapshot_input,
        started,
        async move {
            polled_for_future.fetch_add(1, Ordering::SeqCst);
            42_u32
        },
    )
    .await
    .unwrap();

    assert_eq!(42, result.1);
    assert_eq!(1, polled.load(Ordering::SeqCst));
    assert_eq!(1, recorder.calls.load(Ordering::SeqCst));
    assert!(recorder.snapshot.lock().unwrap().is_some());
}

#[tokio::test]
async fn oversized_snapshot_never_polls_transport_or_recorder() {
    let recorder = Arc::new(Recorder::default());
    let mut snapshot_input = input();
    snapshot_input.assembly = Box::leak(Box::new(ContextAssembly {
        system_prompt: "x".repeat(context::MAX_REQUEST_SNAPSHOT_BYTES),
        ..(*snapshot_input.assembly).clone()
    }));
    let scope = snapshot_input.scope.clone();
    let polled = Arc::new(AtomicUsize::new(0));
    let polled_for_future = Arc::clone(&polled);
    let result = provider_hook::begin_provider_request(
        recorder.as_ref(),
        snapshot_input,
        started(&scope),
        async move {
            polled_for_future.fetch_add(1, Ordering::SeqCst);
        },
    )
    .await;

    assert!(result.is_err());
    assert_eq!(0, polled.load(Ordering::SeqCst));
    assert_eq!(0, recorder.calls.load(Ordering::SeqCst));
}
