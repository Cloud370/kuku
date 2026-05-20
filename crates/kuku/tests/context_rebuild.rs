use kuku::context::{
    assemble_context, build_request_provenance, rebuild_history, CanonicalMessage, ContextInput,
    EnvironmentSource, FileSource, HistoryRange, InstructionSource, MemorySource, MessageBlock,
    RequestProvenanceInput, ToolRegistryProvenance, ToolResult, ToolSchema, ToolUse,
};
use kuku::event::{EventPayload, EventStore};
use kuku::prompt::builtin_prompt_catalog;
use serde_json::json;

#[test]
fn rebuilds_and_assembles_context_from_events_and_explicit_sources() {
    let temp = tempfile::tempdir().unwrap();
    let events_path = temp.path().join("events.jsonl");
    let mut store = EventStore::open(&events_path).unwrap();

    store
        .append(EventPayload::UserInput {
            turn: 1,
            ts: "2026-05-13T00:00:00Z".to_string(),
            text: "inspect".to_string(),
        })
        .unwrap();
    store
        .append(EventPayload::ModelResponse {
            turn: 1,
            ts: "2026-05-13T00:00:01Z".to_string(),
            request_id: "req_1".to_string(),
            text: "Done.".to_string(),
            thinking: None,
            stop_reason: "end_turn".to_string(),
            tool_call_count: None,
            usage: json!({"input_tokens": 3}),
        })
        .unwrap();

    let events = EventStore::replay(&events_path).unwrap();
    let history = rebuild_history(&events);
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
        builtin_prompt_catalog(),
    )
    .unwrap();

    assert!(assembly.system_prompt.contains("<kuku_identity>"));
    assert!(assembly.system_prompt.contains("<kuku_hard_rules>"));
    assert!(assembly.system_prompt.contains("<kuku_working_style>"));
    assert_eq!(assembly.prelude_messages.len(), 2);
    assert_eq!(assembly.history, history);
    assert_eq!(assembly.tools, tools);
    assert_eq!(assembly.project_instruction_sources, project_instructions);
    assert_eq!(
        assembly.memory_sources,
        vec![global_memory.clone(), project_memory.clone()]
    );
    assert_eq!(assembly.prompt_asset_sources.len(), 3);

    match &assembly.prelude_messages[0].blocks[..] {
        [MessageBlock::Text(text)] => {
            assert!(text.contains("<kuku_execution_context>"));
            assert!(text.contains("Workspace root: /workspace"));
            assert!(text.contains("Platform: linux"));
            assert!(text.contains("Current date: 2026-05-14"));
            assert!(text.contains("<kuku_project_instructions>"));
            assert!(text.contains("instructions"));
            assert!(text.contains("<kuku_memory>"));
            assert!(text.contains("global"));
            assert!(text.contains("project"));
            assert!(!text.contains("<kuku_current_task>"));
        }
        other => panic!("expected one project-context text block, got {other:?}"),
    }

    match &assembly.prelude_messages[1].blocks[..] {
        [MessageBlock::Text(text)] => {
            assert!(text.contains("<kuku_tool_guidance>"));
            assert!(
                text.contains("Use tools to establish evidence before concluding or modifying.")
            );
            assert!(text.contains("Do not guess when tools can establish the answer."));
            assert!(text.contains("Treat tool results as evidence."));
            assert!(text.contains(
                "Do not claim conclusions that are not supported by tool or file evidence."
            ));
        }
        other => panic!("expected one tool-guidance text block, got {other:?}"),
    }

    let provenance = build_request_provenance(RequestProvenanceInput {
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
        subagent_registry: None,
        skill_registry: None,
        provider_format: "anthropic".to_string(),
        provider: "anthropic".to_string(),
        model: "claude-sonnet-4-6".to_string(),
        request_params: json!({"temperature": 0}),
        token_estimate: None,
        context_budget_tier: "normal".to_string(),
        max_context_tokens: Some(200_000),
        remaining_input_tokens: None,
    });

    assert_eq!(provenance.request_id, "req_2");
    assert_eq!(provenance.platform, "linux");
    assert_eq!(provenance.current_date, "2026-05-14");
    assert_eq!(provenance.history_range.first_event_id, Some(1));
    assert_eq!(provenance.history_range.last_event_id, Some(2));
    assert_eq!(provenance.prompt_asset_sources.len(), 3);
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
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
        builtin_prompt_catalog(),
    )
    .unwrap();

    match &assembly.prelude_messages[0].blocks[..] {
        [MessageBlock::Text(text)] => {
            assert!(text.contains("<kuku_execution_context>"));
            assert!(text.contains("No project instructions found."));
            assert!(text.contains("No global memory."));
            assert!(text.contains("No project memory."));
            assert!(!text.contains("<kuku_current_task>"));
        }
        other => panic!("expected one project-context text block, got {other:?}"),
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
        builtin_prompt_catalog(),
    )
    .unwrap();

    assembly
        .prelude_messages
        .insert(1, CanonicalMessage::user_text("<kuku_system_notice>\n- Context drift: /workspace/AGENTS.md changed (sha256:old -> sha256:new)\n</kuku_system_notice>"));

    assert_eq!(assembly.prelude_messages.len(), 3);
    match &assembly.prelude_messages[1].blocks[..] {
        [MessageBlock::Text(text)] => {
            assert!(text.contains("<kuku_system_notice>"));
            assert!(text.contains("Context drift:"));
        }
        other => panic!("expected one drift-notice text block, got {other:?}"),
    }
    match &assembly.prelude_messages[2].blocks[..] {
        [MessageBlock::Text(text)] => {
            assert!(text.contains("<kuku_tool_guidance>"));
        }
        other => panic!("expected tool guidance after drift notice, got {other:?}"),
    }
}

#[test]
fn rebuilds_multi_group_tool_history_at_crate_boundary() {
    let temp = tempfile::tempdir().unwrap();
    let events_path = temp.path().join("events.jsonl");
    let mut store = EventStore::open(&events_path).unwrap();

    store
        .append(EventPayload::UserInput {
            turn: 1,
            ts: "2026-05-13T00:00:00Z".to_string(),
            text: "inspect".to_string(),
        })
        .unwrap();
    store
        .append(EventPayload::ModelResponse {
            turn: 1,
            ts: "2026-05-13T00:00:01Z".to_string(),
            request_id: "req_1".to_string(),
            text: "I will inspect.".to_string(),
            thinking: None,
            stop_reason: "tool_use".to_string(),
            tool_call_count: Some(2),
            usage: json!({"input_tokens": 10}),
        })
        .unwrap();
    store
        .append(EventPayload::ToolCall {
            turn: 1,
            ts: "2026-05-13T00:00:02Z".to_string(),
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
            tool_call_id: "tool_a".to_string(),
            status: "ok".to_string(),
            summary: "tool_a summary".to_string(),
            model_content: "read output".to_string(),
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
            stop_reason: "end_turn".to_string(),
            tool_call_count: None,
            usage: json!({"input_tokens": 12}),
        })
        .unwrap();

    let history = rebuild_history(&EventStore::replay(&events_path).unwrap());
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
