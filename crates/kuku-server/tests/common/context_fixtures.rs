use kuku::event::{
    CapabilityFact, CapabilityKind, CapabilityState, ContextBreakdown, ConversationContextFact,
    ConversationId, DelegatedResultFact, InstructionContextFact, InstructionKind,
    MemoryContextFact, MemoryKind, ObservationFact, ObservationKind, ObservationRetention,
    ObservedRange, RequestId, RequestScope, SkillContextFact, SkillLoadOrigin, SourceFact,
    SourceScope, WorkspaceId, WorkspaceRelativePath,
};
use kuku_server::api::{
    AgentSummary, AgentThread, CapabilityProjection, ContextCatalog, ContextSections,
    ContextSnapshot, DelegatedAgentProjection, DelegatedAgentStatus, InstructionContextItem,
    MemoryContextItem, ObservationContextItem, SkillContextItem, TierSummary,
};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};

const CONTEXT_SNAPSHOT_FIXTURE: &str = include_str!("../fixtures/api/v1/context_snapshot.json");

fn fixture<T>() -> T
where
    T: DeserializeOwned,
{
    serde_json::from_str(CONTEXT_SNAPSHOT_FIXTURE).expect("context snapshot fixture is valid")
}

fn parse<T>(value: Value) -> T
where
    T: DeserializeOwned,
{
    serde_json::from_value(value).expect("context fixture value is valid")
}

fn source(path: &str) -> SourceFact {
    SourceFact {
        scope: SourceScope::Project,
        id: "source:project".to_string(),
        relative_path: Some(WorkspaceRelativePath::parse(path).unwrap()),
    }
}

fn request_scope() -> RequestScope {
    RequestScope {
        execution: kuku::event::ExecutionScope {
            workspace_id: WorkspaceId::parse("wsp_000000000000000000000001").unwrap(),
            task_id: kuku::event::TaskId::parse("tsk_000000000000000000000001").unwrap(),
            run_id: kuku::event::RunId::parse("run_000000000000000000000001").unwrap(),
            turn_id: kuku::event::TurnId::parse("trn_000000000000000000000001").unwrap(),
            conversation_id: ConversationId::parse("con_000000000000000000000001").unwrap(),
            turn_index: 1,
        },
        request_id: RequestId::parse("req_000000000000000000000001").unwrap(),
    }
}

/// Returns the empty, unconfigured Context snapshot from the G1 wire fixture.
pub fn empty_snapshot() -> ContextSnapshot {
    fixture()
}

/// Returns SDK-owned context facts with deterministic source and request identities.
pub fn sdk_context_breakdown() -> ContextBreakdown {
    let scope = request_scope();
    ContextBreakdown {
        skills: vec![SkillContextFact {
            skill_id: "skill:project:tdd".to_string(),
            source: source(".kuku/skills/tdd/SKILL.md"),
            origin: SkillLoadOrigin::Project,
            content_hash: "sha256:skill".to_string(),
        }],
        instructions: vec![InstructionContextFact {
            kind: InstructionKind::Project,
            source: source("AGENTS.md"),
            content_hash: "sha256:instructions".to_string(),
        }],
        memory: vec![MemoryContextFact {
            kind: MemoryKind::Project,
            source: source("memory.md"),
            content_hash: "sha256:memory".to_string(),
        }],
        conversation: ConversationContextFact {
            retained_turns: 2,
            handoff_boundaries: 1,
            history_summarized: false,
            delegated_results: vec![ConversationId::parse("con_000000000000000000000002").unwrap()],
        },
        observations: vec![ObservationFact {
            scope,
            tool_call_id: "call_read_file".to_string(),
            kind: ObservationKind::FileRead,
            relative_path: Some(WorkspaceRelativePath::parse("src/lib.rs").unwrap()),
            observed_hash: Some("sha256:observed".to_string()),
            range: Some(ObservedRange {
                start_line: 1,
                end_line: 12,
            }),
            retention: ObservationRetention::Retained,
            summary: "Read the module entrypoint".to_string(),
        }],
        delegated_results: vec![DelegatedResultFact {
            conversation_id: ConversationId::parse("con_000000000000000000000002").unwrap(),
            agent_id: "agent:project:review".to_string(),
            content_hash: "sha256:agent-result".to_string(),
        }],
        capabilities: vec![CapabilityFact {
            kind: CapabilityKind::FileRead,
            state: CapabilityState::Available,
        }],
        token_estimate: Some(1_024),
    }
}

