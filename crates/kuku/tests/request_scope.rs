mod common;

use common::{test_config, TestEnv};

use httpmock::prelude::*;
use kuku::event::{
    EventPayload, EventStore, ProviderFailureKind, RequestCause, TaskEvent, TaskLedgerRecord,
};
use kuku::{query, Provider};

const TOOL_USE: &[u8] = b"\"type\":\"tool_use\"";

fn is_initial_request(request: &HttpMockRequest) -> bool {
    request.body.as_ref().is_some_and(|body| {
        !body
            .windows(TOOL_USE.len())
            .any(|window| window == TOOL_USE)
    })
}

#[tokio::test(flavor = "current_thread")]
async fn provider_failure_has_one_started_and_one_failed_fact() {
    let env = TestEnv::new();
    let server = MockServer::start();
    let failure = server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(401)
            .header("request-id", "provider-request-failed")
            .body(r#"{"type":"error","error":{"type":"authentication_error","message":"denied"}}"#);
    });

    let error = query("fail once")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .session("s_request_failure")
        .run()
        .await
        .unwrap_err();

    failure.assert_hits(1);
    assert_eq!("provider_auth", error.code());
    let events = EventStore::replay(env.events_path("s_request_failure")).unwrap();
    let facts: Vec<&TaskEvent> = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::TaskLedger(TaskLedgerRecord::Activity(batch)) => Some(batch.events()),
            _ => None,
        })
        .flatten()
        .collect();
    let started: Vec<_> = facts
        .iter()
        .filter_map(|fact| match fact {
            TaskEvent::RequestStarted(value) => Some(value),
            _ => None,
        })
        .collect();
    let failed: Vec<_> = facts
        .iter()
        .filter_map(|fact| match fact {
            TaskEvent::RequestFailed(value) => Some(value),
            _ => None,
        })
        .collect();

    assert_eq!(1, started.len());
    assert_eq!(1, failed.len());
    assert_eq!(started[0].scope, failed[0].scope);
    assert_eq!(ProviderFailureKind::Authentication, failed[0].failure.kind);
    assert_eq!(
        Some("provider-request-failed"),
        failed[0].provider_request_id.as_deref()
    );
    assert!(failed[0].usage.is_none());
    assert!(facts
        .iter()
        .all(|fact| !matches!(fact, TaskEvent::RequestCompleted(_))));
}

fn anthropic_response(message: serde_json::Value) -> String {
    let id = message["id"].as_str().unwrap();
    let content = message["content"].as_array().unwrap();
    let stop_reason = message["stop_reason"].as_str().unwrap();
    let usage = &message["usage"];
    let mut response = format!(
        "event: message_start\ndata: {}\n\n",
        serde_json::json!({
            "type": "message_start",
            "message": {"id": id, "content": [], "usage": usage}
        })
    );
    for (index, block) in content.iter().enumerate() {
        match block["type"].as_str().unwrap() {
            "text" => {
                response.push_str(&format!(
                    "event: content_block_start\ndata: {}\n\n",
                    serde_json::json!({
                        "type": "content_block_start",
                        "index": index,
                        "content_block": {"type": "text", "text": ""}
                    })
                ));
                response.push_str(&format!(
                    "event: content_block_delta\ndata: {}\n\n",
                    serde_json::json!({
                        "type": "content_block_delta",
                        "index": index,
                        "delta": {"type": "text_delta", "text": block["text"]}
                    })
                ));
            }
            "tool_use" => {
                response.push_str(&format!(
                    "event: content_block_start\ndata: {}\n\n",
                    serde_json::json!({
                        "type": "content_block_start",
                        "index": index,
                        "content_block": {
                            "type": "tool_use",
                            "id": block["id"],
                            "name": block["name"],
                            "input": {}
                        }
                    })
                ));
                response.push_str(&format!(
                    "event: content_block_delta\ndata: {}\n\n",
                    serde_json::json!({
                        "type": "content_block_delta",
                        "index": index,
                        "delta": {
                            "type": "input_json_delta",
                            "partial_json": block["input"].to_string()
                        }
                    })
                ));
            }
            other => panic!("unsupported fixture block: {other}"),
        }
        response.push_str(&format!(
            "event: content_block_stop\ndata: {}\n\n",
            serde_json::json!({"type": "content_block_stop", "index": index})
        ));
    }
    response.push_str(&format!(
        "event: message_delta\ndata: {}\n\n",
        serde_json::json!({
            "type": "message_delta",
            "delta": {"stop_reason": stop_reason},
            "usage": {"output_tokens": usage["output_tokens"]}
        })
    ));
    response.push_str("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n");
    response
}

