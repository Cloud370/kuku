use super::{fixture, options, settled, tool, EventPayload, MockServer, Session};
use kuku::event::ModelStopReason;
use kuku::{
    AdapterFuture, AgentConfig, AgentStatus, Input, ModelAdapter, ModelEvent, ModelRequest,
    ModelStream,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn workspace_reads_overlap_and_wait_for_earlier_mutations_in_the_batch() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("answer.txt"), "before-value\n").unwrap();
    std::fs::write(workspace.path().join("other.txt"), "independent-value\n").unwrap();
    let calls = [
        tool("before", "read_file", json!({"path":"answer.txt"})),
        tool("other", "read_file", json!({"path":"other.txt"})),
        tool(
            "edit",
            "edit_file",
            json!({"path":"answer.txt","old_text":"before-value","new_text":"after-value"}),
        ),
        tool("after", "read_file", json!({"path":"answer.txt"})),
        tool("missing", "read_file", json!({"path":"missing.txt"})),
    ];
    let first = fixture(
        &server,
        |request| super::tool_count(request) == 0,
        json!({"tool_calls":calls}),
        "tool_calls",
    )
    .await;
    let last = fixture(
        &server,
        |request| {
            let body: Value =
                serde_json::from_slice(request.body.as_deref().unwrap_or_default()).unwrap();
            let tools = body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|row| row["role"] == "tool")
                .collect::<Vec<_>>();
            tools.len() == 5
                && [
                    ("before", "before-value"),
                    ("other", "independent-value"),
                    ("after", "after-value"),
                ]
                .into_iter()
                .all(|(id, value)| {
                    tools.iter().any(|row| {
                        row["tool_call_id"] == id
                            && row["content"]
                                .as_str()
                                .is_some_and(|text| text.contains(value))
                    })
                })
        },
        json!({"content":"results received"}),
        "stop",
    )
    .await;
    let mut options = options(&server, workspace.path(), data.path());
    options.limits.max_requests_per_open = Some(2);
    let session = Session::open(options).await.unwrap();
    let agent = AgentConfig {
        no_agents: true,
        ..Default::default()
    };
    session.create_agent("create", "main", agent).unwrap();
    session
        .send(Input::new(
            "input",
            "main",
            "read both files and update the answer",
        ))
        .unwrap();
    let result = settled(&session, "main").await;
    assert_eq!(result.status, AgentStatus::Idle, "{:?}", result.last_error);
    assert_eq!(result.last_output.as_deref(), Some("results received"));
    let events = session.events(0, 256).unwrap();
    let dispatched = |call: &str| {
        events
            .iter()
            .find(|event| {
                matches!(&event.payload, EventPayload::Unknown(value)
            if value["kind"] == "runtime.tool_dispatched" && value["tool_call_id"] == call)
            })
            .unwrap()
            .id
    };
    let settled = |call: &str, expected: &str| {
        events
            .iter()
            .find_map(|event| match &event.payload {
                EventPayload::ToolResult {
                    tool_call_id,
                    status,
                    ..
                } if tool_call_id == call => {
                    assert_eq!(status, expected, "{event:?}");
                    Some(event.id)
                }
                _ => None,
            })
            .unwrap()
    };
    let reads_finished = [settled("before", "ok"), settled("other", "ok")];
    assert!(dispatched("before").max(dispatched("other")) < *reads_finished.iter().min().unwrap());
    assert!(*reads_finished.iter().max().unwrap() < dispatched("edit"));
    assert!(settled("edit", "ok") < dispatched("after"));
    settled("missing", "error");
    first.assert_hits_async(1).await;
    last.assert_hits_async(1).await;
    session.close().await.unwrap();
}

#[tokio::test]
async fn malformed_tool_batch_reports_its_cause_without_dispatching_calls() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let calls = [
        tool("valid", "find_files", json!({"path":"."})),
        json!({"id":"invalid","type":"function","function":{
            "name":"read_file", "arguments":"{\"path\":PRIVATE_ARGUMENT_TEXT"
        }}),
    ];
    let model = fixture(&server, |_| true, json!({"tool_calls":calls}), "tool_calls").await;
    let mut settings = options(&server, workspace.path(), data.path());
    settings.limits.retry_transient_provider_errors = false;
    let session = Session::open(settings).await.unwrap();
    session
        .create_agent(
            "create",
            "main",
            AgentConfig {
                no_agents: true,
                ..Default::default()
            },
        )
        .unwrap();
    session
        .send(Input::new("input", "main", "read the workspace"))
        .unwrap();
    let result = settled(&session, "main").await;
    let expected = "The model sent tool arguments that were not a valid JSON object.";
    assert_eq!(result.status, AgentStatus::Interrupted);
    assert_eq!(
        result.last_error.as_deref(),
        Some(format!("provider error: {expected}").as_str())
    );
    let events = session.events(0, 128).unwrap();
    assert!(events.iter().any(|event| matches!(&event.payload,
        EventPayload::Unknown(value) if value["kind"] == "runtime.model_retry"
            && value["message"] == expected && value["will_retry"] == false)));
    assert!(!events
        .iter()
        .any(|event| matches!(&event.payload, EventPayload::ToolCall { .. })));
    model.assert_hits_async(1).await;
    session.close().await.unwrap();
}

