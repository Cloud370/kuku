use serde::{Deserialize, Serialize};

/// Tool profile preset that determines which tools a subagent can use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolProfile {
    /// No tools — subagent can only read its own instruction and the delegated prompt.
    #[serde(rename = "none")]
    None,
    /// Read-only inspection: find_files, read_file, search_text.
    #[serde(rename = "read")]
    Read,
    /// Full read + write + command: all 8 built-in tools.
    /// Write operations go through permission gate; commands ask→deny in child sessions.
    #[serde(rename = "read_write")]
    ReadWrite,
}

impl ToolProfile {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Read => "read",
            Self::ReadWrite => "read_write",
        }
    }

    /// Returns the set of tool names allowed under this profile.
    pub fn allowed_tools(&self) -> &'static [&'static str] {
        match self {
            Self::None => &[],
            Self::Read => &["find_files", "read_file", "search_text"],
            Self::ReadWrite => &[
                "find_files",
                "read_file",
                "search_text",
                "edit_file",
                "write_file",
                "memory.remember",
                "memory.forget",
                "run_command",
            ],
        }
    }
}

/// Expected output format from a subagent. In v1 this is injected as a hint in the instructions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum OutputContract {
    /// Brief summary of work done and conclusions.
    #[serde(rename = "summary")]
    #[default]
    Summary,
    /// Structured findings with file/line evidence.
    #[serde(rename = "findings")]
    Findings,
    /// An implementation plan or design approach.
    #[serde(rename = "plan")]
    Plan,
    /// Handoff-formatted compressed context (future).
    #[serde(rename = "handoff")]
    Handoff,
}

impl OutputContract {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Summary => "summary",
            Self::Findings => "findings",
            Self::Plan => "plan",
            Self::Handoff => "handoff",
        }
    }

    /// v1: returns a hint string injected into the child's instructions.
    pub fn instruction_hint(&self) -> &'static str {
        match self {
            Self::Summary => "Produce a concise summary of your work and conclusions.",
            Self::Findings => {
                "Report your findings with specific file paths and line numbers as evidence."
            }
            Self::Plan => "Outline an implementation approach with key files and steps.",
            Self::Handoff => "Compress the relevant context into a handoff-ready summary.",
        }
    }
}

/// Maximum permission posture a subagent definition declares.
/// v1: reserved field, no runtime behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PermissionPosture {
    #[serde(rename = "default")]
    #[default]
    Default,
}

/// Where this subagent definition was loaded from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DefinitionSource {
    #[serde(rename = "builtin")]
    Builtin,
    #[serde(rename = "claude_code:user")]
    ClaudeCodeUser,
    #[serde(rename = "claude_code:project")]
    ClaudeCodeProject,
    #[serde(rename = "opencode:user")]
    OpenCodeUser,
    #[serde(rename = "opencode:project")]
    OpenCodeProject,
}

impl DefinitionSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::ClaudeCodeUser => "claude_code:user",
            Self::ClaudeCodeProject => "claude_code:project",
            Self::OpenCodeUser => "opencode:user",
            Self::OpenCodeProject => "opencode:project",
        }
    }
}

/// A subagent definition — the internal representation for any subagent,
/// whether built-in or imported from a compatibility source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentDefinition {
    /// Stable id for `agent` tool dispatch.
    pub name: String,
    /// Short capability summary for catalog selection.
    pub description: String,
    /// Full agent instructions (becomes user-message content in child session).
    pub instructions: String,
    /// Model capability tier: strong / balanced / light.
    pub tier: String,
    /// Tool allowlist preset.
    pub tool_profile: ToolProfile,
    /// Maximum permission stance (v1: reserved).
    #[serde(default)]
    pub permission: PermissionPosture,
    /// Hard turn limit for the child session.
    pub max_turns: u32,
    /// Expected output format.
    #[serde(default)]
    pub output_contract: OutputContract,
    /// Origin of this definition.
    pub source: DefinitionSource,
    /// Content hash for drift detection and snapshot pinning.
    pub hash: String,
    /// Original file path for compatibility imports, if applicable.
    #[serde(default)]
    pub source_path: Option<String>,
    /// Raw metadata from compatibility import (unknown/unsupported fields).
    #[serde(default)]
    pub metadata: serde_json::Value,
}

impl SubagentDefinition {
    /// Compute a deterministic hash from the definition's content fields.
    pub fn compute_hash(&self) -> String {
        use sha2::{Digest, Sha256};
        let canonical = format!(
            "{}|{}|{}|{}|{}|{}|{}",
            self.name,
            self.description,
            self.instructions,
            self.tier,
            self.tool_profile.as_str(),
            self.max_turns,
            self.output_contract.as_str()
        );
        let digest = Sha256::digest(canonical.as_bytes());
        format!("sha256:{digest:x}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_profile_allowed_tools_read() {
        let tools = ToolProfile::Read.allowed_tools();
        assert!(tools.contains(&"find_files"));
        assert!(tools.contains(&"read_file"));
        assert!(tools.contains(&"search_text"));
        assert!(!tools.contains(&"edit_file"));
        assert!(!tools.contains(&"run_command"));
    }

    #[test]
    fn tool_profile_allowed_tools_read_write() {
        let tools = ToolProfile::ReadWrite.allowed_tools();
        assert!(tools.contains(&"edit_file"));
        assert!(tools.contains(&"write_file"));
        assert!(tools.contains(&"run_command"));
        assert!(tools.contains(&"memory.remember"));
    }

    #[test]
    fn tool_profile_allowed_tools_none() {
        assert!(ToolProfile::None.allowed_tools().is_empty());
    }

    #[test]
    fn subagent_definition_hash_is_deterministic() {
        let def = SubagentDefinition {
            name: "review".into(),
            description: "Review code".into(),
            instructions: "Review carefully.".into(),
            tier: "balanced".into(),
            tool_profile: ToolProfile::Read,
            permission: PermissionPosture::Default,
            max_turns: 4,
            output_contract: OutputContract::Findings,
            source: DefinitionSource::Builtin,
            hash: String::new(),
            source_path: None,
            metadata: serde_json::Value::Null,
        };
        let hash = def.compute_hash();
        assert!(hash.starts_with("sha256:"));
        assert_eq!(hash, def.compute_hash(), "hash must be deterministic");
    }

    #[test]
    fn output_contract_instruction_hints_are_stable() {
        assert!(OutputContract::Summary
            .instruction_hint()
            .contains("concise summary"));
        assert!(OutputContract::Findings
            .instruction_hint()
            .contains("file paths"));
        assert!(OutputContract::Plan
            .instruction_hint()
            .contains("implementation approach"));
    }
}
