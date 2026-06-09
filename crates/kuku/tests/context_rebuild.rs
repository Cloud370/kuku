use kuku::context::{
    assemble_context, rebuild_history, restore_prompt_snapshot, CanonicalMessage, ContextInput,
    EnvironmentSource, FileSource, HistoryRange, InstructionSource, MemorySource, MessageBlock,
    RequestProvenance, ToolRegistryProvenance, ToolResult, ToolSchema, ToolUse,
};
use kuku::conversation::address::ConversationAddress;
use kuku::event::{EventPayload, EventStore};
use kuku::prompt::builtin_prompt_catalog;
use serde_json::json;

fn prompt_snapshot(
    conversation: &str,
    turn: u64,
    messages: Vec<kuku::event::types::ContextMessage>,
    project_instruction_sources: Vec<FileSource>,
) -> EventPayload {
    EventPayload::PromptSnapshot {
        ts: "2026-06-09T00:00:00Z".to_string(),
        conversation: conversation.to_string(),
        binding_id: format!("binding:{conversation}"),
        snapshot_id: format!("snapshot:{conversation}:{turn}"),
        turn,
        messages,
        project_instruction_sources,
        memory_sources: vec![],
        prompt_asset_sources: vec![],
        skills: json!({}),
        bootstrap_loaded: vec![],
        provider: "anthropic".to_string(),
        model: "claude-sonnet-4-6".to_string(),
        renderer: kuku::context::PromptRendererIdentity {
            provider: "anthropic".to_string(),
            renderer: "anthropic".to_string(),
        },
        tool_registry: Box::new(ToolRegistryProvenance {
            hash: "sha256:tools".to_string(),
            names: vec![],
            tool_count: 0,
        }),
        agent_registry: None,
        skill_registry: Box::new(None),
        plugin_registry: Box::new(None),
        capabilities: kuku::context::PromptCapabilityMetadata {
            context_budget_tier: "normal".to_string(),
            max_context_tokens: Some(200_000),
            remaining_input_tokens: Some(180_000),
        },
    }
}

