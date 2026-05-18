use std::collections::BTreeMap;
use std::path::Path;

use crate::error::Result;

use super::definition::{DefinitionSource, OutputContract, SubagentDefinition, ToolProfile};

/// Merged subagent registry with version tracking.
#[derive(Debug, Clone)]
pub struct SubagentRegistry {
    definitions: BTreeMap<String, SubagentDefinition>,
    names: Vec<String>,
    hash: String,
}

impl SubagentRegistry {
    /// Build a new registry from built-in definitions and optional external imports.
    pub fn builder() -> SubagentRegistryBuilder {
        SubagentRegistryBuilder::default()
    }

    /// Look up a definition by name.
    pub fn get(&self, name: &str) -> Option<&SubagentDefinition> {
        self.definitions.get(name)
    }

    /// All loaded definition names in stable order.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// All loaded definitions in stable order.
    pub fn definitions(&self) -> Vec<&SubagentDefinition> {
        self.names
            .iter()
            .filter_map(|n| self.definitions.get(n))
            .collect()
    }

    /// Number of loaded definitions.
    pub fn len(&self) -> usize {
        self.definitions.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }

    /// Registry content hash for change detection.
    pub fn hash(&self) -> &str {
        &self.hash
    }
}

#[derive(Default)]
pub struct SubagentRegistryBuilder {
    definitions: BTreeMap<String, SubagentDefinition>,
}

impl SubagentRegistryBuilder {
    /// Load the two built-in subagent definitions.
    pub fn builtins(mut self) -> Self {
        self.add(builtin_review());
        self.add(builtin_explore());
        self
    }

    /// Load Claude Code custom agents from user directory.
    pub fn load_claude_user_agents(mut self) -> Result<Self> {
        let home = dirs_next().ok_or_else(|| {
            crate::error::Error::InvalidArgument("cannot determine home directory".to_string())
        })?;
        let dir = home.join(".claude").join("agents");
        if dir.exists() {
            let loaded =
                super::compat::claude_code::load_from_dir(&dir, DefinitionSource::ClaudeCodeUser)?;
            for def in loaded {
                self.add(def);
            }
        }
        Ok(self)
    }

    /// Load Claude Code custom agents from project directory.
    pub fn load_claude_project_agents(mut self, workspace: &Path) -> Result<Self> {
        let dir = workspace.join(".claude").join("agents");
        if dir.exists() {
            let loaded = super::compat::claude_code::load_from_dir(
                &dir,
                DefinitionSource::ClaudeCodeProject,
            )?;
            for def in loaded {
                self.add(def);
            }
        }
        Ok(self)
    }

    /// Load OpenCode agents from user directory.
    pub fn load_opencode_user_agents(mut self) -> Result<Self> {
        let home = dirs_next().ok_or_else(|| {
            crate::error::Error::InvalidArgument("cannot determine home directory".to_string())
        })?;
        let dir = home.join(".opencode").join("agent");
        if dir.exists() {
            let loaded =
                super::compat::opencode::load_from_dir(&dir, DefinitionSource::OpenCodeUser)?;
            for def in loaded {
                self.add(def);
            }
        }
        Ok(self)
    }

    /// Load OpenCode agents from project directory.
    pub fn load_opencode_project_agents(mut self, workspace: &Path) -> Result<Self> {
        let dir = workspace.join(".opencode").join("agent");
        if dir.exists() {
            let loaded =
                super::compat::opencode::load_from_dir(&dir, DefinitionSource::OpenCodeProject)?;
            for def in loaded {
                self.add(def);
            }
        }
        Ok(self)
    }

    fn add(&mut self, def: SubagentDefinition) {
        self.definitions.insert(def.name.clone(), def);
    }

    /// Finalize the registry, computing the aggregate hash.
    pub fn build(self) -> SubagentRegistry {
        let mut names: Vec<String> = self.definitions.keys().cloned().collect();
        names.sort();
        let hash = compute_registry_hash(&self.definitions, &names);
        SubagentRegistry {
            definitions: self.definitions,
            names,
            hash,
        }
    }
}

fn builtin_review() -> SubagentDefinition {
    SubagentDefinition {
        name: "review".into(),
        description: "Review code or docs for correctness, evidence, and boundary issues.".into(),
        instructions: concat!(
            "You are a code and document reviewer. Your job is to read the provided context carefully ",
            "and identify issues related to correctness, consistency, and boundary problems.\n\n",
            "For each finding, cite the specific file path and line number as evidence.\n",
            "Do not make changes — only report what you find.\n",
            "If you find no issues, state that clearly.\n",
        ).into(),
        tier: "balanced".into(),
        tool_profile: ToolProfile::Read,
        permission: super::definition::PermissionPosture::Default,
        max_turns: 4,
        output_contract: OutputContract::Findings,
        source: DefinitionSource::Builtin,
        hash: String::new(),
        source_path: None,
        metadata: serde_json::Value::Null,
    }
}

fn builtin_explore() -> SubagentDefinition {
    SubagentDefinition {
        name: "explore".into(),
        description:
            "Search broadly for patterns, definitions, or references. Report file/line evidence."
                .into(),
        instructions: concat!(
            "You are a code explorer. Search the codebase broadly for the requested patterns or information.\n",
            "Use find_files to locate relevant files, search_text to find patterns, and read_file to verify findings.\n",
            "Report what you find with file paths and line numbers.\n",
            "Be thorough but efficient — cover the search area without getting lost in details.\n",
        ).into(),
        tier: "light".into(),
        tool_profile: ToolProfile::Read,
        permission: super::definition::PermissionPosture::Default,
        max_turns: 3,
        output_contract: OutputContract::Summary,
        source: DefinitionSource::Builtin,
        hash: String::new(),
        source_path: None,
        metadata: serde_json::Value::Null,
    }
}

fn compute_registry_hash(
    defs: &BTreeMap<String, SubagentDefinition>,
    names: &[String],
) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for name in names {
        if let Some(def) = defs.get(name) {
            hasher.update(name.as_bytes());
            hasher.update(b"|");
            hasher.update(def.compute_hash().as_bytes());
            hasher.update(b"\n");
        }
    }
    format!("sha256:{:x}", hasher.finalize())
}

fn dirs_next() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_registry_contains_two_agents() {
        let registry = SubagentRegistry::builder().builtins().build();
        assert_eq!(registry.len(), 2);
        assert!(registry.get("review").is_some());
        assert!(registry.get("explore").is_some());

        let review = registry.get("review").unwrap();
        assert_eq!(review.tier, "balanced");
        assert_eq!(review.tool_profile, ToolProfile::Read);
        assert_eq!(review.max_turns, 4);

        let explore = registry.get("explore").unwrap();
        assert_eq!(explore.tier, "light");
        assert_eq!(explore.max_turns, 3);
    }

    #[test]
    fn registry_hash_changes_when_definition_differs() {
        let r1 = SubagentRegistry::builder().builtins().build();
        let h1 = r1.hash().to_string();

        let r2 = SubagentRegistry::builder().build();
        assert_ne!(h1, r2.hash());
    }

    #[test]
    fn registry_hash_is_deterministic() {
        let r1 = SubagentRegistry::builder().builtins().build();
        let r2 = SubagentRegistry::builder().builtins().build();
        assert_eq!(r1.hash(), r2.hash());
    }

    #[test]
    fn names_are_sorted() {
        let registry = SubagentRegistry::builder().builtins().build();
        let names = registry.names();
        assert_eq!(names, &["explore", "review"]);
    }
}
