use std::sync::Arc;

use kuku::context::{DurableRequestEvidenceRecorder, RequestEvidenceRecorder};
use kuku::event::{
    EventPayload, EventStore, ExactRequest, ExactRequestParameters, ProviderFact, RequestCause,
    RequestScope, RequestSnapshot, RequestStarted, RevisionToken, TaskEvent, TaskLedgerRecord,
    ThinkingConfig,
};

fn snapshot(scope: RequestScope) -> RequestSnapshot {
    RequestSnapshot {
        scope,
        cause: RequestCause::UserSubmission,
        provider: ProviderFact::Anthropic,
        tier_id: "tier:default".to_owned(),
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
        context: serde_json::from_value(serde_json::json!({
            "skills": [], "instructions": [], "memory": [],
            "conversation": {"retained_turns": 0, "handoff_boundaries": 0, "history_summarized": false, "delegated_results": []},
            "observations": [], "delegated_results": [], "capabilities": [], "token_estimate": null
        })).unwrap(),
        catalog_revision: RevisionToken::parse("a".repeat(64)).unwrap(),
        exact_payload_hash: "sha256:exact".to_owned(),
    }
}

#[test]
fn request_snapshot_precedes_request_start_in_durable_round_trip() {
    let directory = tempfile::tempdir().unwrap();
    let events_path = directory.path().join("events.jsonl");
    let store = EventStore::open(&events_path).unwrap();
    let scope: RequestScope = serde_json::from_value(serde_json::json!({
        "execution": {
            "workspace_id": "wsp_000000000000000000000001",
            "task_id": "tsk_000000000000000000000001",
            "run_id": "run_000000000000000000000001",
            "turn_id": "trn_000000000000000000000001",
            "conversation_id": "con_000000000000000000000001",
            "turn_index": 1
        },
        "request_id": "req_000000000000000000000001"
    }))
    .unwrap();
    let recorder = DurableRequestEvidenceRecorder::new(Arc::new(
        kuku::context::EventStoreContextFactSink::new(store.clone()).unwrap(),
    ));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        assert_eq!(
            0o600,
            std::fs::metadata(&events_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777
        );
    }
    recorder
        .record_before_provider(
            snapshot(scope.clone()),
            RequestStarted {
                scope,
                cause: RequestCause::UserSubmission,
                provider: ProviderFact::Anthropic,
                model: "fixture-model".to_owned(),
                started_at: "2026-07-21T00:00:00Z".to_owned(),
            },
        )
        .unwrap();
    let events = EventStore::replay(&events_path).unwrap();
    let EventPayload::TaskLedger(TaskLedgerRecord::Activity(batch)) = &events[0].payload else {
        panic!("context facts must be an activity batch")
    };
    assert!(matches!(batch.events()[0], TaskEvent::RequestSnapshot(_)));
    assert!(matches!(batch.events()[1], TaskEvent::RequestStarted(_)));
    assert!(matches!(events[0].payload, EventPayload::TaskLedger(_)));
}