#[test]
fn rebuilds_and_assembles_context_from_events_and_explicit_sources() {
    let temp = tempfile::tempdir().unwrap();
    let events_path = temp.path().join("events.jsonl");
    let mut store = EventStore::open(&events_path).unwrap();

    store
        .append(EventPayload::MessageUser {
            turn: 1,
            ts: "2026-05-13T00:00:00Z".to_string(),
            conversation: "main".to_string(),
            text: "inspect".to_string(),
            from: None,
            via_tool_call_id: None,
        })
        .unwrap();
    store
        .append(EventPayload::ModelResponse {
            turn: 1,
            ts: "2026-05-13T00:00:01Z".to_string(),
            request_id: "req_1".to_string(),
            text: "Done.".to_string(),
            thinking: None,
            input_tokens_total: Some(3),
        })
        .unwrap();

    let events = EventStore::replay(&events_path).unwrap();
    let (summary, history) = rebuild_history(&events, &ConversationAddress::MAIN);
    assert!(summary.is_none());
    assert_eq!(
        history,
        vec![
            CanonicalMessage::user_text("inspect"),
            CanonicalMessage::assistant(vec![MessageBlock::Text("Done.".to_string())]),
        ]
    );

    let project_instructions = vec![InstructionSource {
        path: "/workspace/AGENTS.md".to_string(),
        kind: "agents".to_string(),
        hash: "sha256:agents".to_string(),
        content: "instructions".to_string(),
    }];
    let global_memory = MemorySource {
        path: "/home/user/.kuku/memory.md".to_string(),
        hash: "sha256:global".to_string(),
        content: "global".to_string(),
    };
    let project_memory = MemorySource {
        path: "/home/user/.kuku/p/workspace/memory.md".to_string(),
        hash: "sha256:project".to_string(),
        content: "project".to_string(),
    };
    let tools = vec![ToolSchema {
        name: "read".to_string(),
        description: "Read a file".to_string(),
        input_schema: json!({"type": "object"}),
    }];

    let assembly = assemble_context(
        ContextInput {
            environment: EnvironmentSource {
                workspace_path: "/workspace".to_string(),
                platform: "linux".to_string(),
                current_date: "2026-05-14".to_string(),
            },
            project_instructions: project_instructions.clone(),
            global_memory: Some(global_memory.clone()),
            project_memory: Some(project_memory.clone()),
            history: history.clone(),
            tools: tools.clone(),
            model_tiers: Vec::new(),
            runtime_blocks: None,
        },
        &builtin_prompt_catalog(),
    )
    .unwrap();

    assert!(assembly.system_prompt.contains("<kuku_identity>"));
    assert!(assembly.system_prompt.contains("<kuku_hard_rules>"));
    assert!(assembly.system_prompt.contains("<kuku_working_style>"));
    assert_eq!(assembly.prelude_messages.len(), 4);
    assert_eq!(assembly.history, history);
    assert_eq!(assembly.tools, tools);
    assert_eq!(assembly.project_instruction_sources, project_instructions);
    assert_eq!(
        assembly.memory_sources,
        vec![global_memory.clone(), project_memory.clone()]
    );
    assert_eq!(assembly.prompt_asset_sources.len(), 5);

    // [0] tool_guidance
    match &assembly.prelude_messages[0].blocks[..] {
        [MessageBlock::Text(text)] => {
            assert!(text.contains("<kuku_tool_guidance>"));
            assert!(
                text.contains("Use tools to establish evidence before concluding or modifying.")
            );
        }
        other => panic!("expected tool-guidance text block, got {other:?}"),
    }

    // [1] global_memory
    match &assembly.prelude_messages[1].blocks[..] {
        [MessageBlock::Text(text)] => {
            assert!(text.contains("<kuku_global_memory>"));
            assert!(text.contains("global"));
        }
        other => panic!("expected global-memory text block, got {other:?}"),
    }

    // [2] project_memory
    match &assembly.prelude_messages[2].blocks[..] {
        [MessageBlock::Text(text)] => {
            assert!(text.contains("<kuku_project_memory>"));
            assert!(text.contains("project"));
        }
        other => panic!("expected project-memory text block, got {other:?}"),
    }

    // [3] project_context
    match &assembly.prelude_messages[3].blocks[..] {
        [MessageBlock::Text(text)] => {
            assert!(text.contains("<kuku_execution_context>"));
            assert!(text.contains("Workspace root: /workspace"));
            assert!(text.contains("Platform: linux"));
            assert!(text.contains("Current date: 2026-05-14"));
            assert!(text.contains("<kuku_project_instructions>"));
            assert!(text.contains("instructions"));
            assert!(!text.contains("<kuku_memory>"));
            assert!(!text.contains("<kuku_current_task>"));
        }
        other => panic!("expected project-context text block, got {other:?}"),
    }

    let provenance = RequestProvenance {
        request_id: "req_2".to_string(),
        tier: "balanced".to_string(),
        workspace: "/workspace".to_string(),
        platform: "linux".to_string(),
        current_date: "2026-05-14".to_string(),
        project_instruction_sources: vec![FileSource {
            path: "/workspace/AGENTS.md".to_string(),
            hash: "sha256-agents".to_string(),
        }],
        memory_sources: vec![FileSource {
            path: "/home/user/.kuku/memory.md".to_string(),
            hash: "sha256-memory".to_string(),
        }],
        prompt_asset_sources: vec![
            FileSource {
                path: "crates/kuku/prompts/system.md".to_string(),
                hash: "sha256:system".to_string(),
            },
            FileSource {
                path: "crates/kuku/prompts/project-context.md".to_string(),
                hash: "sha256:synthetic".to_string(),
            },
            FileSource {
                path: "crates/kuku/prompts/tool-guidance.md".to_string(),
                hash: "sha256:tool-guidance".to_string(),
            },
        ],
        history_range: HistoryRange {
            first_event_id: Some(events.first().unwrap().id),
            last_event_id: Some(events.last().unwrap().id),
        },
        tool_registry: ToolRegistryProvenance {
            hash: "sha256-tools".to_string(),
            names: vec!["read".to_string()],
            tool_count: 1,
        },
        agent_registry: None,
        skill_registry: None,
        plugin_registry: None,
        provider_format: "anthropic".to_string(),
        provider: "anthropic".to_string(),
        model: "claude-sonnet-4-6".to_string(),
        request_params: json!({"temperature": 0}),
        token_estimate: None,
        context_budget_tier: "normal".to_string(),
        max_context_tokens: Some(200_000),
        remaining_input_tokens: None,
    };

    assert_eq!(provenance.request_id, "req_2");
    assert_eq!(provenance.platform, "linux");
    assert_eq!(provenance.current_date, "2026-05-14");
    assert_eq!(provenance.history_range.first_event_id, Some(1));
    assert_eq!(provenance.history_range.last_event_id, Some(2));
    assert_eq!(provenance.prompt_asset_sources.len(), 3);
    assert!(events_path.exists());
}

