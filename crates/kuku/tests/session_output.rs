mod common;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use httpmock::prelude::{HttpMockRequest, MockServer, POST};
use httpmock::Mock;
use serde_json::{json, Value};

use kuku::event::EventPayload;
use kuku::permission::{AuthorizationDecision, ToolAuthorization, ToolAuthorizer};
use kuku::{
    AgentConfig, AgentStatus, Input, InputSource, OutputChunk, OutputRead, Session, SessionOptions,
};

const MARKER: &str = "OUTPUT_TARGET_标记";
const RESULT_BYTES: usize = 16_384;

#[derive(Debug, Default)]
struct ReadPolicy {
    actions: Mutex<Vec<ToolAuthorization>>,
}

impl ToolAuthorizer for ReadPolicy {
    fn authorize(&self, action: &ToolAuthorization) -> kuku::Result<AuthorizationDecision> {
        self.actions.lock().unwrap().push(action.clone());
        Ok(if action.tool_call_id == "denied" {
            AuthorizationDecision::Deny
        } else {
            AuthorizationDecision::Allow
        })
    }
}

fn body(request: &HttpMockRequest) -> Value {
    serde_json::from_slice(request.body.as_deref().unwrap()).unwrap()
}

fn results(request: &HttpMockRequest) -> Vec<Value> {
    body(request)["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["role"] == "tool")
        .cloned()
        .collect()
}

fn producing(request: &HttpMockRequest) -> bool {
    body(request)["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|item| item["role"] == "user")
        .is_some_and(|item| {
            item["content"]
                .as_str()
                .is_some_and(|text| text.contains("produce-output"))
        })
}

fn tool(id: &str, name: &str, args: Value) -> Value {
    json!({"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}})
}

async fn fixture<'a>(
    server: &'a MockServer,
    matcher: fn(&HttpMockRequest) -> bool,
    message: Value,
    reason: &str,
) -> Mock<'a> {
    let response = common::openai_sse_response(json!({
        "id":"session-output","choices":[{"message":message,"finish_reason":reason}],
        "usage":{"prompt_tokens":100,"completion_tokens":20}
    }));
    server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions").matches(matcher);
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(response);
        })
        .await
}

async fn idle(session: &Session) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let state = session.agent("main").unwrap();
            if state.status != AgentStatus::Running {
                assert_eq!(AgentStatus::Idle, state.status, "{:?}", state.last_error);
                assert!(state.last_error.is_none());
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("output fixture did not settle through the public Session");
}

async fn captured(
    server: &MockServer,
    workspace: &Path,
    data: &Path,
    payload: &[u8],
    policy: Arc<ReadPolicy>,
) -> (Session, String) {
    std::fs::write(workspace.join("captured.bin"), payload).unwrap();
    let command = if cfg!(windows) {
        "type captured.bin"
    } else {
        "cat captured.bin"
    };
    let first = fixture(server, |request| producing(request) && results(request).is_empty(), json!({"tool_calls":[tool("command", "run_command", json!({"command":command,"timeout":5,"brief":"retain output"}))]}), "tool_calls").await;
    let second = fixture(
        server,
        |request| producing(request) && results(request).len() == 1,
        json!({"content":"retained"}),
        "stop",
    )
    .await;
    let mut config = common::test_config();
    config.providers.get_mut("openai").unwrap().base_url = server.url("");
    let tier = config.tiers.get_mut("balanced").unwrap();
    tier.provider = "openai".into();
    tier.model = "session-output".into();
    tier.think = kuku::config::ThinkLevel::Off;
    config.handoff.enabled = false;
    config.plugin.enabled = false;
    let mut options = SessionOptions::new(workspace, data, config);
    options.authorizer = policy;
    options.limits.max_preview_bytes = 256;
    options.limits.max_steps_per_turn = 8;
    let session = Session::open(options).await.unwrap();
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
        .send(Input::new(
            "produce",
            "main",
            "produce-output from captured.bin",
        ))
        .unwrap();
    idle(&session).await;
    let id = session
        .events(0, 256)
        .unwrap()
        .into_iter()
        .find_map(|event| match event.payload {
            EventPayload::ToolResult {
                tool_call_id,
                structured: Some(value),
                ..
            } if tool_call_id == "command" => value["output_id"].as_str().map(str::to_string),
            _ => None,
        })
        .expect("real command did not retain an output ID");
    first.assert_hits_async(1).await;
    second.assert_hits_async(1).await;
    (session, id)
}

fn chunk(
    session: &Session,
    id: &str,
    offset: u64,
    limit: usize,
    search: Option<&str>,
) -> OutputChunk {
    session
        .read_output(
            id,
            &OutputRead {
                offset,
                limit,
                search: search.map(str::to_string),
            },
        )
        .unwrap()
}

