use kuku::context::{
    assemble_context, build_request_provenance, rebuild_history, CanonicalMessage, ContextInput,
    ContextSource, FileSource, HistoryRange, InstructionSource, MemorySource, MessageBlock,
    RequestProvenanceInput, ToolRegistryProvenance, ToolResult, ToolSchema, ToolUse,
};
use kuku::event::{EventPayload, EventStore};
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
        content: "instructions".to_string(),
    }];
    let global_memory = MemorySource {
        path: "/home/user/.kuku/memory.md".to_string(),
        content: "global".to_string(),
    };
    let project_memory = MemorySource {
        path: "/home/user/.kuku/p/workspace/memory.md".to_string(),
        content: "project".to_string(),
    };
    let tools = vec![ToolSchema {
        name: "read".to_string(),
        description: "Read a file".to_string(),
        input_schema: json!({"type": "object"}),
    }];

    let assembly = assemble_context(ContextInput {
        project_instructions: project_instructions.clone(),
        global_memory: Some(global_memory.clone()),
        project_memory: Some(project_memory.clone()),
        history: history.clone(),
        tools: tools.clone(),
    });

    assert_eq!(assembly.sources.len(), 5);
    assert!(matches!(
        assembly.sources[0],
        ContextSource::ProjectInstructions(_)
    ));
    assert!(matches!(
        assembly.sources[1],
        ContextSource::GlobalMemory(_)
    ));
    assert!(matches!(
        assembly.sources[2],
        ContextSource::ProjectMemory(_)
    ));
    assert!(matches!(assembly.sources[3], ContextSource::History(_)));
    assert!(matches!(assembly.sources[4], ContextSource::Tools(_)));

    let provenance = build_request_provenance(RequestProvenanceInput {
        request_id: "req_2".to_string(),
        role: "default".to_string(),
        workspace: "/workspace".to_string(),
        project_instruction_sources: vec![FileSource {
            path: "/workspace/AGENTS.md".to_string(),
            hash: "sha256-agents".to_string(),
        }],
        memory_sources: vec![FileSource {
            path: "/home/user/.kuku/memory.md".to_string(),
            hash: "sha256-memory".to_string(),
        }],
        prompt_asset_sources: Vec::new(),
        history_range: HistoryRange {
            first_event_id: Some(events.first().unwrap().id),
            last_event_id: Some(events.last().unwrap().id),
        },
        tool_registry: ToolRegistryProvenance {
            hash: "sha256-tools".to_string(),
            ordered_tool_names: vec!["read".to_string()],
            tool_count: 1,
        },
        provider_alias: "sonnet".to_string(),
        provider_format: "anthropic".to_string(),
        resolved_provider: "anthropic".to_string(),
        resolved_model: "claude-sonnet-4-6".to_string(),
        params: json!({"temperature": 0}),
        token_estimate: None,
    });

    assert_eq!(provenance.request_id, "req_2");
    assert_eq!(provenance.history_range.first_event_id, Some(1));
    assert_eq!(provenance.history_range.last_event_id, Some(2));
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
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
