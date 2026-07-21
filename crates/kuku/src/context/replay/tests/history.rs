#[test]
fn rebuild_history_does_not_replay_prompt_snapshot_as_conversation_history() {
    let events = vec![
        event(
            1,
            EventPayload::PromptSnapshot {
                ts: "2026-05-13T00:00:00Z".to_string(),
                conversation: "main".to_string(),
                binding_id: "main".to_string(),
                snapshot_id: "snapshot_1".to_string(),
                turn: 1,
                messages: vec![crate::event::types::ContextMessage {
                    role: "user".to_string(),
                    content: "<kuku_tool_guidance>stable prelude</kuku_tool_guidance>".to_string(),
                }],
                project_instruction_sources: Vec::new(),
                memory_sources: Vec::new(),
                prompt_asset_sources: Vec::new(),
                skills: json!(null),
                bootstrap_loaded: Vec::new(),
                provider: "anthropic".to_string(),
                model: "model".to_string(),
                renderer: crate::context::PromptRendererIdentity {
                    provider: "anthropic".to_string(),
                    renderer: "anthropic".to_string(),
                },
                tool_registry: Box::new(crate::context::ToolRegistryProvenance {
                    hash: "count:0".to_string(),
                    names: Vec::new(),
                    tool_count: 0,
                }),
                agent_registry: None,
                skill_registry: Box::new(None),
                plugin_registry: Box::new(None),
                capabilities: crate::context::PromptCapabilityMetadata {
                    max_context_tokens: Some(200000),
                    remaining_input_tokens: None,
                    context_budget_tier: "normal".to_string(),
                },
            },
        ),
        user_input(2, 1, "hello"),
        model_response(3, 1, "req_1", "hi"),
        turn_end(4, 1),
    ];

    let (summary, history) = rebuild_history(&events, &ConversationAddress::MAIN);

    assert!(summary.is_none());
    assert_eq!(
        history,
        vec![
            CanonicalMessage::user_text("hello"),
            CanonicalMessage::assistant(vec![MessageBlock::Text("hi".to_string())]),
        ]
    );
}
#[test]
fn preserves_stream_order_across_multiple_response_groups() {
    let events = vec![
        user_input(1, 1, "first"),
        model_response(2, 1, "req_2", "first answer"),
        turn_end(3, 1),
        user_input(4, 2, "second"),
        model_response(5, 2, "req_10", "second answer"),
        turn_end(6, 2),
    ];

    let (summary, history) = rebuild_history(&events, &ConversationAddress::MAIN);
    assert!(summary.is_none());
    assert_eq!(
        history,
        vec![
            CanonicalMessage::user_text("first"),
            CanonicalMessage::assistant(vec![MessageBlock::Text("first answer".to_string())]),
            CanonicalMessage::user_text("second"),
            CanonicalMessage::assistant(vec![MessageBlock::Text("second answer".to_string())]),
        ]
    );
}

#[test]
fn preserves_multiple_response_groups_inside_one_turn() {
    let events = vec![
        user_input(1, 1, "inspect"),
        model_response(2, 1, "req_1", "I will inspect."),
        tool_call(3, 1, "req_1", "tool_1", 0, "read"),
        tool_result(4, 1, "tool_1", "read output"),
        model_response(5, 1, "req_2", "Done."),
        turn_end(6, 1),
    ];

    let (summary, history) = rebuild_history(&events, &ConversationAddress::MAIN);
    assert!(summary.is_none());
    assert_eq!(
        history,
        vec![
            CanonicalMessage::user_text("inspect"),
            CanonicalMessage::assistant(vec![
                MessageBlock::Text("I will inspect.".to_string()),
                MessageBlock::ToolUse(ToolUse {
                    id: "tool_1".to_string(),
                    name: "read".to_string(),
                    args: json!({"name": "read"}),
                }),
            ]),
            CanonicalMessage::user(vec![MessageBlock::ToolResult(ToolResult {
                tool_call_id: "tool_1".to_string(),
                status: "ok".to_string(),
                summary: "tool_1 summary".to_string(),
                model_content: "read output".to_string(),
                structured: None,
                truncated: false,
            })]),
            CanonicalMessage::assistant(vec![MessageBlock::Text("Done.".to_string())]),
        ]
    );
}

