use std::collections::BTreeMap;
use std::time::Duration;

use super::{agent, common, facts, idle, options, response, HttpMockRequest, MockServer, POST};
use kuku::event::{EventPayload, EventStore};
use kuku::{AgentStatus, Input, NamedValueSource, OutputRead, Session};
use serde_json::{json, Value};

const WIRE_LIMIT: usize = 48 * 1024;
const PAGE_BYTES: usize = 8192;
const PAGE_COUNT: usize = 15;
const QUERY: &str = "Scan the saved output pages.";
const EARLIER: &str = "Previously answered request.";
const EARLIER_REPLY: &str = "Previously delivered response.";
const CHECKPOINT: &str = "## Delivered and verified\nThe earlier response was delivered.\n## Work in progress\nResume the scan if still open.";
const AFTER_REWIND: &str = "What remained before the scan?";
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
async fn provider_context_rejection_resumes_the_turn_without_repeating_a_write() {
    context_rejection(OverflowCase::Continue).await;
}

#[tokio::test]
async fn another_context_rejection_after_compaction_stops_the_turn() {
    context_rejection(OverflowCase::RejectedAgain).await;
}

#[tokio::test]
async fn context_rejection_keeps_budget_for_a_replacement_before_attempting_compaction() {
    context_rejection(OverflowCase::NoBudget).await;
}

#[tokio::test]
async fn cancelling_overflow_compaction_settles_its_card_before_the_agent_stops() {
    context_rejection(OverflowCase::Cancelled).await;
}

#[derive(Clone, Copy)]
enum OverflowCase {
    Continue,
    RejectedAgain,
    NoBudget,
    Cancelled,
}

