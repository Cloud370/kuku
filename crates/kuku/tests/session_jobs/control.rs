use httpmock::prelude::{HttpMockRequest, MockServer, POST};
use serde_json::{json, Value};

use kuku::event::EventPayload;
use kuku::{AgentStatus, JobStatus, Session, WorkspaceScriptFacts};

use super::{
    command_args, committed, common, first_request, job_completed_event, submit, wait_until,
    wire_body, wire_requests, Fixture, ACKNOWLEDGED, AGENT, CALL,
};

const CONTROL: &str = "control-fixture-job";
const PREPARED_LIMIT: usize = 64 * 1024;
const ACTION_COUNT: usize = 240;

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn explicit_job_control_uses_one_bounded_terminal_receipt() {
    let server = MockServer::start_async().await;
    let command = command_args(false);
    let script = format!(
        r#"await tools.run_command({command});
for (let i = 0; i < {ACTION_COUNT}; i++) await tools.stat_file({{path:"stdout.txt"}});
return {{proof:"file checks completed"}};"#,
    );
    let failing_script = format!(
        r#"const command = await tools.run_command({command});
return (await command.stdout.text()).repeat(2000);"#,
    );

    for (route, tool, args, control, status, text) in [
        (
            "/bounded-wait-script",
            "run_js",
            json!({"code":script,"background":true}),
            "wait_job",
            JobStatus::Completed,
            "file checks completed",
        ),
        (
            "/failed-wait-script",
            "run_js",
            json!({"code":failing_script,"background":true}),
            "wait_job",
            JobStatus::Failed,
            "script result exceeds",
        ),
        (
            "/failed-wait-command",
            "run_command",
            command_args(true),
            "wait_job",
            JobStatus::Failed,
            "command exited 7",
        ),
        (
            "/bounded-cancel-command",
            "run_command",
            command_args(true),
            "cancel_job",
            JobStatus::Cancelled,
            "command cancelled",
        ),
    ] {
        let mut fixture = Fixture::new(&server, route, 20_000);
        fixture.options.limits.max_request_bytes = PREPARED_LIMIT;
        let path = format!("{route}/chat/completions");
        let responses = [
            tool_response(CALL, tool, args),
            tool_response(CONTROL, control, json!({"job_id":"job-1"})),
            common::openai_sse_response(json!({
                "choices":[{"message":{"content":ACKNOWLEDGED},"finish_reason":"stop"}]
            })),
        ];
        let matchers: [fn(&HttpMockRequest) -> bool; 3] = [
            |request| first_request(&wire_body(request)),
            |request| last_tool_is(request, CALL),
            |request| last_tool_is(request, CONTROL),
        ];
        let mut mocks = Vec::new();
        for (matcher, response) in matchers.into_iter().zip(responses) {
            let mock = server
                .mock_async(|when, then| {
                    when.method(POST)
                        .path(&path)
                        .matches(matcher)
                        .matches(common::capture_http_request);
                    then.status(200)
                        .header("content-type", "text/event-stream")
                        .body(response);
                })
                .await;
            mocks.push(mock);
        }
        let session = fixture.open().await;
        let mut observer = session.observe(0);
        submit(&session);
        committed(&mut observer, |event| {
            matches!(event, EventPayload::Unknown(value) if value["kind"] == "runtime.tool_dispatched"
                && value["tool_call_id"] == CONTROL)
        }).await;
        if control == "wait_job" {
            fixture.release();
        }
        let terminal = committed(&mut observer, |event| job_completed_event(event, "job-1")).await;
        let EventPayload::Unknown(terminal) = terminal.payload else {
            panic!("expected a durable Job result");
        };
        let proof = terminal["record"]["delivery"]["text"].as_str().unwrap();
        let result = committed(&mut observer, |event| {
            matches!(event, EventPayload::ToolResult { tool_call_id, .. } if tool_call_id == CONTROL)
        }).await;
        let EventPayload::ToolResult {
            model_content,
            structured: Some(receipt),
            ..
        } = result.payload
        else {
            panic!("expected a Job control receipt");
        };
        assert_eq!(proof, model_content);
        assert!(
            proof.contains(text) && proof.contains("Elapsed "),
            "{proof}"
        );
        assert!(proof.len() < 2048);
        assert!(receipt["result"]["structured"].is_null());
        let observed = session.job(AGENT, "job-1").unwrap();
        assert_eq!(status, observed.status);
        let full_result = observed.result.unwrap().structured.unwrap();
        if status == JobStatus::Completed {
            let script = WorkspaceScriptFacts::from_structured(&full_result).unwrap();
            assert_eq!(ACTION_COUNT + 1, script.actions.len());
        } else {
            let reference = if tool == "run_command" {
                &full_result["output_ref"]["id"]
            } else {
                &full_result["actions"][0]["result"]["output_ref"]["id"]
            };
            assert!(proof.contains(reference.as_str().unwrap()), "{proof}");
        }
        wait_until(|| session.agent(AGENT).unwrap().status == AgentStatus::Idle).await;
        for mock in mocks {
            mock.assert_hits_async(1).await;
        }
        let wire = wire_requests(route);
        assert_eq!(3, wire.len());
        let messages = wire[2]["messages"].as_array().unwrap();
        assert_eq!(
            1,
            messages
                .iter()
                .filter(|msg| msg["content"].as_str() == Some(proof))
                .count()
        );
        assert!(!messages.iter().any(|msg| msg["content"]
            .as_str()
            .is_some_and(|text| text.starts_with("<kuku_job_result>"))));
        assert_eq!(0, session.agent(AGENT).unwrap().pending_inputs);
        session.close().await.unwrap();
        let mut options = fixture.options.clone();
        options.session_id = Some(session.id().into());
        let reopened = Session::open(options).await.unwrap();
        assert_eq!(0, reopened.agent(AGENT).unwrap().pending_inputs);
        assert_eq!(status, reopened.job(AGENT, "job-1").unwrap().status);
        reopened.close().await.unwrap();
    }
}

fn last_tool_is(request: &HttpMockRequest, id: &str) -> bool {
    wire_body(request)["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|message| message["role"] == "tool")
        .is_some_and(|message| message["tool_call_id"] == id)
}

fn tool_response(id: &str, tool: &str, args: Value) -> String {
    common::openai_sse_response(json!({
        "choices":[{"message":{"tool_calls":[{
            "id":id,"type":"function","function":{"name":tool,"arguments":args.to_string()}
        }]},"finish_reason":"tool_calls"}]
    }))
}