#[test]
fn ignores_permission_metadata_tool_call_duplicate() {
    let events = vec![
        user_input(1, 1, "run pwd"),
        model_response(2, 1, "req_1", "I will run it."),
        event(
            3,
            EventPayload::ToolCall {
                turn: 1,
                ts: "2026-05-13T00:00:02Z".to_string(),
                conversation: Some("main".to_string()),
                tool_call_id: "tool_1".to_string(),
                request: crate::event::test_request_scope("req_1"),
                index: 0,
                tool: "run_command".to_string(),
                args: json!({"command": "pwd"}),
            },
        ),
        event(
            4,
            EventPayload::ToolCall {
                turn: 1,
                ts: "2026-05-13T00:00:03Z".to_string(),
                conversation: Some("main".to_string()),
                tool_call_id: "tool_1".to_string(),
                request: crate::event::test_request_scope("req_1"),
                index: 0,
                tool: "run_command".to_string(),
                args: json!({
                    "risk": "medium",
                    "summary": "run pwd",
                    "candidate": "run_command(command=pwd)",
                    "source": "default_ask",
                }),
            },
        ),
        event(
            5,
            EventPayload::PermissionRequested {
                execution: crate::event::test_execution_scope(),
                turn: 1,
                ts: "2026-05-13T00:00:04Z".to_string(),
                tool_call_id: "tool_1".to_string(),
                tool: "run_command".to_string(),
                risk: "medium".to_string(),
                summary: "run pwd".to_string(),
                candidate: "run_command(command=pwd)".to_string(),
                source: "default_ask".to_string(),
            },
        ),
        event(
            6,
            EventPayload::PermissionAllow {
                execution: crate::event::test_execution_scope(),
                turn: 1,
                ts: "2026-05-13T00:00:05Z".to_string(),
                tool_call_id: "tool_1".to_string(),
                tool: "run_command".to_string(),
                scope: "once".to_string(),
                matcher: "run_command(command=pwd)".to_string(),
                source: "host".to_string(),
            },
        ),
        tool_result(7, 1, "tool_1", "/workspace"),
        turn_end(8, 1),
    ];

    let (summary, history) = rebuild_history(&events, &ConversationAddress::MAIN);
    assert!(summary.is_none());
    assert_eq!(
        history,
        vec![
            CanonicalMessage::user_text("run pwd"),
            CanonicalMessage::assistant(vec![
                MessageBlock::Text("I will run it.".to_string()),
                MessageBlock::ToolUse(ToolUse {
                    id: "tool_1".to_string(),
                    name: "run_command".to_string(),
                    args: json!({"command": "pwd"}),
                }),
            ]),
            CanonicalMessage::user(vec![MessageBlock::ToolResult(ToolResult {
                tool_call_id: "tool_1".to_string(),
                status: "ok".to_string(),
                summary: "tool_1 summary".to_string(),
                model_content: "/workspace".to_string(),
                structured: None,
                truncated: false,
            })]),
        ]
    );
}