#[test]
fn assemble_context_keeps_stable_empty_placeholders() {
    let assembly = assemble_context(
        ContextInput {
            environment: EnvironmentSource {
                workspace_path: "/workspace".to_string(),
                platform: "windows".to_string(),
                current_date: "2026-05-14".to_string(),
            },
            project_instructions: Vec::new(),
            global_memory: None,
            project_memory: None,
            history: Vec::new(),
            tools: Vec::new(),
            model_tiers: Vec::new(),
            runtime_blocks: None,
        },
        &builtin_prompt_catalog(),
    )
    .unwrap();

    // [0] tool_guidance
    match &assembly.prelude_messages[0].blocks[..] {
        [MessageBlock::Text(text)] => {
            assert!(text.contains("<kuku_tool_guidance>"));
        }
        other => panic!("expected tool-guidance text block, got {other:?}"),
    }

    // [1] global_memory with fallback
    match &assembly.prelude_messages[1].blocks[..] {
        [MessageBlock::Text(text)] => {
            assert!(text.contains("<kuku_global_memory>"));
            assert!(text.contains("No global memory."));
        }
        other => panic!("expected global-memory text block, got {other:?}"),
    }

    // [2] project_memory with fallback
    match &assembly.prelude_messages[2].blocks[..] {
        [MessageBlock::Text(text)] => {
            assert!(text.contains("<kuku_project_memory>"));
            assert!(text.contains("No project memory."));
        }
        other => panic!("expected project-memory text block, got {other:?}"),
    }

    // [3] project_context
    match &assembly.prelude_messages[3].blocks[..] {
        [MessageBlock::Text(text)] => {
            assert!(text.contains("<kuku_execution_context>"));
            assert!(text.contains("No project instructions found."));
            assert!(!text.contains("<kuku_current_task>"));
        }
        other => panic!("expected project-context text block, got {other:?}"),
    }
}

#[test]
fn drift_notice_can_be_inserted_between_project_context_and_tool_guidance() {
    let mut assembly = assemble_context(
        ContextInput {
            environment: EnvironmentSource {
                workspace_path: "/workspace".to_string(),
                platform: "linux".to_string(),
                current_date: "2026-05-14".to_string(),
            },
            project_instructions: Vec::new(),
            global_memory: None,
            project_memory: None,
            history: Vec::new(),
            tools: Vec::new(),
            model_tiers: Vec::new(),
            runtime_blocks: None,
        },
        &builtin_prompt_catalog(),
    )
    .unwrap();

    assembly
        .prelude_messages
        .insert(1, CanonicalMessage::user_text("<kuku_system_notice>\n- Context drift: /workspace/AGENTS.md changed (sha256:old -> sha256:new)\n</kuku_system_notice>"));

    assert_eq!(assembly.prelude_messages.len(), 5);
    match &assembly.prelude_messages[1].blocks[..] {
        [MessageBlock::Text(text)] => {
            assert!(text.contains("<kuku_system_notice>"));
            assert!(text.contains("Context drift:"));
        }
        other => panic!("expected drift-notice text block, got {other:?}"),
    }
    match &assembly.prelude_messages[2].blocks[..] {
        [MessageBlock::Text(text)] => {
            assert!(text.contains("<kuku_global_memory>"));
        }
        other => panic!("expected global_memory after drift notice, got {other:?}"),
    }
}

