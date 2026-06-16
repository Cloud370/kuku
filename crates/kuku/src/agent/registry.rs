use std::collections::BTreeMap;
use std::path::Path;

use crate::error::Result;
use crate::prompt::{PromptAsset, PromptCatalog};

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
    pub fn builtins(mut self, catalog: &PromptCatalog) -> Self {
        for (name, asset) in &catalog.agents {
            if name == "main" {
                continue; // main is not a delegable agent
            }
            if let Some(def) = parse_agent_from_asset(name, asset) {
                self.add(def);
            }
        }
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

fn parse_agent_from_asset(name: &str, asset: &PromptAsset) -> Option<AgentDefinition> {
    // Parse YAML frontmatter from the asset text
    // If no frontmatter, use defaults with filename as name
    let (frontmatter, body) = super::loader::split_frontmatter(&asset.text);
    let mut def = match frontmatter {
        Some(fm) => super::loader::parse_agent_frontmatter(name, &fm, &body)?,
        None => AgentDefinition {
            name: name.to_string(),
            description: body.lines().next().unwrap_or("").to_string(),
            instructions: body.to_string(),
            tier: "balanced".to_string(),
            tool_profile: ToolProfile::Read,
            tools: None,
            max_turns: 10,
            source: DefinitionSource::Builtin,
            hash: String::new(),
            source_path: Some(asset.path.clone()),
            metadata: serde_json::Value::Null,
        },
    };
    def.hash = def.compute_hash();
    Some(def)
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
