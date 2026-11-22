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
    AgentConfig, AgentStatus, Error, Input, InputSource, NamedValueSource, OutputChunk, OutputRead,
    Session, SessionOptions,
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
        Ok(
            if action.tool_call_id == "denied"
                || (action.tool.name == "read_output" && action.args["text"] == "withheld")
            {
                AuthorizationDecision::Deny
            } else {
                AuthorizationDecision::Allow
            },
        )
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
    latest_input_contains(request, "produce-output")
}

fn latest_input_contains(request: &HttpMockRequest, marker: &str) -> bool {
    body(request)["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|item| item["role"] == "user")
        .is_some_and(|item| {
            item["content"]
                .as_str()
                .is_some_and(|text| text.contains(marker))
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

async fn output_session(
    server: &MockServer,
    workspace: &Path,
    data: &Path,
    policy: Arc<ReadPolicy>,
) -> Session {
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
    options.limits.max_preview_bytes = 1024;
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
}

async fn captured(
    server: &MockServer,
    workspace: &Path,
    data: &Path,
    payload: &[u8],
    policy: Arc<ReadPolicy>,
) -> (Session, String) {
    std::fs::write(workspace.join("captured.bin"), payload).unwrap();
    let args = if cfg!(windows) {
        common::shell_args("type captured.bin")
    } else {
        json!({"program":"cat","args":["captured.bin"]})
    };
    let first = fixture(
        server,
        |request| producing(request) && results(request).is_empty(),
        json!({"tool_calls":[tool("command", "run_command", args)]}),
        "tool_calls",
    )
    .await;
    let second = fixture(
        server,
        |request| {
            producing(request)
                && results(request).first().is_some_and(|result| {
                    result["content"]
                        .as_str()
                        .is_some_and(|text| text.contains("Output: "))
                })
        },
        json!({"content":"retained"}),
        "stop",
    )
    .await;
    let session = output_session(server, workspace, data, policy).await;
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
        command.1.contains("Output shape from the observed prefix:")
            && !command.1.contains(&"p".repeat(80)),
        "initial model result included raw bulk output: {}",
        command.1
    );
    let facts = command.2.unwrap();
    assert_eq!(true, facts["capture_complete"]);
    assert_eq!(true, facts["preview_truncated"]);
    assert!(facts["output_preview"]
        .as_str()
        .unwrap()
        .contains(&"p".repeat(80)));
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
        json!({"tool_calls":[tool("page-1", "read_output", json!({"id":id,"limit":9000}))]}),
        "tool_calls",
    )
    .await;
    let page_2 = fixture(&server, |request| !producing(request) && results(request).len() == 2, json!({"tool_calls":[tool("page-2", "read_output", json!({"id":id,"offset":first.next_offset,"limit":u64::MAX}))]}), "tool_calls").await;
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
    for (action, requested_limit) in actions.into_iter().zip([9000, u64::MAX, 8192]) {
        assert_eq!("main", action.agent);
        assert_eq!(InputSource::Host("local".into()), action.source);
        assert_eq!(id, action.args["id"]);
        assert_eq!(requested_limit, action.args["limit"]);
        assert_eq!(
            json!([{"required":["id"]},{"required":["name"]}]),
            action.tool.input_schema["oneOf"]
        );
    }
    session.close().await.unwrap();
    for mock in [page_1, page_2, search, final_response] {
        mock.assert_hits_async(1).await;
    }
}

#[cfg(unix)]
#[tokio::test]
async fn model_receives_short_output_and_an_extraction_hint_for_large_output() {
    let line = |name: &str| format!("{{\"id\":1,\"name\":\"{name}\"}}\n");
    for long in [false, true] {
        let mut payload = line("first-record") + &line("second-record");
        if long {
            payload.push_str(&line("extra").repeat(100));
        }
        let server = MockServer::start_async().await;
        let workspace = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (session, id) = captured(
            &server,
            workspace.path(),
            data.path(),
            payload.as_bytes(),
            Arc::new(ReadPolicy::default()),
        )
        .await;
        let (status, content, facts) = stored_result(&session, "command");
        assert_eq!("ok", status);
        assert!(content.contains(&format!("Output: {id}")));
        if long {
            assert!(!content.contains("first-record") && !content.contains("second-record"));
            assert!(
                content.contains("first two lines are JSON objects"),
                "{content}"
            );
            assert!(content.contains(r#"["id","name"]"#), "{content}");
            assert!(
                content.contains(r#"tools.output_lines({id, stream:"stdout"})"#),
                "{content}"
            );
            assert!(
                content.contains("tools.search_output({id, text})"),
                "{content}"
            );
        } else {
            assert!(
                content.contains("first-record") && content.contains("second-record"),
                "{content}"
            );
        }
        assert_eq!(true, facts.unwrap()["capture_complete"]);
        let raw: Vec<_> = chunk(&session, &id, 0, 8192, None)
            .segments
            .into_iter()
            .flat_map(|segment| segment.as_bytes().to_vec())
            .collect();
        assert_eq!(payload.as_bytes(), raw);
        session.close().await.unwrap();
    }
}

#[tokio::test]
async fn script_search_resolves_a_named_capture_and_uses_the_read_output_permission() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let policy = Arc::new(ReadPolicy::default());
    let (session, id) = captured(
        &server,
        workspace.path(),
        data.path(),
        b"x needle withheld needle\n",
        policy.clone(),
    )
    .await;
    session
        .save_named_value("main", "log", NamedValueSource::Output(id.clone()), false)
        .await
        .unwrap();
    let requested = fixture(
        &server,
        |request| !producing(request) && results(request).len() == 1,
        json!({"tool_calls":[tool("script-search", "run_js", json!({"code":r#"
            const first = await tools.search_output({name:"log",text:"needle",stream:"stdout",limit:1});
            if (first.status !== "ok") return first;
            const second = await tools.search_output({id:first.result.output_id,text:"needle",offset:first.result.next_offset});
            const denied = await tools.search_output({name:"log",text:"withheld"});
            return {first:first.result,second:second.result,denied:denied.status};
        "#}))]}),
        "tool_calls",
    ).await;
    let done = fixture(
        &server,
        |request| !producing(request) && results(request).len() == 2,
        json!({"content":"script search settled"}),
        "stop",
    )
    .await;
    session
        .send(Input::new(
            "search",
            "main",
            "find the two known strings in the saved log",
        ))
        .unwrap();
    idle(&session).await;
    let (status, content, facts) = stored_result(&session, "script-search");
    assert_eq!("ok", status, "{content}");
    let returned = &facts.unwrap()["value"];
    assert_eq!(
        returned["first"]["matches"],
        json!([{"stream":"stdout","offset":2,"end_offset":8}])
    );
    assert_eq!(returned["first"]["has_more"], true);
    assert_eq!(
        returned["second"]["matches"],
        json!([{"stream":"stdout","offset":18,"end_offset":24}])
    );
    assert_eq!(returned["second"]["has_more"], false);
    assert_eq!(returned["denied"], "blocked");
    let actions = policy
        .actions
        .lock()
        .unwrap()
        .iter()
        .filter(|action| action.tool.name == "read_output")
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(3, actions.len());
    for action in &actions {
        assert_eq!(id, action.args["id"]);
        assert!(action.args.get("name").is_none());
        assert_eq!(json!(["text"]), action.tool.input_schema["required"]);
    }
    assert_eq!(json!("withheld"), actions[2].args["text"]);
    session.close().await.unwrap();
    requested.assert_hits_async(1).await;
    done.assert_hits_async(1).await;
}

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
            tool("fraction", "read_output", json!({"id":"absent","limit":1.5})),
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
    for call in ["bad-id", "zero", "fraction"] {
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

#[tokio::test]
async fn model_reads_a_running_job_through_direct_and_script_tools() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let command = if cfg!(windows) {
        std::fs::write(workspace.path().join("gate.cmd"), concat!(
            "@echo off\r\necho ready-prefix\r\nset /a ticks=0 >nul\r\n:wait\r\n",
            "if exist release.txt goto emit\r\nif %ticks% geq 20 exit /b 98\r\n",
            "set /a ticks+=1 >nul\r\n%SystemRoot%\\System32\\ping.exe -n 2 127.0.0.1 >nul\r\ngoto wait\r\n",
            ":emit\r\necho final-suffix\r\n",
        )).unwrap();
        "gate.cmd"
    } else {
        "printf 'ready-prefix\\n'; while [ ! -f release.txt ]; do sleep 0.05; done; printf 'final-suffix\\n'"
    };
    let mut args = common::shell_args(command);
    args["background"] = json!(true);
    let start = fixture(
        &server,
        |request| producing(request) && results(request).is_empty(),
        json!({"tool_calls":[tool("command", "run_command", args)]}),
        "tool_calls",
    )
    .await;
    let started = fixture(
        &server,
        |request| producing(request) && results(request).len() == 1,
        json!({"content":"command is running"}),
        "stop",
    )
    .await;
    let session = output_session(
        &server,
        workspace.path(),
        data.path(),
        Arc::new(ReadPolicy::default()),
    )
    .await;
    session
        .send(Input::new(
            "produce",
            "main",
            "produce-output from a background process",
        ))
        .unwrap();
    idle(&session).await;
    let receipt = stored_result(&session, "command").2.unwrap();
    let id = receipt["output_id"].as_str().unwrap();
    let job = receipt["job_id"].as_str().unwrap();
    let read = json!({"id":id});
    let code = format!(
        r#"
const found = await tools.search_output({{id:"{id}",text:"ready-prefix"}});
return {{matchCount:found.result.matches.length,captureComplete:found.result.capture_complete}};
"#
    );
    let requested = fixture(&server, |request| latest_input_contains(request, "sample capture") && results(request).len() == 1,
        json!({"tool_calls":[tool("page", "read_output", read.clone()), tool("denied", "read_output", read), tool("script", "run_js", json!({"code":code}))]}),
        "tool_calls").await;
    let sampled = fixture(
        &server,
        |request| latest_input_contains(request, "sample capture") && results(request).len() == 4,
        json!({"content":"sampled live capture"}),
        "stop",
    )
    .await;
    session
        .send(Input::new(
            "sample",
            "main",
            "sample capture before releasing the process",
        ))
        .unwrap();
    idle(&session).await;
    let page = json_page(&stored_result(&session, "page"));
    assert_eq!(false, page["complete"]);
    assert_eq!(false, page["has_more"]);
    assert!(page["segments"][0]["text"]
        .as_str()
        .unwrap()
        .contains("ready-prefix"));
    assert_eq!("blocked", stored_result(&session, "denied").0);
    let script = stored_result(&session, "script");
    assert_eq!("ok", script.0);
    assert_eq!(
        json!({"matchCount":1,"captureComplete":false}),
        script.2.unwrap()["value"]
    );
    assert!(
        matches!(session.save_named_value("main", "early", NamedValueSource::Output(id.to_owned()), false).await, Err(Error::InvalidArgument(message)) if message.contains("still being captured"))
    );

    std::fs::write(workspace.path().join("release.txt"), []).unwrap();
    session.wait_job("main", job).await.unwrap();
    let end = chunk(
        &session,
        id,
        page["next_offset"].as_u64().unwrap(),
        100,
        None,
    );
    assert!(end.complete);
    assert!(end.segments[0].text.contains("final-suffix"));
    session.close().await.unwrap();
    start.assert_hits_async(1).await;
    started.assert_hits_async(1).await;
    requested.assert_hits_async(1).await;
    sampled.assert_hits_async(1).await;
}

#[tokio::test]
async fn naming_command_output_reuses_its_file_and_replaced_body_can_expire() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let (session, id) = captured(
        &server,
        workspace.path(),
        data.path(),
        "retained evidence\n".repeat(9_000).as_bytes(),
        Arc::new(ReadPolicy::default()),
    )
    .await;
    let dir = session.events_path().parent().unwrap().join("outputs");
    let original_path = dir.join(format!("{id}.output"));
    let original_bytes = std::fs::metadata(&original_path).unwrap().len();
    let named = session
        .save_named_value(
            "main",
            "baseline",
            NamedValueSource::Output(id.clone()),
            false,
        )
        .await
        .unwrap();
    assert_eq!(id, named.reference.id);
    assert_eq!(1, std::fs::read_dir(&dir).unwrap().count());
    assert_eq!(
        original_bytes,
        std::fs::metadata(&original_path).unwrap().len()
    );
    let replacement = session
        .save_named_value(
            "main",
            "baseline",
            NamedValueSource::Text("new baseline".into()),
            true,
        )
        .await
        .unwrap();
    assert!(original_path.exists());
    let mut options = SessionOptions::new(workspace.path(), data.path(), common::test_config());
    options.session_id = Some(session.id().into());
    options.limits.output_retention_secs = 0;
    session.close().await.unwrap();

    let reopened = Session::open(options).await.unwrap();
    assert_eq!(
        replacement,
        reopened.named_value("main", "baseline").unwrap()
    );
    assert!(matches!(
        reopened.read_output(&id, &OutputRead::default()),
        Err(Error::OutputUnavailable(_))
    ));
    assert!(!original_path.exists());
    assert!(reopened.events(0, 256).unwrap().iter().any(|event| {
        matches!(&event.payload, EventPayload::ToolResult { structured: Some(facts), .. } if facts["output_ref"]["id"] == id)
    }));
    reopened.close().await.unwrap();
}