#[test]
fn keeps_reused_tool_call_id_in_later_turn() {
    let events = vec![
        user_input(1, 1, "read first"),
        model_response(2, 1, "req_1", "Reading first."),
        event(
            3,
            EventPayload::ToolCall {
                turn: 1,
                ts: "2026-05-13T00:00:02Z".to_string(),
                conversation: Some("main".to_string()),
                tool_call_id: "tool_1".to_string(),
                request: crate::event::test_request_scope("req_1"),
                index: 0,
                tool: "read_file".to_string(),
                args: json!({"path": "first.txt"}),
            },
        ),
        tool_result(4, 1, "tool_1", "first output"),
        turn_end(5, 1),
        user_input(6, 2, "read second"),
        model_response(7, 2, "req_2", "Reading second."),
        event(
            8,
            EventPayload::ToolCall {
                turn: 2,
                ts: "2026-05-13T00:00:06Z".to_string(),
                conversation: Some("main".to_string()),
                tool_call_id: "tool_1".to_string(),
                request: crate::event::test_request_scope("req_2"),
                index: 0,
                tool: "read_file".to_string(),
                args: json!({"path": "second.txt"}),
            },
        ),
        tool_result(9, 2, "tool_1", "second output"),
        turn_end(10, 2),
    ];

    let (summary, history) = rebuild_history(&events, &ConversationAddress::MAIN);
    assert!(summary.is_none());
    assert_eq!(
        history,
        vec![
            CanonicalMessage::user_text("read first"),
            CanonicalMessage::assistant(vec![
                MessageBlock::Text("Reading first.".to_string()),
                MessageBlock::ToolUse(ToolUse {
                    id: "tool_1".to_string(),
                    name: "read_file".to_string(),
                    args: json!({"path": "first.txt"}),
                }),
            ]),
            CanonicalMessage::user(vec![MessageBlock::ToolResult(ToolResult {
                tool_call_id: "tool_1".to_string(),
                status: "ok".to_string(),
                summary: "tool_1 summary".to_string(),
                model_content: "first output".to_string(),
                structured: None,
                truncated: false,
            })]),
            CanonicalMessage::user_text("read second"),
            CanonicalMessage::assistant(vec![
                MessageBlock::Text("Reading second.".to_string()),
                MessageBlock::ToolUse(ToolUse {
                    id: "tool_1".to_string(),
                    name: "read_file".to_string(),
                    args: json!({"path": "second.txt"}),
                }),
            ]),
            CanonicalMessage::user(vec![MessageBlock::ToolResult(ToolResult {
                tool_call_id: "tool_1".to_string(),
                status: "ok".to_string(),
                summary: "tool_1 summary".to_string(),
                model_content: "second output".to_string(),
                structured: None,
                truncated: false,
            })]),
        ]
    );
}

#[test]
fn keeps_same_turn_reused_tool_call_id_when_request_id_differs() {
    let events = vec![
        user_input(1, 1, "run metadata tools"),
        model_response(2, 1, "req_1", "First call."),
        event(
            3,
            EventPayload::ToolCall {
                turn: 1,
                ts: "2026-05-13T00:00:02Z".to_string(),
                conversation: Some("main".to_string()),
                tool_call_id: "tool_1".to_string(),
                request: crate::event::test_request_scope("req_1"),
                index: 0,
                tool: "inspect_metadata".to_string(),
                args: json!({"target": "first"}),
            },
        ),
        tool_result(4, 1, "tool_1", "first output"),
        model_response(5, 1, "req_2", "Second call."),
        event(
            6,
            EventPayload::ToolCall {
                turn: 1,
                ts: "2026-05-13T00:00:04Z".to_string(),
                conversation: Some("main".to_string()),
                tool_call_id: "tool_1".to_string(),
                request: crate::event::test_request_scope("req_2"),
                index: 0,
                tool: "inspect_metadata".to_string(),
                args: json!({
                    "risk": "low",
                    "summary": "real metadata payload",
                    "candidate": "inspect_metadata",
                    "source": "provider",
                }),
            },
        ),
        tool_result(7, 1, "tool_1", "second output"),
        turn_end(8, 1),
    ];

    let (summary, history) = rebuild_history(&events, &ConversationAddress::MAIN);
    assert!(summary.is_none());
    assert_eq!(
        history,
        vec![
            CanonicalMessage::user_text("run metadata tools"),
            CanonicalMessage::assistant(vec![
                MessageBlock::Text("First call.".to_string()),
                MessageBlock::ToolUse(ToolUse {
                    id: "tool_1".to_string(),
                    name: "inspect_metadata".to_string(),
                    args: json!({"target": "first"}),
                }),
            ]),
            CanonicalMessage::user(vec![MessageBlock::ToolResult(ToolResult {
                tool_call_id: "tool_1".to_string(),
                status: "ok".to_string(),
                summary: "tool_1 summary".to_string(),
                model_content: "first output".to_string(),
                structured: None,
                truncated: false,
            })]),
            CanonicalMessage::assistant(vec![
                MessageBlock::Text("Second call.".to_string()),
                MessageBlock::ToolUse(ToolUse {
                    id: "tool_1".to_string(),
                    name: "inspect_metadata".to_string(),
                    args: json!({
                        "risk": "low",
                        "summary": "real metadata payload",
                        "candidate": "inspect_metadata",
                        "source": "provider",
                    }),
                }),
            ]),
            CanonicalMessage::user(vec![MessageBlock::ToolResult(ToolResult {
                tool_call_id: "tool_1".to_string(),
                status: "ok".to_string(),
                summary: "tool_1 summary".to_string(),
                model_content: "second output".to_string(),
                structured: None,
                truncated: false,
            })]),
        ]
    );
}