#[tokio::test(flavor = "current_thread")]
async fn every_provider_call_has_one_unique_lifecycle_in_the_same_run() {
    let env = TestEnv::new();
    let server = MockServer::start();
    std::fs::write(env.workspace.path().join("README.md"), "# Fixture\n").unwrap();
    let first = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(is_initial_request);
        then.status(200).body(anthropic_response(serde_json::json!({
            "id": "provider-request-one",
            "content": [{
                "type": "tool_use",
                "id": "toolu_find",
                "name": "find_files",
                "input": {"path": "."}
            }],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 5, "output_tokens": 6}
        })));
    });
    let second = server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200).body(anthropic_response(serde_json::json!({
            "id": "provider-request-two",
            "content": [{"type": "text", "text": "done"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 8}
        })));
    });

    let output = query("inspect files")
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(server.base_url())
        .api_key("test-key")
        .config(test_config())
        .run()
        .await
        .unwrap();

    first.assert_hits(1);
    second.assert_hits(1);
    let events = EventStore::replay(env.events_path(&output.session_id)).unwrap();
    let facts: Vec<&TaskEvent> = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::TaskLedger(TaskLedgerRecord::Activity(batch)) => Some(batch.events()),
            _ => None,
        })
        .flatten()
        .collect();
    let started: Vec<_> = facts
        .iter()
        .filter_map(|fact| match fact {
            TaskEvent::RequestStarted(value) => Some(value),
            _ => None,
        })
        .collect();
    let completed: Vec<_> = facts
        .iter()
        .filter_map(|fact| match fact {
            TaskEvent::RequestCompleted(value) => Some(value),
            _ => None,
        })
        .collect();
    let failed = facts
        .iter()
        .filter(|fact| matches!(fact, TaskEvent::RequestFailed(_)))
        .count();

    assert_eq!(2, started.len());
    assert_eq!(2, completed.len());
    assert_eq!(0, failed);
    assert_ne!(started[0].scope.request_id, started[1].scope.request_id);
    assert_eq!(started[0].scope.execution, started[1].scope.execution);
    assert_eq!(&RequestCause::UserSubmission, &started[0].cause);
    assert_eq!(
        &RequestCause::ToolContinuation {
            parent_request_id: started[0].scope.request_id.clone(),
        },
        &started[1].cause
    );
    assert_eq!(started[0].scope, completed[0].scope);
    assert_eq!(started[1].scope, completed[1].scope);
    assert_eq!(
        Some("provider-request-one"),
        completed[0].provider_request_id.as_deref()
    );
    assert_eq!(
        Some("provider-request-two"),
        completed[1].provider_request_id.as_deref()
    );
    assert_eq!(Some(5), completed[0].usage.input_tokens);
    assert_eq!(Some(6), completed[0].usage.output_tokens);
    assert_eq!(Some(10), completed[1].usage.input_tokens);
    assert_eq!(Some(8), completed[1].usage.output_tokens);
    assert!(completed.iter().all(|fact| fact.elapsed_ms.is_some()));

    let execution = &started[0].scope.execution;
    assert!(events.iter().all(|event| match &event.payload {
        EventPayload::ContextSources { request, .. }
        | EventPayload::ModelResponse { request, .. }
        | EventPayload::ModelError { request, .. }
        | EventPayload::ToolCall { request, .. } => &request.execution == execution,
        EventPayload::MessageUser {
            execution: fact_execution,
            ..
        }
        | EventPayload::MessageAssistant {
            execution: fact_execution,
            ..
        }
        | EventPayload::PermissionAllow {
            execution: fact_execution,
            ..
        }
        | EventPayload::PermissionRequested {
            execution: fact_execution,
            ..
        }
        | EventPayload::PermissionDeny {
            execution: fact_execution,
            ..
        }
        | EventPayload::ToolResult {
            execution: fact_execution,
            ..
        }
        | EventPayload::Handoff {
            execution: fact_execution,
            ..
        }
        | EventPayload::TurnStarted {
            execution: fact_execution,
            ..
        }
        | EventPayload::TurnCompleted {
            execution: fact_execution,
            ..
        }
        | EventPayload::TurnCancelled {
            execution: fact_execution,
            ..
        }
        | EventPayload::TurnInterrupted {
            execution: fact_execution,
            ..
        } => fact_execution == execution,
        _ => true,
    }));
}
