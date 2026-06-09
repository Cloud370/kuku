use std::collections::BTreeMap;
use std::path::Path;

use crate::error::Result;

use super::definition::{AgentDefinition, DefinitionSource, ToolProfile};

#[derive(Debug, Clone)]
pub struct AgentRegistry {
    definitions: BTreeMap<String, AgentDefinition>,
    names: Vec<String>,
    hash: String,
}

impl AgentRegistry {
    pub fn builder() -> AgentRegistryBuilder {
        AgentRegistryBuilder::default()
    }

    pub fn get(&self, name: &str) -> Option<&AgentDefinition> {
        self.definitions.get(name)
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }

    pub fn definitions(&self) -> Vec<&AgentDefinition> {
        self.names
            .iter()
            .filter_map(|name| self.definitions.get(name))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.definitions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }

    pub fn hash(&self) -> &str {
        &self.hash
    }
}

#[derive(Default)]
pub struct AgentRegistryBuilder {
    definitions: BTreeMap<String, AgentDefinition>,
}

impl AgentRegistryBuilder {
    pub fn builtins(mut self) -> Self {
        self.add(builtin_review());
        self.add(builtin_explore());
        self
    }

    pub fn load_from_dir(mut self, dir: &Path, source: DefinitionSource) -> Result<Self> {
        let defs = super::loader::load_from_dir(dir, source)?;
        for def in defs {
            self.add(def);
        }
        Ok(self)
    }

    pub fn build_with_discovery(
        mut self,
        workspace: &Path,
        config: &crate::config::DiscoveryConfig,
    ) -> Result<Self> {
        let discovered = crate::discovery::discover(workspace, config);
        for entry in &discovered.agents {
            self = self.load_from_dir(&entry.path, entry.scope.into())?;
        }
        Ok(self)
    }

    fn add(&mut self, def: AgentDefinition) {
        self.definitions.insert(def.name.clone(), def);
    }

    pub fn build(self) -> AgentRegistry {
        let mut names: Vec<String> = self.definitions.keys().cloned().collect();
        names.sort();
        let hash = compute_registry_hash(&self.definitions, &names);
        AgentRegistry {
            definitions: self.definitions,
            names,
            hash,
        }
    }
}

fn builtin_review() -> AgentDefinition {
    let mut definition = AgentDefinition {
        name: "review".into(),
        description: "Review code or docs for correctness, evidence, and boundary issues.".into(),
        instructions: concat!(
            "You are a code and document reviewer. Your job is to read the provided context carefully ",
            "and identify issues related to correctness, consistency, and boundary problems.\n\n",
            "For each finding, cite the specific file path and line number as evidence.\n",
            "Do not make changes - only report what you find.\n",
            "If you find no issues, state that clearly.\n",
            "When you finish, report your findings with specific file paths and line numbers as evidence.\n",
        )
        .into(),
        tier: "balanced".into(),
        tool_profile: ToolProfile::Read,
        tools: Some(vec!["find_files".into(), "read_file".into(), "search_text".into()]),
        max_turns: 10,
        source: DefinitionSource::Builtin,
        hash: String::new(),
        source_path: None,
        metadata: serde_json::Value::Null,
    };
    definition.hash = definition.compute_hash();
    definition
}

fn builtin_explore() -> AgentDefinition {
    let mut definition = AgentDefinition {
        name: "explore".into(),
        description: "Search broadly for patterns, definitions, or references. Report file/line evidence."
            .into(),
        instructions: concat!(
            "You are a code explorer. Search the codebase broadly for the requested patterns or information.\n",
            "Use find_files to locate relevant files, search_text to find patterns, and read_file to verify findings.\n",
            "Report what you find with file paths and line numbers.\n",
            "Be thorough but efficient - cover the search area without getting lost in details.\n",
        )
        .into(),
        tier: "light".into(),
        tool_profile: ToolProfile::Read,
        tools: Some(vec!["find_files".into(), "read_file".into(), "search_text".into()]),
        max_turns: 10,
        source: DefinitionSource::Builtin,
        hash: String::new(),
        source_path: None,
        metadata: serde_json::Value::Null,
    };
    definition.hash = definition.compute_hash();
    definition
}

fn compute_registry_hash(defs: &BTreeMap<String, AgentDefinition>, names: &[String]) -> String {
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