#[test]
fn main_history_keeps_agent_result_when_child_model_responses_interleave() {
    let events = vec![
        event(
            1,
            EventPayload::TurnStarted {
                execution: crate::event::test_execution_scope(),
                turn: 1,
                ts: "2026-05-13T00:00:00Z".to_string(),
                conversation: "main".to_string(),
            },
        ),
        event(
            2,
            EventPayload::MessageUser {
                execution: crate::event::test_execution_scope(),
                turn: 1,
                ts: "2026-05-13T00:00:01Z".to_string(),
                conversation: "main".to_string(),
                text: "delegate".to_string(),
                from: None,
                via_tool_call_id: None,
            },
        ),
        model_response(3, 1, "req_1", "Delegating."),
        event(
            4,
            EventPayload::ToolCall {
                turn: 1,
                ts: "2026-05-13T00:00:03Z".to_string(),
                conversation: Some("main".to_string()),
                tool_call_id: "agent_1".to_string(),
                request: crate::event::test_request_scope("req_1"),
                index: 0,
                tool: "agent".to_string(),
                args: json!({"to": "explore", "message": "child task"}),
            },
        ),
        event(
            5,
            EventPayload::TurnStarted {
                execution: crate::event::test_execution_scope(),
                turn: 2,
                ts: "2026-05-13T00:00:04Z".to_string(),
                conversation: "explore".to_string(),
            },
        ),
        model_response(6, 2, "req_1", ""),
        event(
            7,
            EventPayload::ToolCall {
                turn: 2,
                ts: "2026-05-13T00:00:05Z".to_string(),
                conversation: Some("explore".to_string()),
                tool_call_id: "read_1".to_string(),
                request: crate::event::test_request_scope("req_1"),
                index: 0,
                tool: "read_file".to_string(),
                args: json!({"path": "README.md"}),
            },
        ),
        event(
            8,
            EventPayload::ToolResult {
                execution: crate::event::test_execution_scope(),
                turn: 2,
                ts: "2026-05-13T00:00:06Z".to_string(),
                conversation: Some("explore".to_string()),
                tool_call_id: "read_1".to_string(),
                status: "ok".to_string(),
                summary: "read README.md".to_string(),
                model_content: "README content".to_string(),
                truncated: false,
                files_read: Vec::new(),
                files_changed: Vec::new(),
                commands_run: Vec::new(),
                memory_changed: None,
                structured: None,
            },
        ),
        model_response(9, 2, "req_2", "CHILD_OK"),
        event(
            10,
            EventPayload::MessageAssistant {
                execution: crate::event::test_execution_scope(),
                turn: 2,
                ts: "2026-05-13T00:00:08Z".to_string(),
                conversation: "explore".to_string(),
                message_id: "req_2".to_string(),
                text: "CHILD_OK".to_string(),
            },
        ),
        event(
            11,
            EventPayload::TurnCompleted {
                execution: crate::event::test_execution_scope(),
                turn: 2,
                ts: "2026-05-13T00:00:09Z".to_string(),
                conversation: "explore".to_string(),
            },
        ),
        event(
            12,
            EventPayload::ToolResult {
                execution: crate::event::test_execution_scope(),
                turn: 1,
                ts: "2026-05-13T00:00:10Z".to_string(),
                conversation: None,
                tool_call_id: "agent_1".to_string(),
                status: "ok".to_string(),
                summary: "explore completed".to_string(),
                model_content: "CHILD_OK".to_string(),
                truncated: false,
                files_read: Vec::new(),
                files_changed: Vec::new(),
                commands_run: Vec::new(),
                memory_changed: None,
                structured: None,
            },
        ),
    ];

    let (summary, history) = rebuild_history(&events, &ConversationAddress::MAIN);
    assert!(summary.is_none());
    assert_eq!(
        history,
        vec![
            CanonicalMessage::user_text("delegate"),
            CanonicalMessage::assistant(vec![
                MessageBlock::Text("Delegating.".to_string()),
                MessageBlock::ToolUse(ToolUse {
                    id: "agent_1".to_string(),
                    name: "agent".to_string(),
                    args: json!({"to": "explore", "message": "child task"}),
                }),
            ]),
            CanonicalMessage::user(vec![MessageBlock::ToolResult(ToolResult {
                tool_call_id: "agent_1".to_string(),
                status: "ok".to_string(),
                summary: "explore completed".to_string(),
                model_content: "CHILD_OK".to_string(),
                structured: None,
                truncated: false,
            })]),
        ]
    );
}