/// Returns a bounded snapshot carrying loaded Skills, provenance, and an observation.
pub fn snapshot_with_context_evidence() -> ContextSnapshot {
    let facts = sdk_context_breakdown();
    let mut wire = serde_json::to_value(empty_snapshot()).unwrap();
    wire["next_request_base"] = serde_json::to_value(&facts).unwrap();
    wire["sections"] = json!(ContextSections {
        skills: vec![SkillContextItem {
            skill_id: "skill:project:tdd".to_string(),
            name: "TDD".to_string(),
            description: "Test-first implementation".to_string(),
            source: source(".kuku/skills/tdd/SKILL.md"),
            origin: SkillLoadOrigin::Project,
            content_hash: "sha256:skill".to_string(),
        }],
        instructions: vec![InstructionContextItem {
            kind: InstructionKind::Project,
            source: source("AGENTS.md"),
            content_hash: "sha256:instructions".to_string(),
            label: "Project instructions".to_string(),
        }],
        memory: vec![MemoryContextItem {
            kind: MemoryKind::Project,
            source: source("memory.md"),
            content_hash: "sha256:memory".to_string(),
            label: "Project memory".to_string(),
        }],
        conversation: kuku_server::api::ConversationContext {
            retained_turns: 2,
            handoff_boundaries: 1,
            history_summarized: false,
            delegated_results: vec![ConversationId::parse("con_000000000000000000000002").unwrap()],
        },
        observations: vec![ObservationContextItem {
            request_id: RequestId::parse("req_000000000000000000000001").unwrap(),
            tool_call_id: "call_read_file".to_string(),
            kind: ObservationKind::FileRead,
            relative_path: Some(WorkspaceRelativePath::parse("src/lib.rs").unwrap()),
            retention: ObservationRetention::Retained,
            summary: "Read the module entrypoint".to_string(),
        }],
        agents: vec![DelegatedAgentProjection {
            conversation_id: ConversationId::parse("con_000000000000000000000002").unwrap(),
            agent: AgentSummary {
                agent_id: "agent:project:review".to_string(),
                name: "Review Agent".to_string(),
                description: "Reviews project changes".to_string(),
            },
            tier: TierSummary {
                tier_id: "balanced".to_string(),
                label: "Balanced".to_string(),
                purpose: "General work".to_string(),
                provider: "fixture-provider".to_string(),
                model: "fixture-model".to_string(),
                think: None,
                is_default: true,
            },
            status: DelegatedAgentStatus::Completed,
            result_in_main: true,
        }],
        capabilities: vec![CapabilityProjection {
            kind: CapabilityKind::FileRead,
            state: CapabilityState::Available,
        }],
    });
    parse(wire)
}

/// Returns a historical snapshot with one completed Request selected.
pub fn historical_snapshot() -> ContextSnapshot {
    let mut wire = serde_json::to_value(snapshot_with_context_evidence()).unwrap();
    let request = json!({
        "request_id": "req_000000000000000000000001",
        "run_id": "run_000000000000000000000001",
        "turn_id": "trn_000000000000000000000001",
        "conversation_id": "con_000000000000000000000001",
        "status": "completed",
        "cause": {"kind": "user_submission"},
        "provider": {"kind": "anthropic"},
        "model": "fixture-model",
        "started_at": "2026-07-20T00:00:00Z"
    });
    wire["selected_request"] = request.clone();
    wire["request_history"] = json!([request]);
    wire["exact_request"] = json!({
        "messages": [],
        "tools": [],
        "parameters": {
            "model": "fixture-model",
            "max_output_tokens": null,
            "temperature": null,
            "stream": true,
            "thinking": {"kind": "disabled"}
        }
    });
    parse(wire)
}

/// Returns a deterministic Agent thread fixture for read-only context views.
pub fn agent_thread() -> AgentThread {
    parse(json!({
        "api_version": 1,
        "task_id": "tsk_000000000000000000000001",
        "conversation_id": "con_000000000000000000000002",
        "agent": {
            "agent_id": "agent:project:review",
            "name": "Review Agent",
            "description": "Reviews project changes"
        },
        "tier": {
            "tier_id": "balanced",
            "label": "Balanced",
            "purpose": "General work",
            "provider": "fixture-provider",
            "model": "fixture-model",
            "think": null,
            "is_default": true
        },
        "status": "completed",
        "result_in_main": true,
        "messages": [],
        "messages_truncated_before": false
    }))
}

/// Returns a deterministic catalog containing the Skill and Agent shown above.
pub fn context_catalog() -> ContextCatalog {
    parse(json!({
        "api_version": 1,
        "revision": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "tiers": [{"tier": {
            "tier_id": "balanced",
            "label": "Balanced",
            "purpose": "General work",
            "provider": "fixture-provider",
            "model": "fixture-model",
            "think": null,
            "is_default": true
        }}],
        "skills": [{
            "skill_id": "skill:project:tdd",
            "name": "TDD",
            "description": "Test-first implementation",
            "source": {"scope": "project", "id": "source:project", "relative_path": ".kuku/skills/tdd/SKILL.md"}
        }],
        "agents": [{"agent": {
            "agent_id": "agent:project:review",
            "name": "Review Agent",
            "description": "Reviews project changes"
        }, "tier_id": "balanced"}],
        "tools": [{"tool_id": "read_file", "name": "Read file", "description": "Read a workspace file"}]
    }))
}

/// Ensures a fixture remains serializable without private transport data.
pub fn assert_public_fixture<T>(value: &T)
where
    T: Serialize,
{
    let wire = serde_json::to_string(value).unwrap();
    for forbidden in [
        "credential",
        "raw_events",
        "absolute_path",
        "provider-secret",
    ] {
        assert!(
            !wire.contains(forbidden),
            "fixture contains forbidden field {forbidden}"
        );
    }
}