async fn context_rejection(case: OverflowCase) {
    const PRIOR: &str = "OLD-UNCOMPRESSED-DETAIL";
    const CURRENT: &str = "Continue the current task.";
    const DONE: &str = "The current task resumed successfully.";
    fn has_write_result(body: &Value) -> bool {
        body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["role"] == "tool" && message["tool_call_id"] == "overflow-write")
    }
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let (mut config, path) = options(&server, workspace.path(), data.path());
    config.config.handoff.enabled = true;
    config.config.handoff.threshold = 0.98;
    config.limits.max_request_bytes = 64 * 1024;
    if matches!(case, OverflowCase::NoBudget) {
        config.limits.max_requests_per_open = Some(4);
    }
    let session = Session::open(config).await.unwrap();
    let mut agent = agent();
    agent.tools = Some(vec!["write_file".into()]);
    session.create_agent("create", "main", agent).unwrap();
    let initial = server
        .mock_async(|when, then| {
            when.method(POST).path(&path).matches(|req| {
                last_user(&request_body(req)).is_some_and(|text| text.starts_with(PRIOR))
            });
            then.status(200)
                .body(response(json!({"content": EARLIER_REPLY}), "stop"));
        })
        .await;
    let first_step = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(&path)
                .body_contains(PRIOR)
                .matches(|req| {
                    let body = request_body(req);
                    last_user(&body) == Some(CURRENT) && !has_write_result(&body)
                });
            let arguments =
                json!({"path":"result.txt","content":"written once","brief":"Write result"});
            then.status(200).body(response(
                json!({"tool_calls":[{"id":"overflow-write","type":"function",
                    "function":{"name":"write_file","arguments":arguments.to_string()}}
                ]}),
                "tool_calls",
            ));
        })
        .await;
    let context_error = json!({"error":{"code":"context_length_exceeded","message":"The deployed context window is smaller."}});
    let rejected = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(&path)
                .body_contains(PRIOR)
                .matches(|req| {
                    let body = request_body(req);
                    last_user(&body) == Some(CURRENT)
                        && has_write_result(&body)
                        && req.body.as_deref().unwrap().len() < 48 * 1024
                });
            then.status(400).json_body(context_error.clone());
        })
        .await;
    let summary = server
        .mock_async(|when, then| {
            when.method(POST).path(&path).matches(|req| {
                last_user(&request_body(req))
                    .is_some_and(|text| text.starts_with("Write a checkpoint"))
            });
            let delay = if matches!(case, OverflowCase::Cancelled) {
                Duration::from_secs(2)
            } else {
                Duration::ZERO
            };
            then.status(200)
                .delay(delay)
                .body(response(json!({"content": CHECKPOINT}), "stop"));
        })
        .await;
    let resumed = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(&path)
                .body_contains("<kuku_checkpoint>")
                .matches(|req| {
                    let body = request_body(req);
                    last_user(&body) == Some(CURRENT)
                        && has_write_result(&body)
                        && !body.to_string().contains(PRIOR)
                });
            if matches!(case, OverflowCase::RejectedAgain) {
                then.status(400).json_body(context_error.clone());
            } else {
                then.status(200)
                    .body(response(json!({"content": DONE}), "stop"));
            }
        })
        .await;
    session
        .send(Input::new(
            "earlier",
            "main",
            format!("{PRIOR} {}", "earlier investigation ".repeat(400)),
        ))
        .unwrap();
    idle(&session, "main").await;
    session
        .send(Input::new("continue", "main", CURRENT))
        .unwrap();
    if matches!(case, OverflowCase::Cancelled) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while summary.hits_async().await == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        session.cancel("main").await.unwrap();
    }
    let agent = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let snapshot = session.agent("main").unwrap();
            if snapshot.status != AgentStatus::Running {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let (expected_status, summaries, expected_replacement, missing_usage) = match case {
        OverflowCase::Continue => {
            assert_eq!(Some(DONE), agent.last_output.as_deref());
            (AgentStatus::Idle, 1, 1, 1)
        }
        OverflowCase::RejectedAgain | OverflowCase::NoBudget => {
            assert!(agent
                .last_error
                .as_deref()
                .unwrap()
                .contains("context_length_exceeded"));
            let replacements = usize::from(matches!(case, OverflowCase::RejectedAgain));
            (
                AgentStatus::Interrupted,
                replacements,
                replacements,
                1 + replacements as u64,
            )
        }
        OverflowCase::Cancelled => (AgentStatus::Stopped, 1, 0, 2),
    };
    assert_eq!(expected_status, agent.status, "{:?}", agent.last_error);
    assert_eq!(
        (3 + summaries + expected_replacement) as u64,
        agent.usage.model_requests
    );
    assert_eq!(missing_usage, agent.usage.unreported_requests);
    assert_eq!(
        "written once",
        std::fs::read_to_string(workspace.path().join("result.txt")).unwrap()
    );
    let events = EventStore::replay(session.events_path()).unwrap();
    let current = events
        .iter()
        .filter(|event| {
            matches!(&event.payload,
        EventPayload::MessageUser { text, .. } if text == CURRENT)
        })
        .count();
    let writes = events
        .iter()
        .filter(|event| {
            matches!(&event.payload,
        EventPayload::ToolCall { tool_call_id, .. } if tool_call_id == "overflow-write")
        })
        .count();
    assert_eq!((1, 1), (current, writes));
    let steps = facts(&events, "runtime.step_prepared", "main");
    assert_eq!(3 + expected_replacement, steps.len());
    let rejection = facts(&events, "runtime.model_rejected", "main");
    assert_eq!(1, rejection.len());
    assert_eq!(steps[2]["request_id"], rejection[0]["request_id"]);
    let compactions = facts(&events, "runtime.compaction_started", "main");
    assert_eq!(summaries, compactions.len());
    if matches!(case, OverflowCase::Cancelled) {
        let finished = facts(&events, "runtime.compaction_finished", "main");
        assert_eq!(1, finished.len());
        assert_eq!("cancelled", finished[0]["status"]);
        assert!(finished[0]["elapsed_ms"].as_u64().is_some());
    }
    if expected_replacement != 0 {
        assert_eq!(steps[2]["turn"], steps[3]["turn"]);
        assert_ne!(steps[2]["request_id"], steps[3]["request_id"]);
        assert_ne!(steps[2]["segment"], steps[3]["segment"]);
        assert!(
            steps[3]["request_bytes"].as_u64().unwrap()
                < steps[2]["request_bytes"].as_u64().unwrap()
        );
        assert_eq!(
            rejection[0]["request_id"],
            compactions[0]["rejected_request_id"]
        );
        assert_eq!(
            "completed",
            facts(&events, "runtime.compaction_finished", "main")[0]["status"]
        );
    }
    initial.assert_hits_async(1).await;
    first_step.assert_hits_async(1).await;
    rejected.assert_hits_async(1).await;
    summary.assert_hits_async(summaries).await;
    resumed.assert_hits_async(expected_replacement).await;
    session.close().await.unwrap();
}

#[tokio::test]
async fn compaction_preserves_recent_batches_and_rewind_restores_the_predecessor() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let (mut config, path) = options(&server, workspace.path(), data.path());
    config.config.handoff.enabled = true;
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