#[test]
fn orders_tool_use_and_tool_results_by_tool_call_index() {
    let events = vec![
        user_input(1, 1, "inspect"),
        model_response(2, 1, "req_1", "I will inspect."),
        tool_call(3, 1, "req_1", "tool_b", 1, "grep"),
        tool_call(4, 1, "req_1", "tool_a", 0, "read"),
        tool_result(5, 1, "tool_b", "grep output"),
        tool_result(6, 1, "tool_a", "read output"),
        turn_end(7, 1),
    ];

    let (summary, history) = rebuild_history(&events, &ConversationAddress::MAIN);
    assert!(summary.is_none());
    assert_eq!(
        history,
        vec![
            CanonicalMessage::user_text("inspect"),
            CanonicalMessage::assistant(vec![
                MessageBlock::Text("I will inspect.".to_string()),
                MessageBlock::ToolUse(ToolUse {
                    id: "tool_a".to_string(),
                    name: "read".to_string(),
                    args: json!({"name": "read"}),
                }),
                MessageBlock::ToolUse(ToolUse {
                    id: "tool_b".to_string(),
                    name: "grep".to_string(),
                    args: json!({"name": "grep"}),
                }),
            ]),
            CanonicalMessage::user(vec![
                MessageBlock::ToolResult(ToolResult {
                    tool_call_id: "tool_a".to_string(),
                    status: "ok".to_string(),
                    summary: "tool_a summary".to_string(),
                    model_content: "read output".to_string(),
                    structured: None,
                    truncated: false,
                }),
                MessageBlock::ToolResult(ToolResult {
                    tool_call_id: "tool_b".to_string(),
                    status: "ok".to_string(),
                    summary: "tool_b summary".to_string(),
                    model_content: "grep output".to_string(),
                    structured: None,
                    truncated: false,
                }),
            ]),
        ]
    );
}

#[test]
fn synthesizes_cancelled_results_for_unmatched_tool_calls() {
    let events = vec![
        user_input(1, 1, "inspect"),
        model_response(2, 1, "req_1", "I will inspect."),
        tool_call(3, 1, "req_1", "tool_a", 0, "read"),
        tool_call(4, 1, "req_1", "tool_b", 1, "grep"),
        tool_result(5, 1, "tool_a", "read output"),
        turn_end(6, 1),
    ];

    let (summary, history) = rebuild_history(&events, &ConversationAddress::MAIN);
    assert!(summary.is_none());
    assert_eq!(
        history,
        vec![
            CanonicalMessage::user_text("inspect"),
            CanonicalMessage::assistant(vec![
                MessageBlock::Text("I will inspect.".to_string()),
                MessageBlock::ToolUse(ToolUse {
                    id: "tool_a".to_string(),
                    name: "read".to_string(),
                    args: json!({"name": "read"}),
                }),
                MessageBlock::ToolUse(ToolUse {
                    id: "tool_b".to_string(),
                    name: "grep".to_string(),
                    args: json!({"name": "grep"}),
                }),
            ]),
            CanonicalMessage::user(vec![
                MessageBlock::ToolResult(ToolResult {
                    tool_call_id: "tool_a".to_string(),
                    status: "ok".to_string(),
                    summary: "tool_a summary".to_string(),
                    model_content: "read output".to_string(),
                    structured: None,
                    truncated: false,
                }),
                MessageBlock::ToolResult(ToolResult {
                    tool_call_id: "tool_b".to_string(),
                    status: "cancelled".to_string(),
                    summary: "tool result missing during replay".to_string(),
                    model_content: "tool call was cancelled before producing a result".to_string(),
                    structured: None,
                    truncated: false,
                }),
            ]),
        ]
    );
}

