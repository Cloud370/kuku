use kuku::event::{
    CapabilityFact, CapabilityKind, CapabilityState, ContextBreakdown, ConversationContextFact,
    ConversationId, DelegatedResultFact, ExactContentBlock, ExactMessage, ExactRequest,
    ExactRequestParameters, ExactTool, ExecutionScope, InstructionContextFact, InstructionKind,
    MemoryContextFact, MemoryKind, MessageRole, ObservationFact, ObservationKind,
    ObservationRetention, ObservedRange, ProviderFact, RequestCause, RequestId, RequestScope,
    RequestSnapshot, RevisionToken, RunId, SkillContextFact, SkillLoadFact, SkillLoadOrigin,
    SourceFact, SourceScope, TaskId, ThinkingConfig, ToolResultStatus, TurnId, WorkspaceId,
    WorkspaceRelativePath, JSON_SAFE_INTEGER_MAX, MAX_WORKSPACE_RELATIVE_PATH_BYTES,
};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::Serialize;

fn execution_scope() -> ExecutionScope {
    ExecutionScope {
        workspace_id: WorkspaceId::parse("wsp_0123456789abcdef01234567").unwrap(),
        task_id: TaskId::parse("tsk_0123456789abcdef01234567").unwrap(),
        run_id: RunId::parse("run_0123456789abcdef01234567").unwrap(),
        turn_id: TurnId::parse("trn_0123456789abcdef01234567").unwrap(),
        conversation_id: ConversationId::parse("con_0123456789abcdef01234567").unwrap(),
        turn_index: 7,
    }
}

fn request_scope() -> RequestScope {
    RequestScope {
        execution: execution_scope(),
        request_id: RequestId::parse("req_0123456789abcdef01234567").unwrap(),
    }
}

fn source(scope: SourceScope, path: Option<&str>) -> SourceFact {
    SourceFact {
        scope,
        id: "source:fixture".to_string(),
        relative_path: path.map(|value| WorkspaceRelativePath::parse(value).unwrap()),
    }
}

fn assert_round_trip<T>(value: &T)
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let wire = serde_json::to_value(value).unwrap();
    let decoded = serde_json::from_value(wire).unwrap();
    assert_eq!(*value, decoded);
}

