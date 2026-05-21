use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::context::ToolSchema;

use super::builtin;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub read_only: bool,
    pub max_result_chars: usize,
    pub risk: String,
}

pub(crate) fn builtin_registry(agent_enabled: bool, skills_enabled: bool) -> Vec<ToolDefinition> {
    let mut tools = vec![
        tool(
            "find_files",
            "Browse the file tree — prefer this over shell commands for listing files. Directories shown with trailing /. Use pattern to filter, max_depth to limit recursion. Excludes build/dependency directories by default.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Search root relative to the workspace. Defaults to the workspace root."},
                    "pattern": {"type": "string", "description": "File glob pattern, e.g. *.md or docs/**/*.md."},
                    "max_depth": {"type": "integer", "description": "Maximum recursion depth (default: unlimited)."}
                }
            }),
            true,
            8_000,
            "read",
        ),
        tool(
            "read_file",
            "Read a file from the workspace with line numbers. Use offset and limit for pagination.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path relative to the workspace."},
                    "offset": {"type": "integer", "description": "Start line for pagination."},
                    "limit": {"type": "integer", "description": "Number of lines to read."}
                },
                "required": ["path"]
            }),
            true,
            80_000,
            "read",
        ),
        tool(
            "search_text",
            "Search file contents with a regex pattern. Use view=files, lines, or count.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Regular expression pattern to search for."},
                    "path": {"type": "string", "description": "Search scope relative to the workspace. Defaults to workspace root."},
                    "include": {"type": "string", "description": "File-name glob filter."},
                    "view": {"type": "string", "enum": ["files", "lines", "count"], "description": "Result view. Defaults to files."}
                },
                "required": ["pattern"]
            }),
            true,
            80_000,
            "read",
        ),
        tool(
            "edit_file",
            "Make a precise replacement in an existing file. The file must have been read in the current session before editing.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path relative to the workspace."},
                    "old_text": {"type": "string", "description": "Exact text to replace."},
                    "new_text": {"type": "string", "description": "Replacement text."},
                    "replace_all": {"type": "boolean", "description": "Replace all occurrences. Defaults to false."},
                    "brief": {"type": "string", "description": "What this edit does, in 3-5 words."}
                },
                "required": ["path", "old_text", "new_text", "brief"]
            }),
            false,
            20_000,
            "edit",
        ),
        tool(
            "write_file",
            "Write a complete file. Creates new files or overwrites existing files after a full prior read snapshot.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path relative to the workspace."},
                    "content": {"type": "string", "description": "Complete file content."},
                    "brief": {"type": "string", "description": "What this file is for, in 3-5 words."}
                },
                "required": ["path", "content", "brief"]
            }),
            false,
            20_000,
            "edit",
        ),
        tool(
            "remember_memory",
            "Append one natural-language bullet to global or project memory under a supported section.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "scope": {"type": "string", "enum": ["global", "project"], "description": "Which memory file to update."},
                    "kind": {"type": "string", "enum": ["how_to_work", "what_is_true", "where_to_look"], "description": "Which memory section to append to."},
                    "text": {"type": "string", "description": "Natural-language bullet text without the leading '- '."}
                },
                "required": ["scope", "kind", "text"]
            }),
            false,
            20_000,
            "edit",
        ),
        tool(
            "forget_memory",
            "Remove exactly one matching bullet from global or project memory by exact text.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "scope": {"type": "string", "enum": ["global", "project"], "description": "Which memory file to update."},
                    "text": {"type": "string", "description": "Exact natural-language bullet text to remove, without the leading '- '."}
                },
                "required": ["scope", "text"]
            }),
            false,
            20_000,
            "edit",
        ),
        tool(
            "run_command",
            "Run a local command with the workspace as cwd. timeout is required in seconds.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "The command string to execute."},
                    "timeout": {"type": "integer", "description": "Timeout in seconds."},
                    "brief": {"type": "string", "description": "What this command does, in 3-5 words."}
                },
                "required": ["command", "timeout", "brief"]
            }),
            false,
            80_000,
            "command",
        ),
    ];
    if agent_enabled {
        tools.push(builtin::agent_definition());
    }
    if skills_enabled {
        tools.push(builtin::use_skill_definition());
    }
    tools
}

pub(crate) fn registry_hash(registry: &[ToolDefinition]) -> String {
    let canonical = serde_json::to_vec(registry).expect("tool registry serializes");
    let digest = Sha256::digest(canonical);
    format!("sha256:{digest:x}")
}

pub(crate) fn tool_names(registry: &[ToolDefinition]) -> Vec<String> {
    registry.iter().map(|tool| tool.name.clone()).collect()
}

pub(crate) fn to_tool_schemas(registry: &[ToolDefinition]) -> Vec<ToolSchema> {
    registry
        .iter()
        .map(|tool| ToolSchema {
            name: tool.name.clone(),
            description: tool.description.clone(),
            input_schema: tool.input_schema.clone(),
        })
        .collect()
}

fn tool(
    name: &str,
    description: &str,
    input_schema: Value,
    read_only: bool,
    max_result_chars: usize,
    risk: &str,
) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: description.to_string(),
        input_schema,
        read_only,
        max_result_chars,
        risk: risk.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{builtin_registry, registry_hash, tool_names};

    #[test]
    fn builtin_registry_matches_documented_public_tool_surface() {
        let registry = builtin_registry(false, false);

        assert_eq!(
            tool_names(&registry),
            vec![
                "find_files",
                "read_file",
                "search_text",
                "edit_file",
                "write_file",
                "remember_memory",
                "forget_memory",
                "run_command",
            ]
        );
        assert_eq!(registry[0].risk, "read");
        assert!(registry[0].read_only);
        assert_eq!(registry[0].max_result_chars, 8_000);
        assert_eq!(
            registry_hash(&registry),
            registry_hash(&builtin_registry(false, false))
        );

        let remember = registry
            .iter()
            .find(|tool| tool.name == "remember_memory")
            .expect("remember_memory registered");
        assert!(!remember.read_only);
        assert_eq!(remember.risk, "edit");

        let forget = registry
            .iter()
            .find(|tool| tool.name == "forget_memory")
            .expect("forget_memory registered");
        assert!(!forget.read_only);
        assert_eq!(forget.risk, "edit");
    }

    #[test]
    fn builtin_registry_includes_agent_when_enabled() {
        let registry = builtin_registry(true, false);
        let names = tool_names(&registry);
        assert!(names.contains(&"agent".to_string()));
        assert_eq!(names.len(), 9);
        assert_eq!(names.last().unwrap(), "agent");
    }

    #[test]
    fn builtin_registry_excludes_agent_when_disabled() {
        let registry = builtin_registry(false, false);
        let names = tool_names(&registry);
        assert!(!names.contains(&"agent".to_string()));
        assert_eq!(names.len(), 8);
    }

    #[test]
    fn builtin_registry_includes_use_skill_when_enabled() {
        let registry = builtin_registry(false, true);
        let names = tool_names(&registry);
        assert!(names.contains(&"use_skill".to_string()));
        assert_eq!(names.len(), 9);
        assert_eq!(names.last().unwrap(), "use_skill");
    }

    #[test]
    fn builtin_registry_excludes_use_skill_when_disabled() {
        let registry = builtin_registry(false, false);
        let names = tool_names(&registry);
        assert!(!names.contains(&"use_skill".to_string()));
    }
}