#[test]
fn preserves_thinking_in_response_group() {
    let events = vec![
        user_input(1, 1, "hi"),
        event(
            2,
            EventPayload::ModelResponse {
                turn: 1,
                ts: "2026-05-13T00:00:01Z".to_string(),
                request: crate::event::test_request_scope("req_1"),
                text: "Hello!".to_string(),
                thinking: Some("The user said hi".to_string()),
                input_tokens_total: Some(10),
            },
        ),
        turn_end(3, 1),
    ];

    let (summary, history) = rebuild_history(&events, &ConversationAddress::MAIN);
    assert!(summary.is_none());
    assert_eq!(
        history,
        vec![
            CanonicalMessage::user_text("hi"),
            CanonicalMessage::assistant(vec![
                MessageBlock::Thinking("The user said hi".to_string()),
                MessageBlock::Text("Hello!".to_string()),
            ]),
        ]
    );
}

#[test]
fn preserves_thinking_with_tool_calls() {
    let events = vec![
        user_input(1, 1, "inspect"),
        event(
            2,
            EventPayload::ModelResponse {
                turn: 1,
                ts: "2026-05-13T00:00:01Z".to_string(),
                request: crate::event::test_request_scope("req_1"),
                text: "I will inspect.".to_string(),
                thinking: Some("Need to read the file first".to_string()),
                input_tokens_total: Some(10),
            },
        ),
        tool_call(3, 1, "req_1", "tool_1", 0, "read"),
        tool_result(4, 1, "tool_1", "read output"),
        event(
            5,
            EventPayload::ModelResponse {
                turn: 1,
                ts: "2026-05-13T00:00:05Z".to_string(),
                request: crate::event::test_request_scope("req_2"),
                text: "Done.".to_string(),
                thinking: Some("File looks good".to_string()),
                input_tokens_total: Some(12),
            },
        ),
        turn_end(6, 1),
    ];

    let (summary, history) = rebuild_history(&events, &ConversationAddress::MAIN);
    assert!(summary.is_none());
    assert_eq!(
        history,
        vec![
            CanonicalMessage::user_text("inspect"),
            CanonicalMessage::assistant(vec![
                MessageBlock::Thinking("Need to read the file first".to_string()),
                MessageBlock::Text("I will inspect.".to_string()),
                MessageBlock::ToolUse(ToolUse {
                    id: "tool_1".to_string(),
                    name: "read".to_string(),
                    args: json!({"name": "read"}),
                }),
            ]),
            CanonicalMessage::user(vec![MessageBlock::ToolResult(ToolResult {
                tool_call_id: "tool_1".to_string(),
                status: "ok".to_string(),
                summary: "tool_1 summary".to_string(),
                model_content: "read output".to_string(),
                structured: None,
                truncated: false,
            })]),
            CanonicalMessage::assistant(vec![
                MessageBlock::Thinking("File looks good".to_string()),
                MessageBlock::Text("Done.".to_string()),
            ]),
        ]
    );
}

fn handoff_event_with_keep_turns(
    id: u64,
    turn: u64,
    summary: &str,
    keep_turns: usize,
) -> StoredEvent {
    event(
        id,
        EventPayload::Handoff {
            execution: crate::event::test_execution_scope(),
            turn,
            ts: "2026-05-27T00:00:01Z".to_string(),
            request_id: "req_handoff".to_string(),
            summary: summary.to_string(),
            keep_turns,
        },
    )
}

#[test]
fn rebuild_history_returns_none_summary_when_no_handoff() {
    let events = vec![
        user_input(1, 1, "hello"),
        model_response(2, 1, "req_1", "hi"),
        turn_end(3, 1),
    ];
    let (summary, history) = rebuild_history(&events, &ConversationAddress::MAIN);
    assert!(summary.is_none());
    assert_eq!(history.len(), 2);
}