#[tokio::test]
async fn manual_compaction_only_summarizes_the_selected_agents_completed_work() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let (original, path) = options(&server, workspace.path(), data.path());
    let model = server
        .mock_async(|when, then| {
            when.method(POST).path(&path).matches(|request| {
                common::capture_http_request(request)
                    && !request
                        .body
                        .as_ref()
                        .unwrap()
                        .windows(b"Write a checkpoint".len())
                        .any(|window| window == b"Write a checkpoint")
            });
            then.status(200)
                .body(response(json!({"content":"answer"}), "stop"));
        })
        .await;
    let session = Session::open(original).await.unwrap();
    session
        .create_agent("create-main", "main", agent())
        .unwrap();
    session
        .create_agent("create-reviewer", "reviewer", agent())
        .unwrap();
    let large_request = "MAIN_OLD. ".repeat(420);
    for (command, target, text) in [
        ("main-first", "main", large_request.as_str()),
        ("review-first", "reviewer", "REVIEW_OLD"),
        ("main-second", "main", "MAIN_RECENT"),
    ] {
        session.send(Input::new(command, target, text)).unwrap();
        idle(&session, target).await;
    }

    let previous = session.agent("main").unwrap();
    let rejected = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(&path)
                .body_contains("Write a checkpoint for the same agent");
            then.status(502);
        })
        .await;
    let first_attempt = session.compact_command("compress", "main").unwrap();
    assert_eq!(
        first_attempt.event_id,
        session
            .compact_command("compress", "main")
            .unwrap()
            .event_id
    );
    idle(&session, "main").await;
    assert!(session
        .restore_context_command("undo-failed", "main")
        .is_err());
    rejected.assert_hits_async(1).await;
    rejected.delete_async().await;
    let summary = server.mock_async(|when, then| {
        when.method(POST).path(&path).body_contains("Write a checkpoint for the same agent");
        then.status(200).delay(Duration::from_millis(150)).body(response(
            json!({"content":"## Delivered and verified\nMAIN_SUMMARY, prior answer delivered.\n## Course corrections\nRecheck src/parser.rs at parse_range before editing."}),
            "stop",
        ));
    }).await;
    session.compact_command("retry", "main").unwrap();
    assert_eq!(AgentStatus::Running, session.agent("main").unwrap().status);
    idle(&session, "main").await;
    let current = session.agent("main").unwrap();
    assert_eq!(previous.turn, current.turn);
    assert_eq!(previous.last_output, current.last_output);
    assert_eq!(
        previous.usage.model_requests + 2,
        current.usage.model_requests
    );
    assert_eq!(None, current.last_error);

    session
        .send(Input::new("review-next", "reviewer", "REVIEW_NEXT"))
        .unwrap();
    idle(&session, "reviewer").await;
    session
        .send(Input::new("main-next", "main", "MAIN_NEXT"))
        .unwrap();
    idle(&session, "main").await;
    let undo = session
        .restore_context_command("undo-compaction", "main")
        .unwrap();
    assert_eq!(
        undo.event_id,
        session
            .restore_context_command("undo-compaction", "main")
            .unwrap()
            .event_id
    );
    session
        .send(Input::new("main-after-undo", "main", "MAIN_RESTORED"))
        .unwrap();
    idle(&session, "main").await;
    let events = EventStore::replay(session.events_path()).unwrap();
    session.close().await.unwrap();

    model.assert_hits_async(6).await;
    summary.assert_hits_async(1).await;
    let bodies = common::take_http_requests(&path)
        .into_iter()
        .filter(|body| !body.to_string().contains("Write a checkpoint"))
        .collect::<Vec<_>>();
    assert_eq!(6, bodies.len());
    assert!(bodies[3]["messages"]
        .as_array()
        .unwrap()
        .starts_with(bodies[1]["messages"].as_array().unwrap()));
    assert!(!bodies[3].to_string().contains("MAIN_SUMMARY"));
    let main_body = bodies[4].to_string();
    assert!(!main_body.contains("MAIN_OLD"));
    assert!(main_body.contains("MAIN_SUMMARY") && main_body.contains("MAIN_RECENT"));
    assert!(main_body.contains("src/parser.rs at parse_range"));
    assert_eq!(1, main_body.matches("<kuku_runtime_date>").count());
    let restored = bodies[5].to_string();
    assert!(restored.contains("MAIN_OLD") && restored.contains("MAIN_NEXT"));
    assert!(!restored.contains("MAIN_SUMMARY"));
    let compactions = facts(&events, "runtime.compaction_finished", "main");
    assert_eq!(compactions[0]["status"], "failed");
    assert!(compactions[0]["reason"]
        .as_str()
        .unwrap()
        .contains("Provider error"));
    assert_eq!(compactions[1]["status"], "completed");
    assert_eq!(
        1,
        facts(&events, "runtime.context_segment", "reviewer").len()
    );
    assert_eq!(2, facts(&events, "runtime.context_segment", "main").len());
    assert_eq!(
        6,
        events
            .iter()
            .filter(|event| matches!(&event.payload, EventPayload::MessageUser { .. }))
            .count()
    );
    assert_eq!(
        6,
        events
            .iter()
            .filter(|event| matches!(&event.payload, EventPayload::TurnStarted { .. }))
            .count()
    );
}

