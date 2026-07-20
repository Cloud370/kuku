use std::sync::Arc;

use kuku::context::{
    CanonicalMessage, ContextAssembly, DurableRequestEvidenceRecorder, EventStoreContextFactSink,
    MessageBlock, RequestEvidenceRecorder, RequestSnapshotBuilder, SnapshotBuildError,
    SnapshotInput, ToolResult, ToolSchema, MAX_REQUEST_SNAPSHOT_BYTES,
};
use kuku::event::{
    CapabilityFact, CapabilityKind, CapabilityState, ContextBreakdown, ConversationContextFact,
    ConversationId, EventPayload, EventStore, ExactRequestParameters, ExecutionScope, MessageRole,
    ProviderFact, RequestCause, RequestId, RequestScope, RequestStarted, RevisionToken, RunId,
    TaskEvent, TaskId, TaskLedgerRecord, ThinkingConfig, TurnId, WorkspaceId,
};

fn request_scope(_seed: &str) -> RequestScope {
    RequestScope {
        execution: ExecutionScope {
            workspace_id: WorkspaceId::try_new().unwrap(),
            task_id: TaskId::try_new().unwrap(),
            run_id: RunId::try_new().unwrap(),
            turn_id: TurnId::try_new().unwrap(),
            conversation_id: ConversationId::try_new().unwrap(),
            turn_index: 1,
        },
        request_id: RequestId::try_new().unwrap(),
    }
}

fn assembly(system_prompt: String, schema: serde_json::Value) -> ContextAssembly {
    ContextAssembly {
        system_prompt,
        prelude_messages: vec![CanonicalMessage::user_text("project policy")],
        history: vec![CanonicalMessage::assistant(vec![MessageBlock::Text(
            "prior answer".to_string(),
        )])],
        tools: vec![
            ToolSchema {
                name: "read_file".to_string(),
                description: "read a contained file".to_string(),
                input_schema: schema,
            },
            ToolSchema {
                name: "run_command".to_string(),
                description: "run a command".to_string(),
                input_schema: serde_json::json!({"type": "object"}),
            },
        ],
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
        capabilities: vec![CapabilityFact {
            kind: CapabilityKind::FileRead,
            state: CapabilityState::Available,
        }],
        token_estimate: Some(42),
    }
}

fn parameters() -> ExactRequestParameters {
    ExactRequestParameters {
        model: "model-a".to_string(),
        max_output_tokens: Some(4_096),
        temperature: None,
        stream: true,
        thinking: ThinkingConfig::Disabled,
    }
}

