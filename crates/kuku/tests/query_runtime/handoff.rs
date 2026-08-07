use super::*;

const HANDOFF_INSTRUCTION_TAG: &[u8] = b"<kuku_handoff_instruction>";

fn handoff_instruction_count(request: &HttpMockRequest) -> usize {
    request.body.as_ref().map_or(0, |body| {
        body.windows(HANDOFF_INSTRUCTION_TAG.len())
            .filter(|window| *window == HANDOFF_INSTRUCTION_TAG)
            .count()
    })
}

fn request_has_no_handoff_instruction(request: &HttpMockRequest) -> bool {
    handoff_instruction_count(request) == 0
}

fn request_has_one_handoff_without_tool_result(request: &HttpMockRequest) -> bool {
    handoff_instruction_count(request) == 1
        && !request
            .body
            .as_ref()
            .is_some_and(|body| body.windows(11).any(|window| window == b"tool_result"))
}

fn request_has_one_handoff_with_tool_result(request: &HttpMockRequest) -> bool {
    handoff_instruction_count(request) == 1
        && request
            .body
            .as_ref()
            .is_some_and(|body| body.windows(11).any(|window| window == b"tool_result"))
}

fn history_contains_text(history: &[kuku::context::CanonicalMessage], expected: &str) -> bool {
    history.iter().any(|message| {
        message.blocks.iter().any(|block| {
            matches!(block, kuku::context::MessageBlock::Text(text) if text.contains(expected))
        })
    })
}

#[tokio::test(flavor = "current_thread")]
async fn threshold_handoff_injects_once_allows_tool_call_and_retains_configured_turns() {
    let env = TestEnv::new();
    std::fs::write(env.workspace.path().join("handoff.txt"), "visible\n").unwrap();
    let mut config = test_config();
    config.handoff.threshold = 0.7;
    config.handoff.keep_turns = 1;

    let prime_server = MockServer::start();
    let prime_mock = prime_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("prime handoff context")
            .matches(request_has_no_handoff_instruction);
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_prime",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Context primed."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 140000, "output_tokens": 5}
            })));
    });

    let session_id = "s_threshold_handoff_fixture";
    let first = query("prime handoff context")
        .session(session_id)
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(prime_server.base_url())
        .api_key("test-key")
        .config(config.clone())
        .run()
        .await
        .unwrap();
    assert_eq!(first.text, "Context primed.");
    prime_mock.assert_hits(1);

    let handoff_server = MockServer::start();
    let final_mock = handoff_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .matches(request_has_one_handoff_with_tool_result);
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_handoff",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "Handoff ready.\n\n<kuku_handoff>## Goal\nContinue safely.\n\n## Next Steps\nRead the retained turn.</kuku_handoff>"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 140500, "output_tokens": 20}
            })));
    });
    let tool_mock = handoff_server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/messages")
            .body_contains("trigger handoff with a tool")
            .matches(request_has_one_handoff_without_tool_result);
        then.status(200)
            .body(anthropic_sse_response(serde_json::json!({
                "id": "msg_after_threshold",
                "type": "message",
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Reading after the threshold."},
                    {"type": "tool_use", "id": "toolu_after_threshold", "name": "read_file", "input": {"path": "handoff.txt"}}
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 140250, "output_tokens": 8}
            })));
    });

    let second = query("trigger handoff with a tool")
        .session(session_id)
        .provider(Provider::Anthropic)
        .model("claude-sonnet-4-6")
        .base_url(handoff_server.base_url())
        .api_key("test-key")
        .config(config)
        .run()
        .await
        .unwrap();

    assert_eq!(second.text, "Handoff ready.");
    tool_mock.assert_hits(1);
    final_mock.assert_hits(1);

    let events = EventStore::replay(env.events_path(session_id)).unwrap();
    let tool_call_position = events
        .iter()
        .position(|event| {
            matches!(
                &event.payload,
                EventPayload::ToolCall { tool_call_id, .. }
                    if tool_call_id == "toolu_after_threshold"
            )
        })
        .expect("tool call after threshold");
    let handoffs = events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| match &event.payload {
            EventPayload::Handoff {
                summary,
                keep_turns,
                ..
            } => Some((index, summary.as_str(), *keep_turns)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(handoffs.len(), 1);
    assert!(tool_call_position < handoffs[0].0);
    assert_eq!(
        handoffs[0].1,
        "## Goal\nContinue safely.\n\n## Next Steps\nRead the retained turn."
    );
    assert_eq!(handoffs[0].2, 1);

    let (summary, history) = rebuild_history(&events, &ConversationAddress::MAIN);
    assert_eq!(summary.as_deref(), Some(handoffs[0].1));
    assert!(history_contains_text(
        &history,
        "trigger handoff with a tool"
    ));
    assert!(!history_contains_text(&history, "prime handoff context"));
}