#[derive(Debug)]
struct ToolArgumentModel {
    requests: Mutex<Vec<Value>>,
    always_invalid: bool,
}

impl ModelAdapter for ToolArgumentModel {
    fn stream(&self, request: ModelRequest) -> AdapterFuture<'_, ModelStream> {
        Box::pin(async move {
            let attempt = {
                let mut requests = self.requests.lock().unwrap();
                requests.push(request.body);
                requests.len()
            };
            let mut events = vec![ModelEvent::StreamStart {
                request_id: format!("model-{attempt}"),
            }];
            let reason = if attempt == 1 || self.always_invalid {
                events.push(ModelEvent::TextDelta {
                    text: "rejected-reply-text".into(),
                });
                add_tool(
                    &mut events,
                    0,
                    "rejected-write",
                    "write_file",
                    r#"{"path":"NODISPATCH","content":"unexpected"}"#,
                );
                add_tool(
                    &mut events,
                    1,
                    "rejected-read",
                    "read_file",
                    r#"{"path":broken-argument"#,
                );
                ModelStopReason::ToolUse
            } else if attempt == 2 {
                add_tool(
                    &mut events,
                    0,
                    "valid-read",
                    "read_file",
                    r#"{"path":"evidence.txt"}"#,
                );
                ModelStopReason::ToolUse
            } else {
                events.push(ModelEvent::TextDelta {
                    text: "finished with evidence".into(),
                });
                ModelStopReason::EndTurn
            };
            events.push(ModelEvent::StopReason { reason });
            events.push(ModelEvent::StreamEnd);
            Ok(Box::pin(tokio_stream::iter(events.into_iter().map(Ok))) as ModelStream)
        })
    }
}

fn add_tool(events: &mut Vec<ModelEvent>, index: u64, id: &str, name: &str, args: &str) {
    events.push(ModelEvent::ToolCallStart {
        id: id.into(),
        index,
        name: name.into(),
    });
    events.push(ModelEvent::ToolCallArgDelta {
        index,
        fragment: args.into(),
    });
    events.push(ModelEvent::ToolCallStop { index });
}

#[tokio::test]
async fn malformed_tool_arguments_regenerate_once_with_the_same_admitted_context() {
    let server = MockServer::start_async().await;
    for (always_invalid, budget, expected_requests, expected_regenerations) in
        [(false, 3, 3, 1), (true, 3, 2, 1), (true, 1, 1, 0)]
    {
        let workspace = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("evidence.txt"), "valid tool evidence").unwrap();
        let model = Arc::new(ToolArgumentModel {
            requests: Mutex::new(Vec::new()),
            always_invalid,
        });
        let mut settings = options(&server, workspace.path(), data.path());
        settings.model_adapter = Some(model.clone());
        settings.limits.max_requests_per_open = Some(budget);
        let session = Session::open(settings).await.unwrap();
        session
            .create_agent(
                "create",
                "main",
                AgentConfig {
                    no_agents: true,
                    ..Default::default()
                },
            )
            .unwrap();
        session
            .send(Input::new("input", "main", "Read the evidence file."))
            .unwrap();
        let snapshot = settled(&session, "main").await;
        assert_eq!(
            snapshot.status,
            if always_invalid {
                AgentStatus::Interrupted
            } else {
                AgentStatus::Idle
            }
        );
        assert!(!workspace.path().join("NODISPATCH").exists());
        let events = session.events(0, 128).unwrap();
        let retries = events.iter().filter(|event| matches!(&event.payload, EventPayload::Unknown(value)
              if value["kind"] == "runtime.model_retry" && value["recovery"] == "regenerate_tool_arguments")).count();
        assert_eq!(retries, expected_regenerations);
        let dispatched = events
            .iter()
            .filter_map(|event| match &event.payload {
                EventPayload::ToolCall { tool_call_id, .. } => Some(tool_call_id.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        if always_invalid {
            assert!(dispatched.is_empty());
        } else {
            assert_eq!(dispatched, ["valid-read"]);
            assert_eq!(
                snapshot.last_output.as_deref(),
                Some("finished with evidence")
            );
        }
        {
            let requests = model.requests.lock().unwrap();
            assert_eq!(requests.len(), expected_requests);
            if expected_requests > 1 {
                assert_eq!(requests[0]["messages"], requests[1]["messages"]);
            }
            if !always_invalid {
                let continued = requests[2]["messages"].to_string();
                assert!(continued.contains("valid tool evidence"));
                assert!(
                    !continued.contains("rejected-reply-text")
                        && !continued.contains("broken-argument")
                );
            }
        }
        session.close().await.unwrap();
    }
}