fn build_snapshot(
    seed: &str,
    assembly: &ContextAssembly,
    current_input: &CanonicalMessage,
) -> Result<kuku::event::RequestSnapshot, SnapshotBuildError> {
    RequestSnapshotBuilder::build(SnapshotInput {
        scope: request_scope(seed),
        cause: RequestCause::UserSubmission,
        provider: ProviderFact::Anthropic,
        tier_id: "tier:default",
        assembly,
        current_input,
        allowlisted_provider_parameters: parameters(),
        breakdown: breakdown(),
        catalog_revision: RevisionToken::parse(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .unwrap(),
    })
}

#[test]
fn snapshot_preserves_order_but_excludes_transport_credentials() {
    let assembly = assembly(
        "system-sensitive".to_string(),
        serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
    );
    let current_input = CanonicalMessage::user(vec![
        MessageBlock::Text("workspace-sensitive".to_string()),
        MessageBlock::ToolResult(ToolResult {
            tool_call_id: "call-1".to_string(),
            status: "ok".to_string(),
            summary: "not provider visible".to_string(),
            model_content: "tool result".to_string(),
            structured: Some(serde_json::json!({"z": 1, "a": 2})),
            truncated: false,
        }),
    ]);

    let snapshot = build_snapshot("exact order", &assembly, &current_input).unwrap();

    assert_eq!(MessageRole::System, snapshot.exact.messages[0].role);
    assert_eq!(MessageRole::User, snapshot.exact.messages[1].role);
    assert_eq!(MessageRole::Assistant, snapshot.exact.messages[2].role);
    assert_eq!(MessageRole::User, snapshot.exact.messages[3].role);
    assert_eq!("read_file", snapshot.exact.tools[0].name);
    assert_eq!("run_command", snapshot.exact.tools[1].name);
    let wire = serde_json::to_value(&snapshot).unwrap();
    assert!(wire.pointer("/exact/authorization").is_none());
    assert!(!wire.to_string().contains("provider-secret"));
    assert!(wire.to_string().contains("workspace-sensitive"));
    let debug = format!("{snapshot:?}");
    assert!(!debug.contains("workspace-sensitive"));
    assert!(debug.contains("<redacted>"));
}

#[test]
fn canonical_hash_sorts_map_keys_without_reordering_arrays() {
    let first = assembly(
        "system".to_string(),
        serde_json::from_str(
            r#"{"type":"object","properties":{"z":{"type":"string"},"a":{"type":"number"}}}"#,
        )
        .unwrap(),
    );
    let second = assembly(
        "system".to_string(),
        serde_json::from_str(
            r#"{"properties":{"a":{"type":"number"},"z":{"type":"string"}},"type":"object"}"#,
        )
        .unwrap(),
    );
    let current_input = CanonicalMessage::user_text("same input");

    let first = build_snapshot("canonical hash", &first, &current_input).unwrap();
    let second = build_snapshot("canonical hash", &second, &current_input).unwrap();

    assert_eq!(first.exact_payload_hash, second.exact_payload_hash);
    assert!(first.exact_payload_hash.starts_with("sha256:"));

    let reversed_input = CanonicalMessage::user(vec![
        MessageBlock::Text("second".to_string()),
        MessageBlock::Text("first".to_string()),
    ]);
    let reversed = build_snapshot(
        "canonical hash",
        &assembly("system".to_string(), serde_json::json!({"type": "object"})),
        &reversed_input,
    )
    .unwrap();
    assert_ne!(first.exact_payload_hash, reversed.exact_payload_hash);
}

#[test]
fn oversized_snapshot_stops_before_persistence() {
    let assembly = assembly(
        "x".repeat(MAX_REQUEST_SNAPSHOT_BYTES),
        serde_json::json!({"type": "object"}),
    );
    let current_input = CanonicalMessage::user_text("small input");

    let error = build_snapshot("oversized", &assembly, &current_input).unwrap_err();

    assert!(matches!(
        error,
        SnapshotBuildError::TooLarge {
            limit: MAX_REQUEST_SNAPSHOT_BYTES,
            ..
        }
    ));
    assert!(!format!("{error:?}").contains(&"x".repeat(64)));
}

#[test]
fn recorder_persists_snapshot_then_started_in_one_durable_batch() {
    let directory = tempfile::tempdir().unwrap();
    let events_path = directory.path().join("events.jsonl");
    let store = EventStore::open(&events_path).unwrap();
    let recorder = DurableRequestEvidenceRecorder::new(Arc::new(
        EventStoreContextFactSink::new(store).unwrap(),
    ));
    let assembly = assembly("system".to_string(), serde_json::json!({"type": "object"}));
    let current_input = CanonicalMessage::user_text("input");
    let snapshot = build_snapshot("durable evidence", &assembly, &current_input).unwrap();
    let started = RequestStarted {
        scope: snapshot.scope.clone(),
        cause: snapshot.cause.clone(),
        provider: snapshot.provider,
        model: snapshot.exact.parameters.model.clone(),
        started_at: "2026-07-21T00:00:00Z".to_string(),
    };

    let cursor = recorder
        .record_before_provider(snapshot.clone(), started.clone())
        .unwrap();

    assert_eq!(1, cursor.get());
    let stored = EventStore::replay(&events_path).unwrap();
    assert_eq!(1, stored.len());
    let EventPayload::TaskLedger(TaskLedgerRecord::Activity(batch)) = &stored[0].payload else {
        panic!("expected task activity batch");
    };
    assert_eq!(
        [
            TaskEvent::RequestSnapshot(snapshot),
            TaskEvent::RequestStarted(started)
        ],
        batch.events()
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        assert_eq!(
            0o600,
            std::fs::metadata(events_path).unwrap().permissions().mode() & 0o777
        );
    }
}