#[test]
fn rebuild_history_returns_summary_and_skips_pre_handoff() {
    let events = vec![
        user_input(1, 1, "old question"),
        model_response(2, 1, "req_1", "old answer"),
        turn_end(3, 1),
        handoff_event_with_keep_turns(4, 1, "## Goal\nDo stuff", 2),
        user_input(5, 2, "new question"),
        model_response(6, 2, "req_2", "new answer"),
        turn_end(7, 2),
    ];
    let (summary, history) = rebuild_history(&events, &ConversationAddress::MAIN);
    assert_eq!(summary.as_deref(), Some("## Goal\nDo stuff"));
    assert_eq!(
        history,
        vec![
            CanonicalMessage::user_text("old question"),
            CanonicalMessage::assistant(vec![MessageBlock::Text("old answer".to_string())]),
            CanonicalMessage::user_text("new question"),
            CanonicalMessage::assistant(vec![MessageBlock::Text("new answer".to_string())]),
        ]
    );
}

#[test]
fn rebuild_history_does_not_wrap_forwarded_message_without_tool_call_marker() {
    let events = vec![event(
        1,
        EventPayload::MessageUser {
            execution: crate::event::test_execution_scope(),
            turn: 1,
            ts: "2026-05-13T00:00:00Z".to_string(),
            conversation: "review".to_string(),
            text: "plain forwarded </kuku_delegated_prompt> text".to_string(),
            from: Some("main".to_string()),
            via_tool_call_id: None,
        },
    )];

    let (_, history) = rebuild_history(&events, &ConversationAddress::parse("review").unwrap());

    assert_eq!(
        history,
        vec![CanonicalMessage::user_text(
            "plain forwarded </kuku_delegated_prompt> text"
        )]
    );
}

#[test]
fn rebuild_history_uses_last_handoff_when_multiple() {
    let events = vec![
        user_input(1, 1, "first"),
        model_response(2, 1, "req_1", "answer1"),
        turn_end(3, 1),
        handoff_event_with_keep_turns(4, 1, "first summary", 2),
        user_input(5, 2, "second"),
        model_response(6, 2, "req_2", "answer2"),
        turn_end(7, 2),
        handoff_event_with_keep_turns(8, 2, "second summary", 2),
        user_input(9, 3, "third"),
        model_response(10, 3, "req_3", "answer3"),
        turn_end(11, 3),
    ];
    let (summary, history) = rebuild_history(&events, &ConversationAddress::MAIN);
    assert_eq!(summary.as_deref(), Some("second summary"));
    assert_eq!(
        history,
        vec![
            CanonicalMessage::user_text("first"),
            CanonicalMessage::assistant(vec![MessageBlock::Text("answer1".to_string())]),
            CanonicalMessage::user_text("second"),
            CanonicalMessage::assistant(vec![MessageBlock::Text("answer2".to_string())]),
            CanonicalMessage::user_text("third"),
            CanonicalMessage::assistant(vec![MessageBlock::Text("answer3".to_string())]),
        ]
    );
}

#[test]
fn rebuild_history_keeps_recent_pre_handoff_turns() {
    let events = vec![
        user_input(1, 1, "first"),
        model_response(2, 1, "req_1", "answer1"),
        turn_end(3, 1),
        user_input(4, 2, "second"),
        model_response(5, 2, "req_2", "answer2"),
        turn_end(6, 2),
        user_input(7, 3, "third"),
        model_response(8, 3, "req_3", "answer3"),
        handoff_event_with_keep_turns(9, 3, "summary", 2),
        turn_end(10, 3),
        user_input(11, 4, "fourth"),
        model_response(12, 4, "req_4", "answer4"),
        turn_end(13, 4),
    ];

    let (summary, history) = rebuild_history(&events, &ConversationAddress::MAIN);

    assert_eq!(summary.as_deref(), Some("summary"));
    assert_eq!(
        history,
        vec![
            CanonicalMessage::user_text("second"),
            CanonicalMessage::assistant(vec![MessageBlock::Text("answer2".to_string())]),
            CanonicalMessage::user_text("third"),
            CanonicalMessage::assistant(vec![MessageBlock::Text("answer3".to_string())]),
            CanonicalMessage::user_text("fourth"),
            CanonicalMessage::assistant(vec![MessageBlock::Text("answer4".to_string())]),
        ]
    );
}
