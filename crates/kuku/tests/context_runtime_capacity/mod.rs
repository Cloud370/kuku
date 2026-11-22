use std::collections::BTreeMap;

use super::{agent, common, facts, idle, options, response, HttpMockRequest, MockServer, POST};
use kuku::event::{EventPayload, EventStore};
use kuku::{Input, NamedValueSource, OutputRead, Session};
use serde_json::{json, Value};

const WIRE_LIMIT: usize = 48 * 1024;
const PAGE_BYTES: usize = 8192;
const PAGE_COUNT: usize = 15;
const QUERY: &str = "Scan the saved output pages.";
const FOLLOW_UP: &str = "Continue using the recorded references.";

fn request_body(request: &HttpMockRequest) -> Value {
    serde_json::from_slice(request.body.as_deref().unwrap()).unwrap()
}

fn last_user(body: &Value) -> Option<&str> {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|entry| entry["role"] == "user")?["content"]
        .as_str()
}

fn page_results(body: &Value) -> Vec<(&str, &str)> {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["role"] == "tool")
        .map(|entry| {
            (
                entry["tool_call_id"].as_str().unwrap(),
                entry["content"].as_str().unwrap(),
            )
        })
        .filter(|(id, _)| id.starts_with("page-"))
        .collect()
}

fn page_data(index: usize) -> String {
    let header = format!("payload-page-{index:02}\n");
    format!("{header}{}", "x".repeat(PAGE_BYTES - header.len()))
}

#[tokio::test]
async fn a_bounded_request_references_consumed_output_pages_and_reopens_the_successor() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let (mut config, path) = options(&server, workspace.path(), data.path());
    config.limits.max_request_bytes = WIRE_LIMIT;
    let session = Session::open(config.clone()).await.unwrap();
    let mut agent = agent();
    agent.tools = Some(vec!["read_output".into(), "stat_file".into()]);
    session.create_agent("create", "main", agent).unwrap();
    let original = (0..PAGE_COUNT).map(page_data).collect::<String>();
    let saved = session
        .save_named_value("main", "capture", NamedValueSource::Text(original), false)
        .await
        .unwrap();
    std::fs::write(workspace.path().join("seed.txt"), "workspace fact").unwrap();

    let mut replies = Vec::new();
    for index in 0..=PAGE_COUNT {
        let (message, reason) = if index < PAGE_COUNT {
            let args = json!({"name":"capture","offset":index * PAGE_BYTES,"limit":PAGE_BYTES});
            let call = json!({"id":format!("page-{index}"),"type":"function",
                "function":{"name":"read_output","arguments":args.to_string()}});
            let calls = if index == 0 {
                vec![
                    call,
                    json!({"id":"stat-seed","type":"function","function":{
                    "name":"stat_file","arguments":r#"{"path":"seed.txt"}"#}}),
                ]
            } else {
                vec![call]
            };
            (json!({"tool_calls":calls}), "tool_calls")
        } else {
            (json!({"content":"Scan complete."}), "stop")
        };
        replies.push(
            server
                .mock_async(|when, then| {
                    let when = when.method(POST).path(&path).matches(|request| {
                        last_user(&request_body(request)) == Some(QUERY)
                            && common::capture_http_request(request)
                    });
                    match index {
                        0 => {
                            when.matches(|request| page_results(&request_body(request)).is_empty())
                        }
                        1 => when.body_contains(r#""tool_call_id":"stat-seed"}]"#),
                        _ => {
                            when.body_contains(format!(r#""tool_call_id":"page-{}"}}]"#, index - 1))
                        }
                    };
                    then.status(200)
                        .header("content-type", "text/event-stream")
                        .body(response(message, reason));
                })
                .await,
        );
    }
    let follow_up = server
        .mock_async(|when, then| {
            when.method(POST).path(&path).matches(|request| {
                last_user(&request_body(request)) == Some(FOLLOW_UP)
                    && common::capture_http_request(request)
            });
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(response(json!({"content":"Still available."}), "stop"));
        })
        .await;
    session.send(Input::new("scan", "main", QUERY)).unwrap();
    idle(&session, "main").await;
    let journal = session.events_path().to_path_buf();
    config.session_id = Some(session.id().into());
    session.close().await.unwrap();

    let reopened = Session::open(config).await.unwrap();
    reopened
        .send(Input::new("follow-up", "main", FOLLOW_UP))
        .unwrap();
    idle(&reopened, "main").await;
    let page = reopened
        .read_agent_output(
            "main",
            &saved.reference.id,
            &OutputRead {
                offset: 0,
                limit: PAGE_BYTES,
                search: None,
            },
        )
        .unwrap();
    assert_eq!(page_data(0), page.segments[0].text);
    reopened.close().await.unwrap();
    for reply in replies {
        reply.assert_hits_async(1).await;
    }
    follow_up.assert_hits_async(1).await;

    let bodies = common::take_http_requests(&path);
    let events = EventStore::replay(&journal).unwrap();
    let segments = facts(&events, "runtime.context_segment", "main");
    let steps = facts(&events, "runtime.step_prepared", "main");
    let original_pages = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::ToolResult {
                tool_call_id,
                model_content,
                ..
            } if tool_call_id.starts_with("page-") => {
                Some((tool_call_id.as_str(), model_content.as_str()))
            }
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(PAGE_COUNT, original_pages.len());
    assert_eq!(PAGE_COUNT + 2, bodies.len());
    assert_eq!(bodies.len(), steps.len());
    assert!(segments.len() > 2);
    assert!(segments[1..].iter().all(|segment| {
        segment["reason"] == "request_byte_budget"
            && segment["output_pages_referenced"].as_u64().unwrap() > 0
    }));
    assert_eq!(
        steps[PAGE_COUNT]["segment"],
        steps[PAGE_COUNT + 1]["segment"]
    );
    assert_eq!(
        page_results(&bodies[PAGE_COUNT]),
        page_results(&bodies[PAGE_COUNT + 1])
    );
    let first_stat = bodies[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["tool_call_id"] == "stat-seed")
        .unwrap();
    assert!(bodies[PAGE_COUNT]["messages"]
        .as_array()
        .unwrap()
        .contains(first_stat));

    for (index, (body, step)) in bodies.iter().zip(&steps).enumerate() {
        let wire = serde_json::to_vec(body).unwrap();
        assert!(wire.len() <= WIRE_LIMIT);
        assert_eq!(wire.len(), step["request_bytes"].as_u64().unwrap() as usize);
        let pages = page_results(body);
        if let Some((id, payload)) = pages.last() {
            assert_eq!(original_pages[id], *payload, "latest page changed");
        }
        for (id, payload) in pages {
            if payload == original_pages[id] {
                continue;
            }
            let reference: Value = serde_json::from_str(payload).unwrap();
            let page_index = id.strip_prefix("page-").unwrap().parse::<usize>().unwrap();
            assert_eq!(reference["kind"], "retained_output_page_reference");
            assert_eq!(
                reference["reread"],
                json!({
                    "tool":"read_output", "arguments":{"id":saved.reference.id,
                    "offset":page_index * PAGE_BYTES,"limit":PAGE_BYTES}
                })
            );
        }
        if index > 0 && step["segment"] == steps[index - 1]["segment"] {
            assert!(body["messages"]
                .as_array()
                .unwrap()
                .starts_with(bodies[index - 1]["messages"].as_array().unwrap()));
        }
    }
}