#[tokio::test]
async fn undo_after_reopen_restores_only_the_preceding_checkpoint() {
    const KEEP: &str = "KEEP_ORIGINAL_USER";
    const BEFORE_REOPEN: &str = "CHECKPOINT_B_BEFORE_REOPEN";
    const AFTER_REOPEN: &str = "CHECKPOINT_B_AFTER_REOPEN";
    const AFTER_UNDO: &str = "CHECKPOINT_A_AFTER_UNDO";
    const REWOUND: &str = "REWOUND_BEFORE_BOTH";
    const FIRST: &str = "## Delivered and verified\nCHECKPOINT_A_SUMMARY. Phase A was delivered.";
    const SECOND: &str =
        "## Delivered and verified\nCHECKPOINT_B_SUMMARY. Phases A and B were delivered.";

    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let (mut config, path) = options(&server, workspace.path(), data.path());
    config.limits.max_request_bytes = WIRE_LIMIT;
    let normal = server
        .mock_async(|when, then| {
            when.method(POST).path(&path).matches(|request| {
                let body = request_body(request);
                let last = last_user(&body).unwrap_or_default();
                let effect = last.starts_with("A_OLD") || last.starts_with("B_OLD");
                !last.starts_with("Write a checkpoint for the same agent")
                    && (!effect
                        || body["messages"].as_array().unwrap().last().unwrap()["role"] == "tool")
                    && common::capture_http_request(request)
            });
            then.status(200).body(response(
                json!({"content":"Verified and delivered."}),
                "stop",
            ));
        })
        .await;
    let mut writes = Vec::new();
    for (prompt, call, file) in [("A_OLD", "a-write", "a.txt"), ("B_OLD", "b-write", "b.txt")] {
        let matcher: fn(&HttpMockRequest) -> bool = if prompt == "A_OLD" {
            |request| first_request_for_effect(request, "A_OLD")
        } else {
            |request| first_request_for_effect(request, "B_OLD")
        };
        let result = json!({"tool_calls":[{"id":call,"type":"function","function":{
            "name":"write_file","arguments":json!({"path":file,"content":prompt}).to_string()
        }}]});
        writes.push(
            server
                .mock_async(|when, then| {
                    when.method(POST).path(&path).matches(matcher);
                    then.status(200).body(response(result, "tool_calls"));
                })
                .await,
        );
    }
    let mut summaries = Vec::new();
    for (has_predecessor, checkpoint) in [(false, FIRST), (true, SECOND)] {
        let matcher: fn(&HttpMockRequest) -> bool = if has_predecessor {
            |request| checkpoint_request_with_predecessor(request, true)
        } else {
            |request| checkpoint_request_with_predecessor(request, false)
        };
        summaries.push(
            server
                .mock_async(|when, then| {
                    when.method(POST).path(&path).matches(matcher);
                    then.status(200)
                        .body(response(json!({"content":checkpoint}), "stop"));
                })
                .await,
        );
    }

    let session = Session::open(config.clone()).await.unwrap();
    let mut selected = agent();
    selected.tools = Some(vec!["write_file".into()]);
    session.create_agent("create", "main", selected).unwrap();
    send_and_settle(&session, KEEP, KEEP).await;
    let mut target = String::new();
    for (phase, turns) in [
        ("a", ["A_OLD", "A_RECENT", "A_TAIL"]),
        ("b", ["B_OLD", "B_RECENT", "B_TAIL"]),
    ] {
        for turn in turns {
            let text = format!("{turn} {}", "evidence ".repeat(700));
            send_and_settle(&session, turn, &text).await;
            if turn == "A_OLD" {
                target = text;
            }
        }
        let before = session.agent("main").unwrap();
        session
            .compact_command(&format!("compact-{phase}"), "main")
            .unwrap();
        idle(&session, "main").await;
        let after = session.agent("main").unwrap();
        assert_eq!(None, after.last_error);
        assert_eq!(before.turn, after.turn);
        assert_eq!(before.last_output, after.last_output);
    }
    send_and_settle(&session, BEFORE_REOPEN, BEFORE_REOPEN).await;
    let journal = session.events_path().to_path_buf();
    config.session_id = Some(session.id().into());
    session.close().await.unwrap();

    let reopened = Session::open(config).await.unwrap();
    send_and_settle(&reopened, AFTER_REOPEN, AFTER_REOPEN).await;
    let undo = reopened
        .restore_context_command("undo-latest", "main")
        .unwrap();
    assert_eq!(
        undo.event_id,
        reopened
            .restore_context_command("undo-latest", "main")
            .unwrap()
            .event_id
    );
    send_and_settle(&reopened, AFTER_UNDO, AFTER_UNDO).await;
    let target_id = EventStore::replay(&journal).unwrap().into_iter().find(|event| {
        matches!(&event.payload, EventPayload::MessageUser { text, .. } if text == &target)
    }).unwrap().id;
    reopened
        .rewind_message("rewind-both", "main", target_id, &target, false)
        .unwrap();
    assert!(reopened
        .restore_context_command("undo-after-rewind", "main")
        .is_err());
    send_and_settle(&reopened, REWOUND, REWOUND).await;
    reopened.close().await.unwrap();

    let events = EventStore::replay(&journal).unwrap();
    let compactions = facts(&events, "runtime.compaction_finished", "main");
    assert_eq!(2, compactions.len());
    for (entry, expected) in compactions.iter().zip([FIRST, SECOND]) {
        assert_eq!("completed", entry["status"]);
        assert_eq!(expected, entry["summary"]);
        assert!(entry["after_bytes"].as_u64().unwrap() < entry["before_bytes"].as_u64().unwrap());
        assert!(entry["after_bytes"].as_u64().unwrap() < WIRE_LIMIT as u64);
    }
    for mock in [writes, summaries].into_iter().flatten() {
        mock.assert_hits_async(1).await;
    }
    normal.assert_hits_async(11).await;
    let bodies: Vec<_> = common::take_http_requests(&path)
        .into_iter()
        .filter(|body| {
            !last_user(body)
                .unwrap_or_default()
                .starts_with("Write a checkpoint for the same agent")
        })
        .collect();
    let steps = facts(&events, "runtime.step_prepared", "main");
    assert_eq!(13, bodies.len());
    assert_eq!(bodies.len(), steps.len());
    assert!(steps
        .iter()
        .all(|step| step["request_bytes"].as_u64().unwrap() <= WIRE_LIMIT as u64));
    let request = |text: &str| {
        let index = bodies
            .iter()
            .position(|body| last_user(body).is_some_and(|last| last.starts_with(text)))
            .unwrap();
        (&bodies[index], &steps[index]["segment"])
    };
    let message_prefix = |earlier: &Value, later: &Value| {
        assert!(later["messages"]
            .as_array()
            .unwrap()
            .starts_with(earlier["messages"].as_array().unwrap()));
    };
    let (before_reopen, active_b) = request(BEFORE_REOPEN);
    let (after_reopen, reopened_b) = request(AFTER_REOPEN);
    let (after_undo, restored_a) = request(AFTER_UNDO);
    let (rewound, restored_original) = request(REWOUND);
    assert_eq!(&compactions[1]["segment"], active_b);
    assert_eq!(active_b, reopened_b);
    message_prefix(before_reopen, after_reopen);
    let reopened_text = after_reopen.to_string();
    assert_eq!(1, reopened_text.matches("CHECKPOINT_B_SUMMARY").count());
    assert!(!reopened_text.contains("CHECKPOINT_A_SUMMARY"));
    assert_eq!(&compactions[0]["segment"], restored_a);
    message_prefix(request("B_TAIL").0, after_undo);
    let messages = after_undo["messages"].as_array().unwrap();
    let last_reopened_turn = messages
        .iter()
        .position(|entry| entry["role"] == "user" && entry["content"] == AFTER_REOPEN)
        .unwrap();
    assert_eq!("assistant", messages[last_reopened_turn + 1]["role"]);
    assert_eq!(
        "Verified and delivered.",
        messages[last_reopened_turn + 1]["content"]
    );
    let after_undo = after_undo.to_string();
    assert_eq!(1, after_undo.matches("CHECKPOINT_A_SUMMARY").count());
    assert!(!after_undo.contains("CHECKPOINT_B_SUMMARY"));
    assert_eq!(request(KEEP).1, restored_original);
    message_prefix(request(KEEP).0, rewound);
    let rewound = rewound.to_string();
    assert!(rewound.contains("Verified and delivered."));
    assert!(
        !rewound.contains("<kuku_checkpoint>")
            && !rewound.contains("A_OLD")
            && !rewound.contains("B_OLD")
    );
    for (id, path, text) in [("a-write", "a.txt", "A_OLD"), ("b-write", "b.txt", "B_OLD")] {
        assert_eq!(1, events.iter().filter(|event| {
            matches!(&event.payload, EventPayload::ToolResult { tool_call_id, status, .. } if tool_call_id == id && status == "ok")
        }).count());
        assert_eq!(
            text,
            std::fs::read_to_string(workspace.path().join(path)).unwrap()
        );
    }
}

