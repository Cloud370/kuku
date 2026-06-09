use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ToolProfile {
    #[serde(rename = "none")]
    #[default]
    None,
    #[serde(rename = "read")]
    Read,
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

    pub fn allowed_tools(&self) -> &'static [&'static str] {
        match self {
            Self::None => &[],
            Self::Read => &[
                "find_files",
                "read_file",
                "search_text",
                "fetch_url",
                "fetch_web",
            ],
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DefinitionSource {
    #[serde(rename = "builtin")]
    Builtin,
    #[serde(rename = "user")]
    User,
    #[serde(rename = "project")]
    Project,
}

impl DefinitionSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::User => "user",
            Self::Project => "project",
        }
    }
}

impl From<crate::discovery::Scope> for DefinitionSource {
    fn from(scope: crate::discovery::Scope) -> Self {
        match scope {
            crate::discovery::Scope::User => Self::User,
            crate::discovery::Scope::Project => Self::Project,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentDefinition {
    pub name: String,
    pub description: String,
    pub instructions: String,
    pub tier: String,
    pub tool_profile: ToolProfile,
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    pub max_turns: u32,
    pub source: DefinitionSource,
    pub hash: String,
    #[serde(default)]
    pub source_path: Option<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

impl AgentDefinition {
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