#[test]
fn rebuilds_multi_group_tool_history_at_crate_boundary() {
    let temp = tempfile::tempdir().unwrap();
    let events_path = temp.path().join("events.jsonl");
    let mut store = EventStore::open(&events_path).unwrap();

    store
        .append(EventPayload::MessageUser {
            turn: 1,
            ts: "2026-05-13T00:00:00Z".to_string(),
            conversation: "main".to_string(),
            text: "inspect".to_string(),
            from: None,
            via_tool_call_id: None,
        })
        .unwrap();
    store
        .append(EventPayload::ModelResponse {
            turn: 1,
            ts: "2026-05-13T00:00:01Z".to_string(),
            request_id: "req_1".to_string(),
            text: "I will inspect.".to_string(),
            thinking: None,
            input_tokens_total: Some(10),
        })
        .unwrap();
    store
        .append(EventPayload::ToolCall {
            turn: 1,
            ts: "2026-05-13T00:00:02Z".to_string(),
            conversation: None,
            request_id: "req_1".to_string(),
            tool_call_id: "tool_b".to_string(),
            index: 1,
            tool: "grep".to_string(),
            args: json!({"name": "grep"}),
        })
        .unwrap();
    store
        .append(EventPayload::ToolCall {
            turn: 1,
            ts: "2026-05-13T00:00:03Z".to_string(),
            conversation: None,
            request_id: "req_1".to_string(),
            tool_call_id: "tool_a".to_string(),
            index: 0,
            tool: "read".to_string(),
            args: json!({"name": "read"}),
        })
        .unwrap();
    store
        .append(EventPayload::ToolResult {
            turn: 1,
            ts: "2026-05-13T00:00:04Z".to_string(),
            conversation: None,
            tool_call_id: "tool_a".to_string(),
            status: "ok".to_string(),
            summary: "tool_a summary".to_string(),
            model_content: "read output".to_string(),
            files_read: Vec::new(),
            files_changed: Vec::new(),
            commands_run: Vec::new(),
            memory_changed: None,
            structured: None,
            truncated: false,
        })
        .unwrap();
    store
        .append(EventPayload::ModelResponse {
            turn: 1,
            ts: "2026-05-13T00:00:05Z".to_string(),
            request_id: "req_2".to_string(),
            text: "Done.".to_string(),
            thinking: None,
            input_tokens_total: Some(12),
        })
        .unwrap();

    let (summary, history) = rebuild_history(
        &EventStore::replay(&events_path).unwrap(),
        &ConversationAddress::MAIN,
    );
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
            CanonicalMessage::assistant(vec![MessageBlock::Text("Done.".to_string())]),
        ]
    );
}

#[test]
fn restores_frozen_prelude_from_fact_event() {
    let events = vec![
        kuku::event::StoredEvent {
            id: 1,
            payload: prompt_snapshot(
                "main",
                1,
                vec![
                    kuku::event::types::ContextMessage {
                        role: "user".to_string(),
                        content: "<kuku_tool_guidance>use tools</kuku_tool_guidance>".to_string(),
                    },
                    kuku::event::types::ContextMessage {
                        role: "user".to_string(),
                        content:
                            "<kuku_execution_context>workspace: /tmp/evlog</kuku_execution_context>"
                                .to_string(),
                    },
                ],
                vec![],
            ),
        },
        kuku::event::StoredEvent {
            id: 2,
            payload: EventPayload::MessageUser {
                turn: 1,
                ts: "2026-05-18T00:00:01Z".to_string(),
                conversation: "main".to_string(),
                text: "inspect".to_string(),
                from: None,
                via_tool_call_id: None,
            },
        },
    ];

    let restored = kuku::context::restore_prompt_snapshot(&events, "main").unwrap();
    assert_eq!(restored.len(), 2);
    assert_eq!(
        restored[0],
        CanonicalMessage::user_text("<kuku_tool_guidance>use tools</kuku_tool_guidance>")
    );
    assert_eq!(
        restored[1],
        CanonicalMessage::user_text(
            "<kuku_execution_context>workspace: /tmp/evlog</kuku_execution_context>"
        )
    );
}

