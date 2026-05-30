use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
/// A file path and its content hash for provenance tracking.
pub struct FileSource {
    pub path: String,
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
/// Range of event IDs included in the conversation history.
pub struct HistoryRange {
    pub first_event_id: Option<u64>,
    pub last_event_id: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
/// Snapshot of the tool registry used for a provider request.
pub struct ToolRegistryProvenance {
    pub hash: String,
    pub names: Vec<String>,
    pub tool_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
/// Snapshot of the subagent registry used for agent catalog rendering.
pub struct SubagentRegistryProvenance {
    pub hash: String,
    pub names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
/// Snapshot of the skill registry used for skill catalog rendering.
pub struct SkillRegistryProvenance {
    pub hash: String,
    pub names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
/// Snapshot of the plugin registry used for a provider request.
pub struct PluginRegistryProvenance {
    pub hash: String,
    pub names: Vec<String>,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq)]
/// Inputs for building request provenance metadata.
pub struct RequestProvenanceInput {
    pub request_id: String,
    pub tier: String,
    pub workspace: String,
    pub platform: String,
    pub current_date: String,
    pub project_instruction_sources: Vec<FileSource>,
    pub memory_sources: Vec<FileSource>,
    pub prompt_asset_sources: Vec<FileSource>,
    pub history_range: HistoryRange,
    pub tool_registry: ToolRegistryProvenance,
    pub subagent_registry: Option<SubagentRegistryProvenance>,
    pub skill_registry: Option<SkillRegistryProvenance>,
    pub plugin_registry: Option<PluginRegistryProvenance>,
    pub provider_format: String,
    pub provider: String,
    pub model: String,
    pub request_params: Value,
    pub token_estimate: Option<u64>,
    pub context_budget_tier: String,
    pub max_context_tokens: Option<u32>,
    pub remaining_input_tokens: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
/// Captured provenance metadata for a provider request, stored in events.jsonl.
pub struct RequestProvenance {
    pub request_id: String,
    pub tier: String,
    pub workspace: String,
    pub platform: String,
    pub current_date: String,
    pub project_instruction_sources: Vec<FileSource>,
    pub memory_sources: Vec<FileSource>,
    pub prompt_asset_sources: Vec<FileSource>,
    pub history_range: HistoryRange,
    pub tool_registry: ToolRegistryProvenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_registry: Option<SubagentRegistryProvenance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_registry: Option<SkillRegistryProvenance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_registry: Option<PluginRegistryProvenance>,
    pub provider_format: String,
    pub provider: String,
    pub model: String,
    pub request_params: Value,
    pub token_estimate: Option<u64>,
    pub context_budget_tier: String,
    pub max_context_tokens: Option<u32>,
    pub remaining_input_tokens: Option<u32>,
}

/// Build request provenance from the given inputs.
pub fn build_request_provenance(input: RequestProvenanceInput) -> RequestProvenance {
    RequestProvenance {
        request_id: input.request_id,
        tier: input.tier,
        workspace: input.workspace,
        platform: input.platform,
        current_date: input.current_date,
        project_instruction_sources: input.project_instruction_sources,
        memory_sources: input.memory_sources,
        prompt_asset_sources: input.prompt_asset_sources,
        history_range: input.history_range,
        tool_registry: input.tool_registry,
        subagent_registry: input.subagent_registry,
        skill_registry: input.skill_registry,
        plugin_registry: input.plugin_registry,
        provider_format: input.provider_format,
        provider: input.provider,
        model: input.model,
        request_params: input.request_params,
        token_estimate: input.token_estimate,
        context_budget_tier: input.context_budget_tier,
        max_context_tokens: input.max_context_tokens,
        remaining_input_tokens: input.remaining_input_tokens,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        build_request_provenance, FileSource, HistoryRange, RequestProvenanceInput,
        SubagentRegistryProvenance, ToolRegistryProvenance,
    };

    fn source(path: &str, hash: &str) -> FileSource {
        FileSource {
            path: path.to_string(),
            hash: hash.to_string(),
        }
    }

    #[test]
    fn builds_structured_request_provenance_without_provider_raw_body_and_preserves_fields() {
        let project_sources = vec![
            source("/workspace/AGENTS.md", "sha256-agents"),
            source("/workspace/CLAUDE.md", "sha256-claude"),
        ];
        let memory_sources = vec![
            source("/home/user/.kuku/memory.md", "sha256-global-memory"),
            source(
                "/home/user/.kuku/p/workspace/memory.md",
                "sha256-project-memory",
            ),
        ];
        let prompt_sources = vec![source("prompt:system/base", "sha256-prompt")];
        let history_range = HistoryRange {
            first_event_id: Some(1),
            last_event_id: Some(42),
        };
        let tool_registry = ToolRegistryProvenance {
            hash: "sha256-tools".to_string(),
            names: vec!["read".to_string(), "grep".to_string()],
            tool_count: 2,
        };

        let provenance = build_request_provenance(RequestProvenanceInput {
            request_id: "req_1".to_string(),
            tier: "balanced".to_string(),
            workspace: "/workspace".to_string(),
            platform: "linux".to_string(),
            current_date: "2026-05-14".to_string(),
            project_instruction_sources: project_sources.clone(),
            memory_sources: memory_sources.clone(),
            prompt_asset_sources: prompt_sources.clone(),
            history_range: history_range.clone(),
            tool_registry: tool_registry.clone(),
            subagent_registry: None,
            skill_registry: None,
            provider_format: "anthropic".to_string(),
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            request_params: json!({"temperature": 0, "max_tokens": 1024}),
            token_estimate: Some(500),
            context_budget_tier: "roomy".to_string(),
            max_context_tokens: Some(200_000),
            remaining_input_tokens: Some(170_000),
        });

        let debug = format!("{provenance:?}");
        assert!(!debug.contains("provider_raw_body"));
        assert!(!debug.contains("provider_body"));
        assert!(!debug.contains("raw_body"));

        let super::RequestProvenance {
            request_id,
            tier,
            workspace,
            platform,
            current_date,
            project_instruction_sources: actual_project_sources,
            memory_sources: actual_memory_sources,
            prompt_asset_sources: actual_prompt_sources,
            history_range: actual_history_range,
            tool_registry: actual_tool_registry,
            subagent_registry: actual_subagent_registry,
            skill_registry: _,
            provider_format,
            provider,
            model,
            request_params,
            token_estimate,
            context_budget_tier,
            max_context_tokens,
            remaining_input_tokens,
        } = provenance;

        assert_eq!(request_id, "req_1");
        assert_eq!(tier, "balanced");
        assert_eq!(workspace, "/workspace");
        assert_eq!(platform, "linux");
        assert_eq!(current_date, "2026-05-14");
        assert_eq!(actual_project_sources, project_sources);
        assert_eq!(actual_memory_sources, memory_sources);
        assert_eq!(actual_prompt_sources, prompt_sources);
        assert_eq!(actual_history_range, history_range);
        assert_eq!(actual_tool_registry, tool_registry);
        assert_eq!(actual_subagent_registry, None);
        assert_eq!(provider_format, "anthropic");
        assert_eq!(provider, "anthropic");
        assert_eq!(model, "claude-sonnet-4-6");
        assert_eq!(
            request_params,
            json!({"temperature": 0, "max_tokens": 1024})
        );
        assert_eq!(token_estimate, Some(500));
        assert_eq!(context_budget_tier, "roomy");
        assert_eq!(max_context_tokens, Some(200_000));
        assert_eq!(remaining_input_tokens, Some(170_000));
    }

    #[test]
    fn provenance_serializes_subagent_registry_when_present() {
        let subagent = SubagentRegistryProvenance {
            hash: "sha256-subagent".to_string(),
            names: vec!["review".to_string(), "explore".to_string()],
        };
        let provenance = build_request_provenance(RequestProvenanceInput {
            request_id: "req_1".to_string(),
            tier: "balanced".to_string(),
            workspace: "/workspace".to_string(),
            platform: "linux".to_string(),
            current_date: "2026-05-18".to_string(),
            project_instruction_sources: vec![],
            memory_sources: vec![],
            prompt_asset_sources: vec![],
            history_range: HistoryRange {
                first_event_id: None,
                last_event_id: None,
            },
            tool_registry: ToolRegistryProvenance {
                hash: "sha256-tools".to_string(),
                names: vec![],
                tool_count: 0,
            },
            subagent_registry: Some(subagent),
            skill_registry: None,
            provider_format: "anthropic".to_string(),
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            request_params: json!({}),
            token_estimate: None,
            context_budget_tier: "normal".to_string(),
            max_context_tokens: None,
            remaining_input_tokens: None,
        });

        let json = serde_json::to_value(&provenance).unwrap();
        let sub = &json["subagent_registry"];
        assert_eq!(sub["hash"], "sha256-subagent");
        assert_eq!(sub["names"][0], "review");
        assert_eq!(sub["names"][1], "explore");

        // When subagent_registry is None, it should be absent from JSON.
        let provenance_none = build_request_provenance(RequestProvenanceInput {
            request_id: "req_2".to_string(),
            tier: "strong".to_string(),
            workspace: "/ws".to_string(),
            platform: "linux".to_string(),
            current_date: "2026-05-18".to_string(),
            project_instruction_sources: vec![],
            memory_sources: vec![],
            prompt_asset_sources: vec![],
            history_range: HistoryRange {
                first_event_id: None,
                last_event_id: None,
            },
            tool_registry: ToolRegistryProvenance {
                hash: "".to_string(),
                names: vec![],
                tool_count: 0,
            },
            subagent_registry: None,
            skill_registry: None,
            provider_format: "anthropic".to_string(),
            provider: "anthropic".to_string(),
            model: "model".to_string(),
            request_params: json!({}),
            token_estimate: None,
            context_budget_tier: "normal".to_string(),
            max_context_tokens: None,
            remaining_input_tokens: None,
        });
        let json_none = serde_json::to_value(&provenance_none).unwrap();
        assert!(json_none.get("subagent_registry").is_none());
    }
}
