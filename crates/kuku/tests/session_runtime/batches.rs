use super::{fixture, options, settled, tool, EventPayload, MockServer, Session};
use kuku::{AgentConfig, AgentStatus, Input};
use serde_json::{json, Value};

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