#[test]
fn rebuild_history_uses_snapshot_for_target_conversation_only() {
    let review = ConversationAddress::parse("review").unwrap();
    let events = vec![
        kuku::event::StoredEvent {
            id: 1,
            payload: prompt_snapshot(
                "main",
                1,
                vec![
                    kuku::event::types::ContextMessage {
                        role: "user".to_string(),
                        content: "<kuku_project_instructions>main v1</kuku_project_instructions>"
                            .to_string(),
                    },
                    kuku::event::types::ContextMessage {
                        role: "user".to_string(),
                        content: "<kuku_input_frame><input.message>main ask</input.message></kuku_input_frame>"
                            .to_string(),
                    },
                ],
                vec![FileSource {
                    path: "/workspace/AGENTS.md".to_string(),
                    hash: "sha256:main-v1".to_string(),
                }],
            ),
        },
        kuku::event::StoredEvent {
            id: 2,
            payload: prompt_snapshot(
                "review",
                1,
                vec![
                    kuku::event::types::ContextMessage {
                        role: "user".to_string(),
                        content:
                            "<kuku_project_instructions>review v1</kuku_project_instructions>"
                                .to_string(),
                    },
                    kuku::event::types::ContextMessage {
                        role: "user".to_string(),
                        content:
                            "<kuku_input_frame><input.message>review ask</input.message></kuku_input_frame>"
                                .to_string(),
                    },
                ],
                vec![FileSource {
                    path: "/workspace/AGENTS.md".to_string(),
                    hash: "sha256:review-v1".to_string(),
                }],
            ),
        },
        kuku::event::StoredEvent {
            id: 3,
            payload: EventPayload::MessageAssistant {
                ts: "2026-06-09T00:00:02Z".to_string(),
                conversation: "main".to_string(),
                turn: 1,
                message_id: "m1".to_string(),
                text: "main answer".to_string(),
            },
        },
        kuku::event::StoredEvent {
            id: 4,
            payload: EventPayload::MessageAssistant {
                ts: "2026-06-09T00:00:03Z".to_string(),
                conversation: "review".to_string(),
                turn: 1,
                message_id: "r1".to_string(),
                text: "review answer".to_string(),
            },
        },
    ];

    let (_, main_history) = rebuild_history(&events, &ConversationAddress::MAIN);
    let (_, review_history) = rebuild_history(&events, &review);

    assert_eq!(
        main_history,
        vec![
            CanonicalMessage::user_text(
                "<kuku_project_instructions>main v1</kuku_project_instructions>",
            ),
            CanonicalMessage::user_text(
                "<kuku_input_frame><input.message>main ask</input.message></kuku_input_frame>",
            ),
            CanonicalMessage::assistant(vec![MessageBlock::Text("main answer".to_string())]),
        ]
    );
    assert_eq!(
        review_history,
        vec![
            CanonicalMessage::user_text(
                "<kuku_project_instructions>review v1</kuku_project_instructions>",
            ),
            CanonicalMessage::user_text(
                "<kuku_input_frame><input.message>review ask</input.message></kuku_input_frame>",
            ),
            CanonicalMessage::assistant(vec![MessageBlock::Text("review answer".to_string())]),
        ]
    );
}

#[test]
fn rebuild_history_replays_non_main_scoped_tool_result() {
    let events = vec![
        kuku::event::StoredEvent {
            id: 1,
            payload: EventPayload::MessageUser {
                ts: "2026-06-09T00:00:01Z".to_string(),
                conversation: "review".to_string(),
                turn: 1,
                text: "inspect".to_string(),
                from: Some("main".to_string()),
                via_tool_call_id: Some("toolu_agent_review".to_string()),
            },
        },
        kuku::event::StoredEvent {
            id: 2,
            payload: EventPayload::ToolCall {
                turn: 1,
                ts: "2026-06-09T00:00:02Z".to_string(),
                conversation: Some("review".to_string()),
                request_id: "req_review_1".to_string(),
                tool_call_id: "toolu_read".to_string(),
                index: 0,
                tool: "read_file".to_string(),
                args: json!({"path": "README.md"}),
            },
        },
        kuku::event::StoredEvent {
            id: 3,
            payload: EventPayload::ToolResult {
                turn: 1,
                ts: "2026-06-09T00:00:03Z".to_string(),
                conversation: Some("review".to_string()),
                tool_call_id: "toolu_read".to_string(),
                status: "ok".to_string(),
                summary: "read README.md".to_string(),
                model_content: "README contents".to_string(),
                truncated: false,
                files_read: Vec::new(),
                files_changed: Vec::new(),
                commands_run: Vec::new(),
                memory_changed: None,
                structured: None,
            },
        },
    ];

    let review = ConversationAddress::parse("review").unwrap();
    let (_, history) = rebuild_history(&events, &review);

    assert_eq!(
        history,
        vec![
            CanonicalMessage::user_text("inspect"),
            CanonicalMessage::assistant(vec![MessageBlock::ToolUse(ToolUse {
                id: "toolu_read".to_string(),
                name: "read_file".to_string(),
                args: json!({"path": "README.md"}),
            })]),
            CanonicalMessage::user(vec![MessageBlock::ToolResult(ToolResult {
                tool_call_id: "toolu_read".to_string(),
                status: "ok".to_string(),
                summary: "read README.md".to_string(),
                model_content: "README contents".to_string(),
                structured: None,
                truncated: false,
            })]),
        ]
    );
}