fn assert_has_schema<T: JsonSchema>() {
    let schema = serde_json::to_value(schemars::schema_for!(T)).unwrap();
    assert_eq!(
        schema["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
}

#[test]
fn exact_request_preserves_message_content_and_tool_order() {
    let exact = ExactRequest {
        messages: vec![
            ExactMessage {
                role: MessageRole::System,
                content: vec![ExactContentBlock::Text {
                    text: "system-secret".to_string(),
                }],
            },
            ExactMessage {
                role: MessageRole::User,
                content: vec![
                    ExactContentBlock::Text {
                        text: "first".to_string(),
                    },
                    ExactContentBlock::ToolUse {
                        tool_call_id: "call-1".to_string(),
                        name: "read_file".to_string(),
                        input: serde_json::json!({"path": "src/lib.rs"}),
                    },
                    ExactContentBlock::ToolResult {
                        tool_call_id: "call-1".to_string(),
                        status: ToolResultStatus::Completed,
                        content: "second".to_string(),
                        structured: None,
                        truncated: false,
                    },
                ],
            },
        ],
        tools: vec![
            ExactTool {
                name: "read_file".to_string(),
                description: "first tool".to_string(),
                input_schema: serde_json::json!({"type": "object"}),
            },
            ExactTool {
                name: "run_command".to_string(),
                description: "second tool".to_string(),
                input_schema: serde_json::json!({"type": "object"}),
            },
        ],
        parameters: ExactRequestParameters {
            model: "model-a".to_string(),
            max_output_tokens: None,
            temperature: Some(0.2),
            stream: true,
            thinking: ThinkingConfig::Enabled {
                budget_tokens: Some(1_024),
            },
        },
    };

    let wire = serde_json::to_value(&exact).unwrap();
    assert_eq!(wire["messages"][0]["role"], "system");
    assert_eq!(wire["messages"][1]["content"][0]["text"], "first");
    assert_eq!(wire["messages"][1]["content"][1]["kind"], "tool_use");
    assert_eq!(wire["messages"][1]["content"][2]["kind"], "tool_result");
    assert_eq!(wire["tools"][0]["name"], "read_file");
    assert_eq!(wire["tools"][1]["name"], "run_command");
    assert!(wire["parameters"]["max_output_tokens"].is_null());
    assert_eq!(wire["parameters"]["thinking"]["kind"], "enabled");
    assert_round_trip(&exact);
}

#[test]
fn request_snapshot_round_trips_all_context_sections() {
    let observation = ObservationFact {
        scope: request_scope(),
        tool_call_id: "call-2".to_string(),
        kind: ObservationKind::Search {
            query: "needle".to_string(),
        },
        relative_path: Some(WorkspaceRelativePath::parse("src").unwrap()),
        observed_hash: Some("sha256:observed".to_string()),
        range: Some(ObservedRange {
            start_line: 3,
            end_line: 9,
        }),
        retention: ObservationRetention::Summarized,
        summary: "sensitive observation".to_string(),
    };
    let context = ContextBreakdown {
        skills: vec![SkillContextFact {
            skill_id: "skill:project:tdd".to_string(),
            source: source(SourceScope::Project, Some(".kuku/skills/tdd/SKILL.md")),
            origin: SkillLoadOrigin::You,
            content_hash: "sha256:skill".to_string(),
        }],
        instructions: vec![InstructionContextFact {
            kind: InstructionKind::Project,
            source: source(SourceScope::Project, Some("AGENTS.md")),
            content_hash: "sha256:instructions".to_string(),
        }],
        memory: vec![MemoryContextFact {
            kind: MemoryKind::Project,
            source: source(SourceScope::Project, Some("memory.md")),
            content_hash: "sha256:memory".to_string(),
        }],
        conversation: ConversationContextFact {
            retained_turns: 4,
            handoff_boundaries: 1,
            history_summarized: true,
            delegated_results: vec![ConversationId::parse("con_89abcdef0123456701234567").unwrap()],
        },
        observations: vec![observation],
        delegated_results: vec![DelegatedResultFact {
            conversation_id: ConversationId::parse("con_89abcdef0123456701234567").unwrap(),
            agent_id: "agent:project:review".to_string(),
            content_hash: "sha256:agent-result".to_string(),
        }],
        capabilities: vec![CapabilityFact {
            kind: CapabilityKind::FileRead,
            state: CapabilityState::Available,
        }],
        token_estimate: None,
    };
    let snapshot = RequestSnapshot {
        scope: request_scope(),
        cause: RequestCause::UserSubmission,
        provider: ProviderFact::Anthropic,
        tier_id: "tier:default".to_string(),
        exact: ExactRequest {
            messages: vec![],
            tools: vec![],
            parameters: ExactRequestParameters {
                model: "model-a".to_string(),
                max_output_tokens: None,
                temperature: None,
                stream: true,
                thinking: ThinkingConfig::Disabled,
            },
        },
        context,
        catalog_revision: RevisionToken::parse(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .unwrap(),
        exact_payload_hash: "sha256:exact".to_string(),
    };

    let wire = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(wire["context"]["skills"][0]["origin"], "you");
    assert_eq!(wire["context"]["observations"][0]["kind"]["kind"], "search");
    assert!(wire["context"]["token_estimate"].is_null());
    assert_round_trip(&snapshot);
}

#[test]
fn durable_skill_and_observation_facts_cover_all_tagged_variants() {
    let skill = SkillLoadFact {
        execution: execution_scope(),
        caused_by_request_id: None,
        skill_id: "skill:project:tdd".to_string(),
        source: source(SourceScope::Project, Some(".kuku/skills/tdd/SKILL.md")),
        origin: SkillLoadOrigin::Agent,
        content_hash: "sha256:skill".to_string(),
    };
    assert_round_trip(&skill);

    let observation_kinds = [
        ObservationKind::FileRead,
        ObservationKind::FileList,
        ObservationKind::Search {
            query: "needle".to_string(),
        },
        ObservationKind::Command {
            command: "cargo test".to_string(),
            exit_code: Some(0),
        },
        ObservationKind::Tool {
            name: "custom".to_string(),
        },
    ];
    let expected = ["file_read", "file_list", "search", "command", "tool"];
    for (kind, expected_kind) in observation_kinds.iter().zip(expected) {
        let wire = serde_json::to_value(kind).unwrap();
        assert_eq!(wire["kind"], expected_kind);
        assert_round_trip(kind);
    }

    let blocks = [
        ExactContentBlock::Text {
            text: "text".to_string(),
        },
        ExactContentBlock::Thinking {
            text: "thinking".to_string(),
        },
        ExactContentBlock::ToolUse {
            tool_call_id: "call".to_string(),
            name: "tool".to_string(),
            input: serde_json::Value::Null,
        },
        ExactContentBlock::ToolResult {
            tool_call_id: "call".to_string(),
            status: ToolResultStatus::Failed,
            content: "result".to_string(),
            structured: Some(serde_json::json!({"ok": false})),
            truncated: true,
        },
    ];
    let expected = ["text", "thinking", "tool_use", "tool_result"];
    for (block, expected_kind) in blocks.iter().zip(expected) {
        let wire = serde_json::to_value(block).unwrap();
        assert_eq!(wire["kind"], expected_kind);
        assert_round_trip(block);
    }

    let thinking = [
        ThinkingConfig::Disabled,
        ThinkingConfig::Adaptive,
        ThinkingConfig::Enabled {
            budget_tokens: None,
        },
    ];
    let expected = ["disabled", "adaptive", "enabled"];
    for (value, expected_kind) in thinking.iter().zip(expected) {
        let wire = serde_json::to_value(value).unwrap();
        assert_eq!(wire["kind"], expected_kind);
        assert_round_trip(value);
    }
}

#[test]
fn closed_categorical_values_round_trip_without_open_strings() {
    for value in [
        serde_json::to_value(MessageRole::System).unwrap(),
        serde_json::to_value(MessageRole::User).unwrap(),
        serde_json::to_value(MessageRole::Assistant).unwrap(),
        serde_json::to_value(MessageRole::Tool).unwrap(),
        serde_json::to_value(SkillLoadOrigin::You).unwrap(),
        serde_json::to_value(SkillLoadOrigin::Agent).unwrap(),
        serde_json::to_value(SkillLoadOrigin::Bootstrap).unwrap(),
        serde_json::to_value(SkillLoadOrigin::Project).unwrap(),
        serde_json::to_value(ObservationRetention::Retained).unwrap(),
        serde_json::to_value(ObservationRetention::Summarized).unwrap(),
        serde_json::to_value(ObservationRetention::Truncated).unwrap(),
    ] {
        assert!(value.is_string());
    }

    for scope in [
        SourceScope::System,
        SourceScope::User,
        SourceScope::Project,
        SourceScope::Workspace,
        SourceScope::Agent,
    ] {
        assert_round_trip(&scope);
    }
    for kind in [
        InstructionKind::System,
        InstructionKind::Project,
        InstructionKind::Workspace,
        InstructionKind::Agent,
    ] {
        assert_round_trip(&kind);
    }
    for kind in [MemoryKind::Global, MemoryKind::Project] {
        assert_round_trip(&kind);
    }
    for kind in [
        CapabilityKind::FileRead,
        CapabilityKind::FileWrite,
        CapabilityKind::CommandExecution,
        CapabilityKind::NetworkAccess,
        CapabilityKind::AgentDelegation,
        CapabilityKind::SkillDiscovery,
        CapabilityKind::Memory,
    ] {
        assert_round_trip(&kind);
    }
    for state in [
        CapabilityState::Available,
        CapabilityState::Unavailable,
        CapabilityState::RequiresApproval,
    ] {
        assert_round_trip(&state);
    }
}

#[test]
fn workspace_relative_path_is_normalized_contained_and_size_bounded() {
    let path = WorkspaceRelativePath::parse("src/event/types/context.rs").unwrap();
    assert_eq!(path.as_str(), "src/event/types/context.rs");
    assert_round_trip(&path);

    assert!(WorkspaceRelativePath::parse("").is_err());
    assert!(WorkspaceRelativePath::parse("/etc/passwd").is_err());
    assert!(WorkspaceRelativePath::parse("../secret").is_err());
    assert!(WorkspaceRelativePath::parse("src/../secret").is_err());
    assert!(WorkspaceRelativePath::parse("./src/lib.rs").is_err());
    assert!(WorkspaceRelativePath::parse("src\\lib.rs").is_err());
    assert!(WorkspaceRelativePath::parse("C:\\secret").is_err());

    let at_limit = "a".repeat(MAX_WORKSPACE_RELATIVE_PATH_BYTES);
    assert!(WorkspaceRelativePath::parse(&at_limit).is_ok());
    assert!(WorkspaceRelativePath::parse(format!("{at_limit}a")).is_err());
}

#[test]
fn context_metrics_reject_values_above_json_safe_integer_max() {
    let too_large = JSON_SAFE_INTEGER_MAX + 1;
    let parameters = serde_json::json!({
        "model": "model-a",
        "max_output_tokens": too_large,
        "temperature": null,
        "stream": true,
        "thinking": {"kind": "disabled"}
    });
    assert!(serde_json::from_value::<ExactRequestParameters>(parameters).is_err());

    let conversation = serde_json::json!({
        "retained_turns": too_large,
        "handoff_boundaries": 0,
        "history_summarized": false,
        "delegated_results": []
    });
    assert!(serde_json::from_value::<ConversationContextFact>(conversation).is_err());

    let range = serde_json::json!({"start_line": 1, "end_line": too_large});
    assert!(serde_json::from_value::<ObservedRange>(range).is_err());
}

#[test]
fn sensitive_debug_never_renders_exact_or_observation_content() {
    let exact = ExactRequest {
        messages: vec![ExactMessage {
            role: MessageRole::User,
            content: vec![ExactContentBlock::Text {
                text: "provider-secret".to_string(),
            }],
        }],
        tools: vec![ExactTool {
            name: "tool".to_string(),
            description: "provider-secret-description".to_string(),
            input_schema: serde_json::json!({"secret": "provider-secret-schema"}),
        }],
        parameters: ExactRequestParameters {
            model: "model-a".to_string(),
            max_output_tokens: None,
            temperature: None,
            stream: true,
            thinking: ThinkingConfig::Disabled,
        },
    };
    let observation = ObservationFact {
        scope: request_scope(),
        tool_call_id: "call".to_string(),
        kind: ObservationKind::Command {
            command: "echo provider-secret".to_string(),
            exit_code: Some(0),
        },
        relative_path: None,
        observed_hash: None,
        range: None,
        retention: ObservationRetention::Retained,
        summary: "provider-secret-summary".to_string(),
    };

    for debug in [format!("{exact:?}"), format!("{observation:?}")] {
        assert!(!debug.contains("provider-secret"));
        assert!(debug.contains("<redacted>"));
    }
}

#[test]
fn every_context_fact_value_has_a_json_schema() {
    assert_has_schema::<WorkspaceRelativePath>();
    assert_has_schema::<SourceScope>();
    assert_has_schema::<SourceFact>();
    assert_has_schema::<MessageRole>();
    assert_has_schema::<ToolResultStatus>();
    assert_has_schema::<ExactContentBlock>();
    assert_has_schema::<ExactMessage>();
    assert_has_schema::<ExactTool>();
    assert_has_schema::<ThinkingConfig>();
    assert_has_schema::<ExactRequestParameters>();
    assert_has_schema::<ExactRequest>();
    assert_has_schema::<SkillLoadOrigin>();
    assert_has_schema::<SkillContextFact>();
    assert_has_schema::<InstructionKind>();
    assert_has_schema::<InstructionContextFact>();
    assert_has_schema::<MemoryKind>();
    assert_has_schema::<MemoryContextFact>();
    assert_has_schema::<ConversationContextFact>();
    assert_has_schema::<DelegatedResultFact>();
    assert_has_schema::<CapabilityKind>();
    assert_has_schema::<CapabilityState>();
    assert_has_schema::<CapabilityFact>();
    assert_has_schema::<ObservationKind>();
    assert_has_schema::<ObservationRetention>();
    assert_has_schema::<ObservedRange>();
    assert_has_schema::<ObservationFact>();
    assert_has_schema::<ContextBreakdown>();
    assert_has_schema::<RequestSnapshot>();
    assert_has_schema::<SkillLoadFact>();
}

#[test]
fn context_fact_module_has_no_server_builder_or_api_dependency() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/event/types/context.rs"
    ))
    .unwrap();
    for forbidden in [
        "kuku_server",
        "kuku-server",
        "axum",
        "crate::api",
        "ContextAssembly",
        "ContextInput",
        "RequestSnapshotBuilder",
        "EventStore",
    ] {
        assert!(!source.contains(forbidden), "forbidden import: {forbidden}");
    }
}
