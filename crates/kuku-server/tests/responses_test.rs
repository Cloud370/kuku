mod common;

use common::mock_provider;
use common::stream::next_event_of_type;

#[tokio::test]
async fn invalid_interaction_id_returns_error() {
    let mock = mock_provider::start_mock_provider().await;
    let config = mock_provider::make_test_config(mock.port());
    let server = common::TestServer::start(config).await;

    let client = wreq::Client::new();
    let resp = client
        .post(format!("{}/runs/fake_run/responses", server.base_url))
        .json(&serde_json::json!({
            "interaction_id": "nonexistent",
            "choice": "once",
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], false);
}

#[tokio::test]
async fn invalid_choice_returns_error() {
    let mock = mock_provider::start_mock_provider().await;
    let config = mock_provider::make_test_config(mock.port());
    let server = common::TestServer::start(config).await;

    let client = wreq::Client::new();
    let resp = client
        .post(format!("{}/runs/fake_run/responses", server.base_url))
        .json(&serde_json::json!({
            "interaction_id": "any",
            "choice": "invalid_choice",
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], false);
    assert_eq!(body["code"], "invalid_request");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn response_with_wrong_run_id_does_not_resolve_permission() {
    let mock = mock_provider::start_mock_provider().await;
    mock.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages").matches(|request| {
            !String::from_utf8_lossy(request.body.as_deref().unwrap_or_default()).contains("\"tool_result\"")
        });
        then.status(200)
            .header("request-id", "req_tool")
            .header("connection", "close")
            .body(mock_provider::anthropic_sse_response(serde_json::json!({
                "id": "msg_tool",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Need approval."},
                    {"type": "tool_use", "id": "toolu_cmd", "name": "run_command", "input": {"command": "printf allowed", "timeout": 60, "brief": "print marker"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    mock.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains(r#""tool_result""#)
            .body_contains("toolu_cmd");
        then.status(200)
            .header("request-id", "req_final")
            .header("connection", "close")
            .body(mock_provider::anthropic_sse_response(serde_json::json!({
                "id": "msg_final",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Allowed."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let config = mock_provider::make_test_config(mock.port());
    let server = common::TestServer::start_deferred(config).await;
    let client = wreq::Client::new();

    let resp = client
        .post(format!("{}/runs", server.base_url))
        .json(&serde_json::json!({
            "prompt": "run command",
            "workspace": server.workspace.path().to_str().unwrap(),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let mut stream = resp.bytes_stream();
    let mut stream_buf = Vec::new();
    let run_start = next_event_of_type(&mut stream, &mut stream_buf, "run_start").await;
    let permission = next_event_of_type(&mut stream, &mut stream_buf, "permission").await;

    let wrong_resp = client
        .post(format!(
            "{}/runs/wrong-{}/responses",
            server.base_url,
            run_start["run_id"].as_str().unwrap()
        ))
        .json(&serde_json::json!({
            "interaction_id": permission["id"].as_str().unwrap(),
            "choice": "once",
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(wrong_resp.status(), 200);
    let body: serde_json::Value = wrong_resp.json().await.unwrap();
    assert_eq!(body["ok"], false);

    let correct_resp = client
        .post(format!(
            "{}/runs/{}/responses",
            server.base_url,
            run_start["run_id"].as_str().unwrap()
        ))
        .json(&serde_json::json!({
            "interaction_id": permission["id"].as_str().unwrap(),
            "choice": "once",
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(correct_resp.status(), 200);
    let body: serde_json::Value = correct_resp.json().await.unwrap();
    assert_eq!(body["ok"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn permission_response_preserves_conversation_identity() {
    let mock = mock_provider::start_mock_provider().await;
    mock.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages").matches(|request| {
            !String::from_utf8_lossy(request.body.as_deref().unwrap_or_default()).contains("\"tool_result\"")
        });
        then.status(200)
            .header("request-id", "req_tool_review")
            .header("connection", "close")
            .body(mock_provider::anthropic_sse_response(serde_json::json!({
                "id": "msg_tool_review",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Need approval."},
                    {"type": "tool_use", "id": "toolu_review", "name": "run_command", "input": {"command": "printf review", "timeout": 60, "brief": "print review marker"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });
    mock.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains(r#""tool_result""#)
            .body_contains("toolu_review");
        then.status(200)
            .header("request-id", "req_final_review")
            .header("connection", "close")
            .body(mock_provider::anthropic_sse_response(serde_json::json!({
                "id": "msg_final_review",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Allowed in review."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 6}
            })));
    });

    let config = mock_provider::make_test_config(mock.port());
    let server = common::TestServer::start_deferred(config).await;
    let client = wreq::Client::new();

    let resp = client
        .post(format!("{}/runs", server.base_url))
        .json(&serde_json::json!({
            "prompt": "review command",
            "workspace": server.workspace.path().to_str().unwrap(),
            "conversation": "review",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let mut stream = resp.bytes_stream();
    let mut stream_buf = Vec::new();
    let run_start = next_event_of_type(&mut stream, &mut stream_buf, "run_start").await;
    let permission = next_event_of_type(&mut stream, &mut stream_buf, "permission").await;
    assert_eq!(permission["conversation"], "review");

    let correct_resp = client
        .post(format!(
            "{}/runs/{}/responses",
            server.base_url,
            run_start["run_id"].as_str().unwrap()
        ))
        .json(&serde_json::json!({
            "interaction_id": permission["id"].as_str().unwrap(),
            "conversation": "review",
            "choice": "once",
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(correct_resp.status(), 200);
    let body: serde_json::Value = correct_resp.json().await.unwrap();
    assert_eq!(body["ok"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn persistent_question_reply_reopens_new_http_app_and_continues_once() {
    let mock = mock_provider::start_mock_provider().await;
    let question = mock.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages").matches(|request| {
            !String::from_utf8_lossy(request.body.as_deref().unwrap_or_default()).contains("answer-is-Rhea")
        });
        then.status(200).body(mock_provider::anthropic_sse_response(serde_json::json!({
            "id":"ask-reopen","type":"message","role":"assistant",
            "content":[{"type":"tool_use","id":"question-reopen","name":"ask_user","input":{"question":"Which name?"}}],
            "stop_reason":"tool_use","usage":{"input_tokens":5,"output_tokens":6}
        })));
    });
    let resumed = mock.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages").matches(|request| {
            let body = String::from_utf8_lossy(request.body.as_deref().unwrap_or_default());
            body.contains("answer-is-Rhea") && !body.contains("write-reopened")
        });
        then.status(200).body(mock_provider::anthropic_sse_response(serde_json::json!({
            "id":"continue-reopen","type":"message","role":"assistant",
            "content":[{"type":"tool_use","id":"write-reopened","name":"write_file","input":{"path":"reopened.txt","content":"Rhea\n","brief":"persist answer"}}],
            "stop_reason":"tool_use","usage":{"input_tokens":8,"output_tokens":7}
        })));
    });
    let completed = mock.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages").body_contains("write-reopened").body_contains(r#""tool_result""#);
        then.status(200).body(mock_provider::anthropic_sse_response(serde_json::json!({
            "id":"done-reopen","type":"message","role":"assistant","content":[{"type":"text","text":"Reopened work completed."}],
            "stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":4}
        })));
    });
    let mut server = common::TestServer::start(mock_provider::make_test_config(mock.port())).await;
    let client = wreq::Client::new();
    let response = client
        .post(format!("{}/runs", server.base_url))
        .json(&serde_json::json!({
            "prompt":"ask for a name","workspace":server.workspace.path(),"agent":"reviewer",
        }))
        .send()
        .await
        .unwrap();
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    let start = next_event_of_type(&mut stream, &mut buffer, "run_start").await;
    let pending = next_event_of_type(&mut stream, &mut buffer, "question").await;
    drop(stream);
    server.restart().await;
    assert!(server
        .state
        .run_manager
        .lock()
        .await
        .active_run_ids()
        .is_empty());
    let payload = serde_json::json!({
        "workspace":server.workspace.path(),"agent":"reviewer","interaction_id":pending["id"],
        "choice":"answer","text":"answer-is-Rhea","command_id":"answer-after-restart",
    });
    let url = format!(
        "{}/sessions/{}/responses",
        server.base_url,
        start["session_id"].as_str().unwrap()
    );
    let response = client.post(&url).json(&payload).send().await.unwrap();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|header| header.to_str().ok())
        .map(str::to_string);
    let body = response.text().await.unwrap();
    assert_eq!(
        Some("application/x-ndjson"),
        content_type.as_deref(),
        "{body}"
    );
    let events = body
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!("done", events.last().unwrap()["type"]);
    assert_eq!("Reopened work completed.", events.last().unwrap()["text"]);
    assert_eq!(
        "Rhea\n",
        std::fs::read_to_string(server.workspace.path().join("reopened.txt")).unwrap()
    );
    let receipt = events[0]["receipt"].clone();
    let retry: serde_json::Value = client
        .post(&url)
        .json(&payload)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(true, retry["replayed"]);
    assert_eq!(receipt, retry["receipt"]);
    let mut conflicting = payload;
    conflicting["text"] = serde_json::json!("different answer");
    let rejected: serde_json::Value = client
        .post(&url)
        .json(&conflicting)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(false, rejected["ok"]);
    conflicting["command_id"] = serde_json::json!("stale-new-reply");
    let stale: serde_json::Value = client
        .post(&url)
        .json(&conflicting)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(false, stale["ok"]);
    question.assert_hits(1);
    resumed.assert_hits(1);
    completed.assert_hits(1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn persistent_permission_reply_resumes_bound_action_after_server_restart() {
    let mock = mock_provider::start_mock_provider().await;
    let requested = mock.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages").matches(|request| {
            !String::from_utf8_lossy(request.body.as_deref().unwrap_or_default()).contains("\"tool_result\"")
        });
        then.status(200).body(mock_provider::anthropic_sse_response(serde_json::json!({
            "id":"defer-reopen","type":"message","role":"assistant",
            "content":[{"type":"tool_use","id":"write-before-defer","name":"write_file","input":{"path":"authorized.txt","content":"authorized once\n","brief":"write after approval"}}],
            "stop_reason":"tool_use","usage":{"input_tokens":5,"output_tokens":6}
        })));
    });
    let completed = mock.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages").body_contains(r#""tool_result""#).body_contains("write-before-defer");
        then.status(200).body(mock_provider::anthropic_sse_response(serde_json::json!({
            "id":"authorized-reopen","type":"message","role":"assistant","content":[{"type":"text","text":"Authorization resumed."}],
            "stop_reason":"end_turn","usage":{"input_tokens":8,"output_tokens":4}
        })));
    });
    let mut server =
        common::TestServer::start_deferred(mock_provider::make_test_config(mock.port())).await;
    let client = wreq::Client::new();
    let response = client
        .post(format!("{}/runs", server.base_url))
        .json(&serde_json::json!({
            "prompt":"write after approval","workspace":server.workspace.path(),
        }))
        .send()
        .await
        .unwrap();
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    let start = next_event_of_type(&mut stream, &mut buffer, "run_start").await;
    let pending = next_event_of_type(&mut stream, &mut buffer, "permission").await;
    assert!(!server.workspace.path().join("authorized.txt").exists());
    drop(stream);
    server.restart().await;
    let response = client.post(format!("{}/sessions/{}/responses",server.base_url,start["session_id"].as_str().unwrap())).json(&serde_json::json!({
        "workspace":server.workspace.path(),"agent":"main","interaction_id":pending["id"],"choice":"once","command_id":"allow-after-restart",
    })).send().await.unwrap().text().await.unwrap();
    let events = response
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!("done", events.last().unwrap()["type"], "{response}");
    assert_eq!(
        "authorized once\n",
        std::fs::read_to_string(server.workspace.path().join("authorized.txt")).unwrap()
    );
    requested.assert_hits(1);
    completed.assert_hits(1);
}

#[derive(Debug)]
struct DeferCommands;

impl kuku::permission::ToolAuthorizer for DeferCommands {
    fn authorize(
        &self,
        action: &kuku::permission::ToolAuthorization,
    ) -> kuku::Result<kuku::permission::AuthorizationDecision> {
        Ok(if action.tool.name == "run_command" {
            kuku::permission::AuthorizationDecision::Defer
        } else {
            kuku::permission::AuthorizationDecision::Allow
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simultaneous_question_and_authorization_emit_once_and_resume_separately() {
    let mock = mock_provider::start_mock_provider().await;
    let first=mock.mock(|when,then| {
        when.method(httpmock::Method::POST).path("/v1/messages").matches(|request| {
            let body=String::from_utf8_lossy(request.body.as_deref().unwrap_or_default());
            body.contains("dual wait") && !body.contains("\"tool_result\"")
        });
        then.status(200).body(mock_provider::anthropic_sse_response(serde_json::json!({
            "id":"dual","type":"message","role":"assistant","content":[
                {"type":"tool_use","id":"ask-dual","name":"ask_user","input":{"question":"Which value?"}},
                {"type":"tool_use","id":"command-dual","name":"run_command","input":{"command":"git init dual-repository","timeout":10,"brief":"initialize only after authorization"}}
            ],"stop_reason":"tool_use","usage":{"input_tokens":5,"output_tokens":8}
        })));
    });
    let probe=mock.mock(|when,then| {
        when.method(httpmock::Method::POST).path("/v1/messages").body_contains("probe input");
        then.status(200).body(mock_provider::anthropic_sse_response(serde_json::json!({
            "id":"probe","type":"message","role":"assistant","content":[{"type":"text","text":"probe complete"}],
            "stop_reason":"end_turn","usage":{"input_tokens":5,"output_tokens":3}
        })));
    });
    let final_response=mock.mock(|when,then| {
        when.method(httpmock::Method::POST).path("/v1/messages").body_contains("dual-answer").body_contains("command-dual").body_contains(r#""tool_result""#);
        then.status(200).body(mock_provider::anthropic_sse_response(serde_json::json!({
            "id":"dual-done","type":"message","role":"assistant","content":[{"type":"text","text":"both waits settled"}],
            "stop_reason":"end_turn","usage":{"input_tokens":12,"output_tokens":4}
        })));
    });
    let server = common::TestServer::start_with_authorizer(
        mock_provider::make_test_config(mock.port()),
        None,
        std::sync::Arc::new(DeferCommands),
    )
    .await;
    let client = wreq::Client::new();
    let response = client
        .post(format!("{}/runs", server.base_url))
        .json(&serde_json::json!({
            "prompt":"dual wait","workspace":server.workspace.path(),
        }))
        .send()
        .await
        .unwrap();
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    let start = next_event_of_type(&mut stream, &mut buffer, "run_start").await;
    let mut question = None;
    let mut permission = None;
    let mut question_count = 0;
    let mut permission_count = 0;
    while question.is_none() || permission.is_none() {
        let event =
            common::stream::next_json_line(&mut stream, &mut buffer, "initial simultaneous waits")
                .await;
        match event["type"].as_str() {
            Some("question") => {
                question_count += 1;
                question = Some(event["id"].clone());
            }
            Some("permission") => {
                permission_count += 1;
                permission = Some(event["id"].clone());
            }
            Some("error") => panic!("failed before both waits: {event}"),
            _ => {}
        }
    }
    let response=client.post(format!("{}/runs",server.base_url)).json(&serde_json::json!({
        "prompt":"probe input","workspace":server.workspace.path(),"session_id":start["session_id"],"agent":"probe"
    })).send().await.unwrap().text().await.unwrap();
    assert!(response
        .lines()
        .any(|line| serde_json::from_str::<serde_json::Value>(line).unwrap()["type"] == "done"));
    let url = format!(
        "{}/sessions/{}/responses",
        server.base_url,
        start["session_id"].as_str().unwrap()
    );
    let answered:serde_json::Value=client.post(&url).json(&serde_json::json!({
        "workspace":server.workspace.path(),"interaction_id":question.unwrap(),"choice":"answer","text":"dual-answer","command_id":"dual-answer-command"
    })).send().await.unwrap().json().await.unwrap();
    assert_eq!(true, answered["ok"]);
    assert!(!server
        .workspace
        .path()
        .join("dual-repository")
        .join(".git")
        .exists());
    final_response.assert_hits(0);
    let allowed:serde_json::Value=client.post(&url).json(&serde_json::json!({
        "workspace":server.workspace.path(),"interaction_id":permission.unwrap(),"choice":"once","command_id":"dual-allow-command"
    })).send().await.unwrap().json().await.unwrap();
    assert_eq!(true, allowed["ok"]);
    loop {
        let event =
            common::stream::next_json_line(&mut stream, &mut buffer, "settling simultaneous waits")
                .await;
        match event["type"].as_str() {
            Some("question") => question_count += 1,
            Some("permission") => permission_count += 1,
            Some("done") => {
                assert_eq!("both waits settled", event["text"]);
                break;
            }
            Some("error") => panic!("failed after both replies: {event}"),
            _ => {}
        }
    }
    assert_eq!(1, question_count);
    assert_eq!(1, permission_count);
    assert!(server
        .workspace
        .path()
        .join("dual-repository")
        .join(".git")
        .is_dir());
    first.assert_hits(1);
    probe.assert_hits(1);
    final_response.assert_hits(1);
}

#[tokio::test(flavor = "current_thread")]
async fn persistent_reply_retry_does_not_activate_an_unadmitted_answer() {
    let mock = mock_provider::start_mock_provider().await;
    let asked=mock.mock(|when,then| {
        when.method(httpmock::Method::POST).path("/v1/messages").matches(|request| {
            !String::from_utf8_lossy(request.body.as_deref().unwrap_or_default()).contains("\"tool_result\"")
        });
        then.status(200).body(mock_provider::anthropic_sse_response(serde_json::json!({
            "id":"ask-unadmitted","type":"message","role":"assistant","content":[{"type":"tool_use","id":"pending-question","name":"ask_user","input":{"question":"Which answer?"}}],
            "stop_reason":"tool_use","usage":{"input_tokens":5,"output_tokens":6}
        })));
    });
    let continued=mock.mock(|when,then| {
        when.method(httpmock::Method::POST).path("/v1/messages").body_contains("persisted-answer").body_contains(r#""tool_result""#);
        then.status(200).body(mock_provider::anthropic_sse_response(serde_json::json!({
            "id":"continued-unadmitted","type":"message","role":"assistant","content":[{"type":"text","text":"answer advanced explicitly"}],
            "stop_reason":"end_turn","usage":{"input_tokens":8,"output_tokens":4}
        })));
    });
    let config = mock_provider::make_test_config(mock.port());
    let server = common::TestServer::start(config.clone()).await;
    let mut options =
        kuku::SessionOptions::new(server.workspace.path(), server.home.path(), config);
    options.session_id = Some("unadmitted-answer".into());
    let session = kuku::Session::open(options).await.unwrap();
    session
        .create_agent("create", "main", kuku::AgentConfig::default())
        .unwrap();
    let mut observer = session.observe(0);
    session
        .send(kuku::Input::new("initial", "main", "ask for an answer"))
        .unwrap();
    let question = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Some(question) = session.agent("main").unwrap().question {
                if session.agent("main").unwrap().status == kuku::AgentStatus::Waiting {
                    break question;
                }
            }
            observer.next().await.unwrap();
        }
    })
    .await
    .unwrap();
    drop(observer);
    let receipt = session
        .reply_question("unadmitted-reply", "main", &question.id, "persisted-answer")
        .unwrap();
    session.close().await.unwrap();
    // NOTE: Keep the reply barrier while discarding later cancellation to model the activation gap.
    server.retain_receipt_prefix("unadmitted-answer", &receipt);
    let client = wreq::Client::new();
    let url = format!("{}/sessions/unadmitted-answer/responses", server.base_url);
    let payload = serde_json::json!({"workspace":server.workspace.path(),"interaction_id":question.id,"choice":"answer","text":"persisted-answer","command_id":"unadmitted-reply"});
    let duplicate: serde_json::Value = client
        .post(&url)
        .json(&payload)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(true, duplicate["replayed"]);
    assert_eq!(serde_json::to_value(receipt).unwrap(), duplicate["receipt"]);
    let path = kuku::session::session_events_path(
        server.home.path(),
        server.workspace.path(),
        "unadmitted-answer",
    )
    .unwrap();
    let agent = kuku::Session::read_agents(&path).unwrap().pop().unwrap();
    assert_eq!(kuku::AgentStatus::Idle, agent.status);
    assert_eq!(1, agent.pending_inputs);
    asked.assert_hits(1);
    continued.assert_hits(0);
    let completed=client.post(format!("{}/runs",server.base_url)).json(&serde_json::json!({
        "workspace":server.workspace.path(),"session_id":"unadmitted-answer","prompt":"advance the persisted answer","command_id":"fresh-continuation"
    })).send().await.unwrap().text().await.unwrap();
    assert!(
        completed
            .lines()
            .any(|line| serde_json::from_str::<serde_json::Value>(line).unwrap()["type"] == "done"),
        "{completed}"
    );
    assert!(
        continued.hits() > 0,
        "fresh input should advance the retained answer"
    );
}