#[test]
fn rebuild_history_ignores_context_source_facts_and_respects_handoff_cutoff() {
    let events = vec![
        kuku::event::StoredEvent {
            id: 1,
            payload: EventPayload::ContextSources {
                turn: 1,
                ts: "2026-05-18T00:00:00Z".to_string(),
                request_id: "req_1".to_string(),
                project_instruction_sources: vec![FileSource {
                    path: "/workspace/AGENTS.md".to_string(),
                    hash: "sha256:before".to_string(),
                }],
                memory_sources: vec![],
            },
        },
        kuku::event::StoredEvent {
            id: 2,
            payload: EventPayload::MessageUser {
                turn: 1,
                ts: "2026-05-18T00:00:01Z".to_string(),
                conversation: "main".to_string(),
                text: "old".to_string(),
                from: None,
                via_tool_call_id: None,
            },
        },
        kuku::event::StoredEvent {
            id: 3,
            payload: EventPayload::ModelResponse {
                turn: 1,
                ts: "2026-05-18T00:00:02Z".to_string(),
                request_id: "req_1".to_string(),
                text: "old answer".to_string(),
                thinking: None,
                input_tokens_total: Some(10),
            },
        },
        kuku::event::StoredEvent {
            id: 4,
            payload: EventPayload::Handoff {
                turn: 1,
                ts: "2026-05-18T00:00:03Z".to_string(),
                request_id: "req_1".to_string(),
                summary: "carry forward".to_string(),
                keep_turns: 1,
            },
        },
        kuku::event::StoredEvent {
            id: 5,
            payload: EventPayload::MessageUser {
                turn: 2,
                ts: "2026-05-18T00:00:04Z".to_string(),
                conversation: "main".to_string(),
                text: "new".to_string(),
                from: None,
                via_tool_call_id: None,
            },
        },
        kuku::event::StoredEvent {
            id: 6,
            payload: EventPayload::ModelResponse {
                turn: 2,
                ts: "2026-05-18T00:00:05Z".to_string(),
                request_id: "req_2".to_string(),
                text: "new answer".to_string(),
                thinking: None,
                input_tokens_total: Some(12),
            },
        },
    ];

    let (summary, history) = rebuild_history(&events, &ConversationAddress::MAIN);
    assert_eq!(summary.as_deref(), Some("carry forward"));
    assert_eq!(
        history,
        vec![
            CanonicalMessage::user_text("old"),
            CanonicalMessage::assistant(vec![MessageBlock::Text("old answer".to_string())]),
            CanonicalMessage::user_text("new"),
            CanonicalMessage::assistant(vec![MessageBlock::Text("new answer".to_string())]),
        ]
    );
}

#[test]
fn prompt_snapshot_preserves_old_agents_content_after_file_changes() {
    let events = vec![
        kuku::event::StoredEvent {
            id: 1,
            payload: prompt_snapshot(
                "main",
                1,
                vec![
                    kuku::event::types::ContextMessage {
                        role: "user".to_string(),
                        content:
                            "<kuku_project_instructions>AGENTS version one</kuku_project_instructions>"
                                .to_string(),
                    },
                    kuku::event::types::ContextMessage {
                        role: "user".to_string(),
                        content: "<input.message>inspect</input.message>".to_string(),
                    },
                ],
                vec![FileSource {
                    path: "/workspace/AGENTS.md".to_string(),
                    hash: "sha256:one".to_string(),
                }],
            ),
        },
        kuku::event::StoredEvent {
            id: 2,
            payload: EventPayload::ContextSources {
                turn: 2,
                ts: "2026-06-09T00:00:01Z".to_string(),
                request_id: "req_2".to_string(),
                project_instruction_sources: vec![FileSource {
                    path: "/workspace/AGENTS.md".to_string(),
                    hash: "sha256:two".to_string(),
                }],
                memory_sources: vec![],
            },
        },
        kuku::event::StoredEvent {
            id: 3,
            payload: EventPayload::MessageAssistant {
                ts: "2026-06-09T00:00:02Z".to_string(),
                conversation: "main".to_string(),
                turn: 1,
                message_id: "m1".to_string(),
                text: "done".to_string(),
            },
        },
    ];

    let (_, history) = rebuild_history(&events, &ConversationAddress::MAIN);

    assert_eq!(
        history,
        vec![
            CanonicalMessage::user_text(
                "<kuku_project_instructions>AGENTS version one</kuku_project_instructions>",
            ),
            CanonicalMessage::user_text("<input.message>inspect</input.message>"),
            CanonicalMessage::assistant(vec![MessageBlock::Text("done".to_string())]),
        ]
    );
}

