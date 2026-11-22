use std::collections::BTreeMap;

use super::{
    agent, common, facts, idle, last_user, options, request_body, response, MockServer, CHECKPOINT,
    EARLIER_REPLY, POST, WIRE_LIMIT,
};
use kuku::event::{EventPayload, EventStore};
use kuku::{AgentStatus, Input, NamedValueSource, OutputRead, Session};
use serde_json::{json, Value};

const PAGE_BYTES: usize = 8192;
const PAGE_COUNT: usize = 15;
const QUERY: &str = "Scan the saved output pages.";
const EARLIER: &str = "Previously answered request.";
const AFTER_REWIND: &str = "What remained before the scan?";
const FOLLOW_UP: &str = "Continue using the recorded references.";

#[tokio::test]
async fn a_large_request_with_token_headroom_only_obeys_an_explicit_byte_limit() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let (mut config, path) = options(&server, workspace.path(), data.path());
    config.config.handoff.enabled = true;
    config
        .config
        .tiers
        .get_mut("balanced")
        .unwrap()
        .context_window = 1_048_576;
    config.limits.max_input_bytes = 256 * 1024;
    let text = "\\".repeat(config.limits.max_input_bytes);
    let provider = server
        .mock_async(|when, then| {
            when.method(POST).path(&path).matches(|request| {
                request.body.as_ref().unwrap().len() > 512 * 1024
                    && last_user(&request_body(request))
                        .is_some_and(|text| !text.contains("Write a checkpoint"))
            });
            then.status(200)
                .body(response(json!({"content":"received"}), "stop"));
        })
        .await;

    let session = Session::open(config.clone()).await.unwrap();
    session.create_agent("create", "main", agent()).unwrap();
    session.send(Input::new("large", "main", &text)).unwrap();
    idle(&session, "main").await;
    assert_eq!(
        session.agent("main").unwrap().last_output.as_deref(),
        Some("received")
    );
    session.close().await.unwrap();

    config.limits.max_request_bytes = Some(512 * 1024);
    let limited = Session::open(config).await.unwrap();
    limited.create_agent("create", "main", agent()).unwrap();
    limited.send(Input::new("large", "main", text)).unwrap();
    let interrupted = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let state = limited.agent("main").unwrap();
            if state.status != AgentStatus::Running {
                break state;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(interrupted.status, AgentStatus::Interrupted);
    assert!(interrupted
        .last_error
        .unwrap()
        .contains("configured byte limit"));
    limited.close().await.unwrap();
    provider.assert_hits_async(1).await;
}

