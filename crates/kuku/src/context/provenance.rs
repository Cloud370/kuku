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
    pub ordered_tool_names: Vec<String>,
    pub tool_count: usize,
}

#[derive(Debug, Clone, PartialEq)]
/// Inputs for building request provenance metadata.
pub struct RequestProvenanceInput {
    pub request_id: String,
    pub role: String,
    pub workspace: String,
    pub platform: String,
    pub current_date: String,
    pub project_instruction_sources: Vec<FileSource>,
    pub memory_sources: Vec<FileSource>,
    pub prompt_asset_sources: Vec<FileSource>,
    pub history_range: HistoryRange,
    pub tool_registry: ToolRegistryProvenance,
    pub provider_alias: String,
    pub provider_format: String,
    pub resolved_provider: String,
    pub resolved_model: String,
    pub params: Value,
    pub token_estimate: Option<u64>,
    pub context_budget_tier: String,
    pub max_context_tokens: Option<u32>,
    pub remaining_input_tokens: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
/// Captured provenance metadata for a provider request, stored in events.jsonl.
pub struct RequestProvenance {
    pub request_id: String,
    pub role: String,
    pub workspace: String,
    pub platform: String,
    pub current_date: String,
    pub project_instruction_sources: Vec<FileSource>,
    pub memory_sources: Vec<FileSource>,
    pub prompt_asset_sources: Vec<FileSource>,
    pub history_range: HistoryRange,
    pub tool_registry: ToolRegistryProvenance,
    pub provider_alias: String,
    pub provider_format: String,
    pub resolved_provider: String,
    pub resolved_model: String,
    pub params: Value,
    pub token_estimate: Option<u64>,
    pub context_budget_tier: String,
    pub max_context_tokens: Option<u32>,
    pub remaining_input_tokens: Option<u32>,
}

/// Build request provenance from the given inputs.
pub fn build_request_provenance(input: RequestProvenanceInput) -> RequestProvenance {
    RequestProvenance {
        request_id: input.request_id,
        role: input.role,
        workspace: input.workspace,
        platform: input.platform,
        current_date: input.current_date,
        project_instruction_sources: input.project_instruction_sources,
        memory_sources: input.memory_sources,
        prompt_asset_sources: input.prompt_asset_sources,
        history_range: input.history_range,
        tool_registry: input.tool_registry,
        provider_alias: input.provider_alias,
        provider_format: input.provider_format,
        resolved_provider: input.resolved_provider,
        resolved_model: input.resolved_model,
        params: input.params,
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
        ToolRegistryProvenance,
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
            ordered_tool_names: vec!["read".to_string(), "grep".to_string()],
            tool_count: 2,
        };

        let provenance = build_request_provenance(RequestProvenanceInput {
            request_id: "req_1".to_string(),
            role: "default".to_string(),
            workspace: "/workspace".to_string(),
            platform: "linux".to_string(),
            current_date: "2026-05-14".to_string(),
            project_instruction_sources: project_sources.clone(),
            memory_sources: memory_sources.clone(),
            prompt_asset_sources: prompt_sources.clone(),
            history_range: history_range.clone(),
            tool_registry: tool_registry.clone(),
            provider_alias: "sonnet".to_string(),
            provider_format: "anthropic".to_string(),
            resolved_provider: "anthropic".to_string(),
            resolved_model: "claude-sonnet-4-6".to_string(),
            params: json!({"temperature": 0, "max_tokens": 1024}),
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
            role,
            workspace,
            platform,
            current_date,
            project_instruction_sources: actual_project_sources,
            memory_sources: actual_memory_sources,
            prompt_asset_sources: actual_prompt_sources,
            history_range: actual_history_range,
            tool_registry: actual_tool_registry,
            provider_alias,
            provider_format,
            resolved_provider,
            resolved_model,
            params,
            token_estimate,
            context_budget_tier,
            max_context_tokens,
            remaining_input_tokens,
        } = provenance;

        assert_eq!(request_id, "req_1");
        assert_eq!(role, "default");
        assert_eq!(workspace, "/workspace");
        assert_eq!(platform, "linux");
        assert_eq!(current_date, "2026-05-14");
        assert_eq!(actual_project_sources, project_sources);
        assert_eq!(actual_memory_sources, memory_sources);
        assert_eq!(actual_prompt_sources, prompt_sources);
        assert_eq!(actual_history_range, history_range);
        assert_eq!(actual_tool_registry, tool_registry);
        assert_eq!(provider_alias, "sonnet");
        assert_eq!(provider_format, "anthropic");
        assert_eq!(resolved_provider, "anthropic");
        assert_eq!(resolved_model, "claude-sonnet-4-6");
        assert_eq!(params, json!({"temperature": 0, "max_tokens": 1024}));
        assert_eq!(token_estimate, Some(500));
        assert_eq!(context_budget_tier, "roomy");
        assert_eq!(max_context_tokens, Some(200_000));
        assert_eq!(remaining_input_tokens, Some(170_000));
    }
}