#[test]
fn conversation_skill_binding_is_stable() {
    let skill_registry_v1 = json!({
        "definitions": {
            "review": {
                "name": "review",
                "description": "Review carefully",
                "instructions": "Review skill instructions v1.",
                "source": "project",
                "hash": "sha256:v1",
                "source_path": "/skills/review",
                "allowed_tools": null,
                "disallowed_tools": null,
                "max_turns": null,
                "model": null,
                "license": null,
                "compatibility": null,
                "metadata": null
            }
        },
        "names": ["review"],
        "hash": "sha256:registry-v1"
    });
    let skill_registry_v2 = json!({
        "definitions": {
            "review": {
                "name": "review",
                "description": "Review carefully",
                "instructions": "Review skill instructions v2.",
                "source": "project",
                "hash": "sha256:v2",
                "source_path": "/skills/review",
                "allowed_tools": null,
                "disallowed_tools": null,
                "max_turns": null,
                "model": null,
                "license": null,
                "compatibility": null,
                "metadata": null
            }
        },
        "names": ["review"],
        "hash": "sha256:registry-v2"
    });
    let events = vec![
        kuku::event::StoredEvent {
            id: 1,
            payload: EventPayload::PromptSnapshot {
                ts: "2026-06-09T00:00:00Z".to_string(),
                conversation: "main".to_string(),
                binding_id: "binding:main:review-v1".to_string(),
                snapshot_id: "snapshot:main:1".to_string(),
                turn: 1,
                messages: vec![
                    kuku::event::types::ContextMessage {
                        role: "user".to_string(),
                        content: "Review skill instructions v1.".to_string(),
                    },
                    kuku::event::types::ContextMessage {
                        role: "user".to_string(),
                        content: "<input.message>inspect</input.message>".to_string(),
                    },
                ],
                project_instruction_sources: vec![],
                memory_sources: vec![],
                prompt_asset_sources: vec![],
                skills: skill_registry_v1,
                bootstrap_loaded: vec!["review".to_string()],
                provider: "anthropic".to_string(),
                model: "claude-sonnet-4-6".to_string(),
                renderer: kuku::context::PromptRendererIdentity {
                    provider: "anthropic".to_string(),
                    renderer: "anthropic".to_string(),
                },
                tool_registry: Box::new(ToolRegistryProvenance {
                    hash: "sha256:tools".to_string(),
                    names: vec![],
                    tool_count: 0,
                }),
                agent_registry: None,
                skill_registry: Box::new(None),
                plugin_registry: Box::new(None),
                capabilities: kuku::context::PromptCapabilityMetadata {
                    context_budget_tier: "normal".to_string(),
                    max_context_tokens: Some(200_000),
                    remaining_input_tokens: Some(180_000),
                },
            },
        },
        kuku::event::StoredEvent {
            id: 2,
            payload: EventPayload::ContextSkills {
                conversation: "main".to_string(),
                turn: 2,
                ts: "2026-06-09T00:00:01Z".to_string(),
                registry: skill_registry_v2,
                bootstrap_loaded: vec!["review".to_string()],
            },
        },
    ];

    let snapshot = restore_prompt_snapshot(&events, "main").unwrap();
    assert_eq!(
        snapshot[0],
        CanonicalMessage::user_text("Review skill instructions v1.")
    );

    let (_, history) = rebuild_history(&events, &ConversationAddress::MAIN);
    assert_eq!(
        history[0],
        CanonicalMessage::user_text("Review skill instructions v1.")
    );
}