fn first_request_for_effect(request: &HttpMockRequest, prompt: &str) -> bool {
    let body = request_body(request);
    last_user(&body).is_some_and(|text| text.starts_with(prompt))
        && body["messages"].as_array().unwrap().last().unwrap()["role"] == "user"
        && common::capture_http_request(request)
}

fn checkpoint_request_with_predecessor(request: &HttpMockRequest, expected: bool) -> bool {
    let body = request_body(request);
    last_user(&body).is_some_and(|text| text.starts_with("Write a checkpoint for the same agent"))
        && body.to_string().contains("CHECKPOINT_A_SUMMARY") == expected
        && common::capture_http_request(request)
}

async fn send_and_settle(session: &Session, command: &str, text: &str) {
    session.send(Input::new(command, "main", text)).unwrap();
    idle(session, "main").await;
}

#[tokio::test]
async fn reopening_records_a_terminal_outcome_for_an_abandoned_compaction() {
    let server = MockServer::start_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let (mut config, path) = options(&server, workspace.path(), data.path());
    let ordinary = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(&path)
                .body_contains("Already answered");
            then.status(200)
                .body(response(json!({"content":"Delivered reply"}), "stop"));
        })
        .await;
    let session = Session::open(config.clone()).await.unwrap();
    session.create_agent("create", "main", agent()).unwrap();
    session
        .send(Input::new("complete", "main", "Already answered"))
        .unwrap();
    idle(&session, "main").await;
    let before = session.agent("main").unwrap();
    let journal = session.events_path().to_path_buf();
    config.session_id = Some(session.id().into());
    session.close().await.unwrap();

    let mut journal_writer = EventStore::open(&journal).unwrap();
    let request = journal_writer
        .append(EventPayload::Unknown(json!({
            "kind":"runtime.operation", "record":{
                "operation":"compaction_requested", "address":"main", "turn":before.turn,
                "command":{"command_id":"abandoned","fingerprint":"fixture"},
            },
        })))
        .unwrap();
    let request_id = format!("compact_{}", request.id);
    journal_writer
        .append(EventPayload::Unknown(json!({
            "kind":"runtime.compaction_started", "agent":"main", "turn":before.turn,
            "request_id":request_id,
        })))
        .unwrap();
    drop(journal_writer);

    let reopened = Session::open(config).await.unwrap();
    let after = reopened.agent("main").unwrap();
    assert_eq!(AgentStatus::Idle, after.status);
    assert_eq!(before.turn, after.turn);
    assert_eq!(before.last_output, after.last_output);
    assert_eq!(
        before.usage.unreported_requests + 1,
        after.usage.unreported_requests
    );
    let events = EventStore::replay(&journal).unwrap();
    let finished = facts(&events, "runtime.compaction_finished", "main");
    assert_eq!(1, finished.len());
    assert_eq!(request_id, finished[0]["request_id"]);
    assert_eq!("interrupted", finished[0]["status"]);
    assert!(finished[0]["reason"].as_str().unwrap().contains("reopened"));
    assert!(reopened.restore_context_command("undo", "main").is_err());
    reopened.close().await.unwrap();
    ordinary.assert_hits_async(1).await;
}
