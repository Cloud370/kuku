use serde::{Deserialize, Serialize};

/// Tool profile preset that determines which tools a subagent can use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ToolProfile {
    /// No tools — subagent can only read its own instruction and the delegated prompt.
    #[serde(rename = "none")]
    #[default]
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
            Self::Read => &["find_files", "read_file", "search_text", "fetch_url", "fetch_web"],
            Self::ReadWrite => &[
                "find_files",
                "read_file",
                "search_text",
                "fetch_url",
                "fetch_web",
                "edit_file",
                "write_file",
                "remember_memory",
                "forget_memory",
                "run_command",
            ],
        }
    }
}

/// Where this subagent definition was loaded from.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DefinitionSource {
    #[serde(rename = "builtin")]
    Builtin,
    #[serde(rename = "kuku:user")]
    KukuUser,
    #[serde(rename = "kuku:project")]
    KukuProject,
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
            Self::KukuUser => "kuku:user",
            Self::KukuProject => "kuku:project",
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
    /// None = inherit parent tools. Some(vec![]) = no tools. Some(["a","b"]) = explicit.
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    /// Hard turn limit for the child session.
    pub max_turns: u32,
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
            "{}|{}|{}|{}|{}|{}|{:?}",
            self.name,
            self.description,
            self.instructions,
            self.tier,
            self.tool_profile.as_str(),
            self.max_turns,
            self.tools,
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
        assert!(tools.contains(&"remember_memory"));
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
            tools: Some(vec!["find_files".into(), "read_file".into()]),
            max_turns: 4,
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
    fn definition_source_kuku_variants() {
        assert_eq!(DefinitionSource::KukuUser.as_str(), "kuku:user");
        assert_eq!(DefinitionSource::KukuProject.as_str(), "kuku:project");
    }
}