fn stored_result(session: &Session, id: &str) -> (String, String, Option<Value>) {
    session
        .events(0, 512)
        .unwrap()
        .into_iter()
        .find_map(|event| match event.payload {
            EventPayload::ToolResult {
                tool_call_id,
                status,
                model_content,
                structured,
                ..
            } if tool_call_id == id => Some((status, model_content, structured)),
            _ => None,
        })
        .expect("model tool did not commit a result")
}

fn json_page(result: &(String, String, Option<Value>)) -> Value {
    assert_eq!("ok", result.0);
    assert!(result.1.len() <= RESULT_BYTES);
    let parsed: Value =
        serde_json::from_str(&result.1).expect("model output page must remain complete JSON");
    assert_eq!(Some(&parsed), result.2.as_ref());
    parsed
}

fn final_pages(request: &HttpMockRequest) -> bool {
    let results = results(request);
    if results.len() != 4 {
        return false;
    }
    let pages = ["page-1", "page-2", "search"].map(|id| {
        results
            .iter()
            .find(|item| item["tool_call_id"] == id)
            .and_then(|item| item["content"].as_str())
            .and_then(|text| serde_json::from_str::<Value>(text).ok())
    });
    pages[0].as_ref().is_some_and(|page| {
        page["offset"] == 0 && page["next_offset"] == 8192 && page["has_more"] == true
    }) && pages[1].as_ref().is_some_and(|page| {
        page["offset"] == 8192 && page["next_offset"] == 16384 && page["has_more"] == true
    }) && pages[2].as_ref().is_some_and(|page| {
        page["offset"] == 16384
            && page["filtered"] == true
            && page["has_more"] == false
            && page["segments"]
                .as_array()
                .unwrap()
                .iter()
                .any(|segment| segment["text"].as_str().unwrap().contains(MARKER))
    })
}

#[tokio::test]
async fn model_reads_and_searches_retained_command_output_without_consuming_ui_reads() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let policy = Arc::new(ReadPolicy::default());
    let payload = format!("{}\n{MARKER}\n{}", "p".repeat(20_000), "q".repeat(4096));
    let (session, id) = captured(
        &server,
        workspace.path(),
        data.path(),
        payload.as_bytes(),
        policy.clone(),
    )
    .await;
    let command = stored_result(&session, "command");
    assert!(
        !command.1.contains(MARKER),
        "initial model preview already contained the target marker"
    );
    let first = chunk(&session, &id, 0, 8192, None);
    let second = chunk(&session, &id, first.next_offset, 8192, None);
    let searched = chunk(&session, &id, second.next_offset, 8192, Some(MARKER));
    for _ in 0..2 {
        assert_eq!(first, chunk(&session, &id, 0, 8192, None));
        assert_eq!(
            searched,
            chunk(&session, &id, second.next_offset, 8192, Some(MARKER))
        );
    }
    let page_1 = fixture(
        &server,
        |request| !producing(request) && results(request).len() == 1,
        json!({"tool_calls":[tool("page-1", "read_output", json!({"id":id,"limit":8192}))]}),
        "tool_calls",
    )
    .await;
    let page_2 = fixture(&server, |request| !producing(request) && results(request).len() == 2, json!({"tool_calls":[tool("page-2", "read_output", json!({"id":id,"offset":first.next_offset,"limit":8192}))]}), "tool_calls").await;
    let search = fixture(&server, |request| !producing(request) && results(request).len() == 3, json!({"tool_calls":[tool("search", "read_output", json!({"id":id,"offset":second.next_offset,"limit":8192,"search":MARKER}))]}), "tool_calls").await;
    let final_response = fixture(
        &server,
        final_pages,
        json!({"content":"retained marker verified"}),
        "stop",
    )
    .await;
    session
        .send(Input::new(
            "inspect",
            "main",
            "inspect retained output by its ID",
        ))
        .unwrap();
    idle(&session).await;
    assert_eq!(
        Some("retained marker verified"),
        session.agent("main").unwrap().last_output.as_deref()
    );
    for (call, expected) in [
        ("page-1", &first),
        ("page-2", &second),
        ("search", &searched),
    ] {
        assert_eq!(
            serde_json::to_value(expected).unwrap(),
            json_page(&stored_result(&session, call))
        );
        assert!(expected.complete);
        assert!(!expected.capture_truncated);
        assert!(!expected.cleanup_incomplete);
    }
    assert_eq!(first, chunk(&session, &id, 0, 8192, None));
    let actions = policy.actions.lock().unwrap().clone();
    let actions = actions
        .iter()
        .filter(|action| action.tool.name == "read_output")
        .collect::<Vec<_>>();
    assert_eq!(3, actions.len());
    for action in actions {
        assert_eq!("main", action.agent);
        assert_eq!(InputSource::Host("local".into()), action.source);
        assert_eq!(id, action.args["id"]);
        assert_eq!(8192, action.args["limit"]);
        assert_eq!(json!(["id"]), action.tool.input_schema["required"]);
    }
    session.close().await.unwrap();
    for mock in [page_1, page_2, search, final_response] {
        mock.assert_hits_async(1).await;
    }
}

