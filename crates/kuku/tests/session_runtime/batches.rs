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
    replies: &'static [ArgumentReply],
}

#[derive(Debug, Clone, Copy)]
enum ArgumentReply {
    Invalid,
    NonObject,
    Read,
    Write,
    Done,
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
            let reason = match self.replies[attempt - 1] {
                ArgumentReply::Invalid | ArgumentReply::NonObject => {
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
                    let args = match self.replies[attempt - 1] {
                        ArgumentReply::NonObject => "[]",
                        _ => r#"{"path":broken-argument"#,
                    };
                    add_tool(&mut events, 1, "rejected-read", "read_file", args);
                    ModelStopReason::ToolUse
                }
                ArgumentReply::Read => {
                    add_tool(
                        &mut events,
                        0,
                        "valid-read",
                        "read_file",
                        r#"{"path":"evidence.txt"}"#,
                    );
                    ModelStopReason::ToolUse
                }
                ArgumentReply::Write => {
                    add_tool(
                        &mut events,
                        0,
                        "valid-write",
                        "write_file",
                        r#"{"path":"answer.txt","content":"verified answer"}"#,
                    );
                    ModelStopReason::ToolUse
                }
                ArgumentReply::Done => {
                    events.push(ModelEvent::TextDelta {
                        text: "finished with evidence".into(),
                    });
                    ModelStopReason::EndTurn
                }
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
async fn workspace_creation_batch_writes_nested_files_in_one_model_turn() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let files = [
        ("config", "package/pyproject.toml", "[project]\n"),
        ("source", "package/src/main.py", "VERSION = 1\n"),
        ("docs", "package/README.md", "# Package\n"),
    ];
    let calls = files.map(|(id, path, content)| {
        tool(
            id,
            "write_file",
            json!({ "path": path, "content": content }),
        )
    });
    let first = fixture(
        &server,
        |request| super::tool_count(request) == 0,
        json!({"tool_calls":calls}),
        "tool_calls",
    )
    .await;
    let last = fixture(
        &server,
        |request| super::tool_count(request) == 3,
        json!({"content":"files ready"}),
        "stop",
    )
    .await;
    let mut settings = options(&server, workspace.path(), data.path());
    settings.limits.max_requests_per_open = Some(2);
    let session = Session::open(settings).await.unwrap();
    session
        .create_agent("create", "main", AgentConfig::default())
        .unwrap();
    session
        .send(Input::new("input", "main", "create a new package"))
        .unwrap();
    assert_eq!(
        Some("files ready"),
        settled(&session, "main").await.last_output.as_deref()
    );
    let events = session.events(0, 256).unwrap();
    for (id, path, expected) in files {
        assert!(events.iter().any(|event| matches!(&event.payload,
            EventPayload::ToolResult { tool_call_id, status, .. }
                if tool_call_id == id && status == "ok")));
        assert_eq!(
            expected,
            std::fs::read_to_string(workspace.path().join(path)).unwrap()
        );
    }
    first.assert_hits_async(1).await;
    last.assert_hits_async(1).await;
    session.close().await.unwrap();
}

#[tokio::test]
async fn malformed_tool_arguments_regenerate_per_step_with_the_same_admitted_context() {
    use ArgumentReply::{Done, Invalid, NonObject, Read, Write};

    let server = MockServer::start_async().await;
    for (replies, budget, reopened, expected_regenerations, dispatched) in [
        (
            &[Invalid, Read, NonObject, Write, Done][..],
            5,
            false,
            2,
            &["valid-read", "valid-write"][..],
        ),
        (
            &[Invalid, Read, NonObject, Write, Done][..],
            2,
            true,
            2,
            &["valid-read", "valid-write"][..],
        ),
        (&[Invalid; 6][..], 7, false, 5, &[][..]),
        (&[Invalid][..], 1, false, 0, &[][..]),
    ] {
        let workspace = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("evidence.txt"), "valid tool evidence").unwrap();
        let model = Arc::new(ToolArgumentModel {
            requests: Mutex::new(Vec::new()),
            replies,
        });
        let mut settings = options(&server, workspace.path(), data.path());
        settings.model_adapter = Some(model.clone());
        settings.limits.max_requests_per_open = Some(budget);
        let mut session = Session::open(settings.clone()).await.unwrap();
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
        if reopened {
            assert_eq!(
                AgentStatus::Interrupted,
                settled(&session, "main").await.status
            );
            settings.session_id = Some(session.id().into());
            settings.limits.max_requests_per_open = Some(5);
            session.close().await.unwrap();
            session = Session::open(settings).await.unwrap();
            session.advance("main").unwrap();
        }
        let snapshot = settled(&session, "main").await;
        let completed = matches!(replies.last(), Some(Done));
        assert_eq!(
            snapshot.status,
            if completed {
                AgentStatus::Idle
            } else {
                AgentStatus::Interrupted
            }
        );
        assert!(!workspace.path().join("NODISPATCH").exists());
        let events = session.events(0, 128).unwrap();
        let retries = events.iter().filter(|event| matches!(&event.payload, EventPayload::Unknown(value)
              if value["kind"] == "runtime.model_retry" && value["recovery"] == "regenerate_tool_arguments")).count();
        assert_eq!(retries, expected_regenerations);
        let rejections: Vec<_> = events
            .iter()
            .filter_map(|event| match &event.payload {
                EventPayload::Unknown(value)
                    if value["kind"] == "runtime.tool_arguments_rejected" =>
                {
                    Some(value)
                }
                _ => None,
            })
            .collect();
        assert_eq!(if completed { 2 } else { replies.len() }, rejections.len());
        for (index, rejected) in rejections.iter().enumerate() {
            assert_eq!(1, rejected["tool_index"]);
            assert_eq!("read_file", rejected["tool"]);
            assert_eq!(
                if completed { 1 } else { index + 1 },
                rejected["step_attempt"]
            );
            assert_eq!(
                if completed && index == 1 {
                    "non_object"
                } else {
                    "invalid_json"
                },
                rejected["problem"]["reason"]
            );
            assert!(!rejected.to_string().contains("broken-argument"));
        }
        let actual_dispatches = events
            .iter()
            .filter_map(|event| match &event.payload {
                EventPayload::ToolCall { tool_call_id, .. } => Some(tool_call_id.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(dispatched, actual_dispatches);
        if completed {
            assert_eq!(
                snapshot.last_output.as_deref(),
                Some("finished with evidence")
            );
            assert_eq!(
                "verified answer",
                std::fs::read_to_string(workspace.path().join("answer.txt")).unwrap()
            );
        }
        {
            let requests = model.requests.lock().unwrap();
            assert_eq!(requests.len(), replies.len());
            if requests.len() > 1 {
                assert_eq!(requests[0]["messages"], requests[1]["messages"]);
            }
            if completed {
                assert_eq!(requests[2]["messages"], requests[3]["messages"]);
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