#[tokio::test]
async fn checkpoint_can_use_model_output_limit_and_store_over_sixteen_kib() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let (mut config, path) = options(&server, workspace.path(), data.path());
    let tier = config.config.tiers.get_mut("balanced").unwrap();
    tier.context_window = 200_000;
    tier.max_output_tokens = Some(32_768);

    let checkpoint = "# Completed work\nVerified facts and retained pointers.\n".repeat(330);
    assert!(checkpoint.len() > 16 * 1024);
    let summary = server
        .mock_async(|when, then| {
            when.method(POST).path(&path).matches(|request| {
                let body = request_body(request);
                last_user(&body).is_some_and(|text| text.starts_with("Write a checkpoint"))
                    && body
                        .get("max_tokens")
                        .or_else(|| body.get("max_completion_tokens"))
                        .and_then(serde_json::Value::as_u64)
                        == Some(32_768)
            });
            then.status(200)
                .body(response(json!({"content":checkpoint}), "stop"));
        })
        .await;
    let main = server
        .mock_async(|when, then| {
            when.method(POST).path(&path).matches(|request| {
                last_user(&request_body(request))
                    .is_some_and(|text| !text.starts_with("Write a checkpoint"))
            });
            then.status(200)
                .body(response(json!({"content":"done"}), "stop"));
        })
        .await;
    let session = Session::open(config).await.unwrap();
    session.create_agent("create", "main", agent()).unwrap();
    session
        .send(Input::new("old", "main", "Retain this fact. ".repeat(3200)))
        .unwrap();
    idle(&session, "main").await;
    session
        .send(Input::new("recent", "main", "Recent work"))
        .unwrap();
    idle(&session, "main").await;
    session.compact_command("checkpoint", "main").unwrap();
    idle(&session, "main").await;

    let events = kuku::event::EventStore::replay(session.events_path()).unwrap();
    let compaction = facts(&events, "runtime.compaction_finished", "main");
    assert_eq!(compaction.len(), 1);
    assert_eq!("completed", compaction[0]["status"]);
    assert_eq!("end_turn", compaction[0]["stop_reason"]);
    assert_eq!(Some(checkpoint.trim()), compaction[0]["summary"].as_str());
    session.close().await.unwrap();
    main.assert_hits_async(2).await;
    summary.assert_hits_async(1).await;
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
async fn compaction_preserves_recent_batches_and_rewind_restores_the_predecessor() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let (mut config, path) = options(&server, workspace.path(), data.path());
    config.config.handoff.enabled = true;
    config
        .config
        .tiers
        .get_mut("balanced")
        .unwrap()
        .context_window = 11_000;
    config.limits.max_request_bytes = Some(WIRE_LIMIT);
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

    let earlier = server
        .mock_async(|when, then| {
            when.method(POST).path(&path).matches(|request| {
                last_user(&request_body(request)) == Some(EARLIER)
                    && common::capture_http_request(request)
            });
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(response(json!({"content":EARLIER_REPLY}), "stop"));
        })
        .await;
    let summaries = server
        .mock_async(|when, then| {
            when.method(POST).path(&path).matches(|request| {
                last_user(&request_body(request)).is_some_and(|prompt| {
                    prompt.starts_with("Write a checkpoint for the same agent")
                        && prompt.contains("## Course corrections")
                }) && common::capture_http_request(request)
            });
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(response(json!({"content":CHECKPOINT}), "stop"));
        })
        .await;
    session
        .send(Input::new("earlier", "main", EARLIER))
        .unwrap();
    idle(&session, "main").await;

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
                        1 => when.matches(|request| {
                            request_body(request)["messages"]
                                .as_array()
                                .unwrap()
                                .last()
                                .unwrap()["tool_call_id"]
                                == "stat-seed"
                        }),
                        _ => {
                            when.body_contains(format!(r#""tool_call_id":"page-{}"}}]"#, index - 1))
                        }
                    };
                    then.status(200)
                        .header("content-type", "text/event-stream")
                        .body(common::openai_sse_response(json!({
                            "id":"context-fixture",
                            "choices":[{"message":message,"finish_reason":reason}]
                        })));
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
    let after_rewind = server
        .mock_async(|when, then| {
            when.method(POST).path(&path).matches(|request| {
                last_user(&request_body(request)) == Some(AFTER_REWIND)
                    && common::capture_http_request(request)
            });
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(response(json!({"content":"History restored."}), "stop"));
        })
        .await;
    session.send(Input::new("scan", "main", QUERY)).unwrap();
    let fallback = server
        .mock_async(|when, then| {
            when.method(POST).path(&path).matches(|request| {
                let body = request_body(request);
                let messages = body["messages"].as_array().unwrap();
                eprintln!(
                    "unmatched user {:?}; last tool ID {:?}; bytes {}",
                    last_user(&body).map(|s| s.chars().take(100).collect::<String>()),
                    messages.last().map(|m| m["tool_call_id"].clone()),
                    request.body.as_deref().unwrap().len()
                );
                true
            });
            then.status(422);
        })
        .await;
    idle(&session, "main").await;
    fallback.assert_hits_async(0).await;
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
    let target = EventStore::replay(&journal).unwrap().into_iter().find(|event| {
        matches!(&event.payload, EventPayload::MessageUser { text, .. } if text == QUERY)
    }).unwrap();
    reopened
        .rewind_message("rewind", "main", target.id, QUERY, false)
        .unwrap();
    reopened
        .send(Input::new("after-rewind", "main", AFTER_REWIND))
        .unwrap();
    idle(&reopened, "main").await;
    reopened.close().await.unwrap();
    for reply in replies {
        reply.assert_hits_async(1).await;
    }
    follow_up.assert_hits_async(1).await;

    earlier.assert_hits_async(1).await;
    after_rewind.assert_hits_async(1).await;

    let bodies = common::take_http_requests(&path);
    let events = EventStore::replay(&journal).unwrap();
    let segments = facts(&events, "runtime.context_segment", "main");
    let steps = facts(&events, "runtime.step_prepared", "main");
    let starts = facts(&events, "runtime.compaction_started", "main");
    let completions = facts(&events, "runtime.compaction_finished", "main");
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
    assert!(!starts.is_empty());
    assert_eq!(starts.len(), completions.len());
    summaries.assert_hits_async(starts.len()).await;
    assert_eq!(PAGE_COUNT + 4, steps.len());
    assert_eq!(bodies.len(), steps.len() + starts.len());
    assert_eq!(segments.len(), 1 + completions.len());

    for (started, finished) in starts.iter().zip(&completions) {
        assert_eq!(started["request_id"], finished["request_id"]);
        assert_eq!(finished["status"], "completed");
        assert_eq!(finished["summary"], CHECKPOINT);
        assert!(
            finished["before_bytes"].as_u64().unwrap() > finished["after_bytes"].as_u64().unwrap()
        );
        assert!(finished["checkpointed_messages"].as_u64().unwrap() > 0);
    }

    let mut task_bodies = Vec::new();
    for body in &bodies {
        assert!(serde_json::to_vec(body).unwrap().len() <= WIRE_LIMIT);
        if last_user(body).is_some_and(|prompt| prompt.starts_with("Write a checkpoint")) {
            continue;
        }
        task_bodies.push(body);
    }
    for (index, (body, step)) in task_bodies.iter().zip(&steps).enumerate() {
        assert_eq!(
            serde_json::to_vec(body).unwrap().len(),
            step["request_bytes"].as_u64().unwrap() as usize
        );
        for (id, payload) in page_results(body) {
            assert_eq!(original_pages[id], payload, "a retained page changed");
        }
        if index > 0 && index + 1 != steps.len() && step["segment"] == steps[index - 1]["segment"] {
            assert!(body["messages"]
                .as_array()
                .unwrap()
                .starts_with(task_bodies[index - 1]["messages"].as_array().unwrap()));
        }
    }
    let resumed = task_bodies[task_bodies.len() - 2].to_string();
    assert!(resumed.contains("The earlier response was delivered."));
    assert!(!resumed.contains(EARLIER));
    let restored = task_bodies.last().unwrap().to_string();
    assert!(restored.contains(EARLIER) && restored.contains(EARLIER_REPLY));
    assert!(!restored.contains("<kuku_checkpoint>") && !restored.contains(QUERY));
    assert_eq!(steps[0]["segment"], steps.last().unwrap()["segment"]);
}