#[cfg(unix)]
#[tokio::test]
async fn escaped_and_invalid_utf8_output_keeps_json_bounded_and_the_authorized_limit_unchanged() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let policy = Arc::new(ReadPolicy::default());
    let payload = [0xff, 0, 1, b'\n'].repeat(2048);
    let (session, id) = captured(
        &server,
        workspace.path(),
        data.path(),
        &payload,
        policy.clone(),
    )
    .await;
    let read = fixture(
        &server,
        |request| !producing(request) && results(request).len() == 1,
        json!({"tool_calls":[tool("escaped", "read_output", json!({"id":id,"limit":8192}))]}),
        "tool_calls",
    )
    .await;
    let done = fixture(
        &server,
        |request| !producing(request) && results(request).len() == 2,
        json!({"content":"bounded page"}),
        "stop",
    )
    .await;
    session
        .send(Input::new(
            "inspect",
            "main",
            "inspect escaped retained bytes",
        ))
        .unwrap();
    idle(&session).await;
    let page = json_page(&stored_result(&session, "escaped"));
    assert_eq!(true, page["lossy_utf8"]);
    assert_eq!(true, page["has_more"]);
    let next = page["next_offset"].as_u64().unwrap();
    assert!((1..8192).contains(&next));
    assert_eq!(
        serde_json::to_value(chunk(&session, &id, 0, next as usize, None)).unwrap(),
        page
    );
    let actions = policy.actions.lock().unwrap().clone();
    assert_eq!(
        Some(json!({"id":id,"limit":8192})),
        actions
            .iter()
            .find(|action| action.tool_call_id == "escaped")
            .map(|action| action.args.clone())
    );
    session.close().await.unwrap();
    read.assert_hits_async(1).await;
    done.assert_hits_async(1).await;
}

#[tokio::test]
async fn invalid_output_arguments_and_host_denial_never_expose_outside_output() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let policy = Arc::new(ReadPolicy::default());
    let (session, id) = captured(
        &server,
        workspace.path(),
        data.path(),
        b"retained evidence",
        policy.clone(),
    )
    .await;
    std::fs::write(data.path().join("outside.output"), "PRIVATE_OUTSIDE_MARKER").unwrap();
    let requested = fixture(
        &server,
        |request| !producing(request) && results(request).len() == 1,
        json!({"tool_calls":[
            tool("bad-id", "read_output", json!({"id":"../../outside","limit":8192})),
            tool("zero", "read_output", json!({"id":"absent","limit":0})),
            tool("over", "read_output", json!({"id":"absent","limit":8193})),
            tool("denied", "read_output", json!({"id":id,"offset":3,"limit":9,"search":"evidence"}))
        ]}),
        "tool_calls",
    )
    .await;
    let done = fixture(
        &server,
        |request| !producing(request) && results(request).len() == 5,
        json!({"content":"invalid reads rejected"}),
        "stop",
    )
    .await;
    session
        .send(Input::new(
            "inspect",
            "main",
            "inspect invalid output requests",
        ))
        .unwrap();
    idle(&session).await;
    for call in ["bad-id", "zero", "over"] {
        let result = stored_result(&session, call);
        assert_eq!("error", result.0);
        assert!(!result.1.contains("PRIVATE_OUTSIDE_MARKER"));
        assert!(result.1.len() <= RESULT_BYTES);
        assert!(result.1.contains(if call == "bad-id" {
            "output id"
        } else {
            "limit"
        }));
    }
    assert_eq!("blocked", stored_result(&session, "denied").0);
    assert!(!session.events(0, 512).unwrap().iter().any(|event| matches!(&event.payload, EventPayload::Unknown(value) if value["kind"] == "runtime.tool_dispatched" && value["tool_call_id"] == "denied")));
    let actions = policy.actions.lock().unwrap().clone();
    assert_eq!(
        Some(json!({"id":id,"offset":3,"limit":9,"search":"evidence"})),
        actions
            .iter()
            .find(|action| action.tool_call_id == "denied")
            .map(|action| action.args.clone())
    );
    session.close().await.unwrap();
    requested.assert_hits_async(1).await;
    done.assert_hits_async(1).await;
}
