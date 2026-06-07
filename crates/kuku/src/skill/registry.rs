//! Skill registry with multi-source builder and change detection.

use std::collections::BTreeMap;
use std::path::Path;

use crate::error::Result;

use serde::{Deserialize, Serialize};

use super::definition::{SkillDefinition, SkillSource};

/// In-memory index of loaded skill definitions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillRegistry {
    definitions: BTreeMap<String, SkillDefinition>,
    names: Vec<String>,
    hash: String,
}

impl SkillRegistry {
    /// Create an empty builder for constructing a registry.
    pub fn builder() -> SkillRegistryBuilder {
        SkillRegistryBuilder::default()
    }

    /// Look up a skill definition by name.
    pub fn get(&self, name: &str) -> Option<&SkillDefinition> {
        self.definitions.get(name)
    }

    /// Return sorted skill names.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// Return all skill definitions.
    pub fn definitions(&self) -> Vec<&SkillDefinition> {
        self.names
            .iter()
            .filter_map(|n| self.definitions.get(n))
            .collect()
    }

    /// Return the number of loaded skills.
    pub fn len(&self) -> usize {
        self.definitions.len()
    }

    /// Return whether no skills are loaded.
    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }

    /// Return the deterministic hash of the registry.
    pub fn hash(&self) -> &str {
        &self.hash
    }
}

/// Builder for constructing a SkillRegistry from multiple sources.
#[derive(Default)]
pub struct SkillRegistryBuilder {
    definitions: BTreeMap<String, SkillDefinition>,
}

impl SkillRegistryBuilder {
    /// Add a single skill definition to the builder.
    pub fn with_definition(mut self, def: SkillDefinition) -> Self {
        self.add(def);
        self
    }

    /// Load all skills from a directory of SKILL.md files.
    pub fn load_from_dir(mut self, dir: &Path, source: SkillSource) -> Result<Self> {
        let defs = super::loader::load_from_dir(dir, source)?;
        for def in defs {
            self.add(def);
        }
        Ok(self)
    }

    /// Build the registry by scanning workspace and discovery config.
    pub fn build_with_discovery(
        mut self,
        workspace: &Path,
        config: &crate::config::DiscoveryConfig,
    ) -> Result<Self> {
        let discovered = crate::discovery::discover(workspace, config);
        for entry in &discovered.skills {
            self = self.load_from_dir(&entry.path, entry.scope.into())?;
        }
        Ok(self)
    }

    fn add(&mut self, def: SkillDefinition) {
        self.definitions.insert(def.name.clone(), def);
    }

    /// Finalize the builder into an immutable SkillRegistry.
    pub fn build(self) -> SkillRegistry {
        let mut names: Vec<String> = self.definitions.keys().cloned().collect();
        names.sort();
        let hash = compute_registry_hash(&self.definitions, &names);
        SkillRegistry {
            definitions: self.definitions,
            names,
            hash,
        }
    }
}

/// Summary of skill additions, updates, and removals between snapshots.
pub struct SkillChanges {
    /// Names of newly added skills.
    pub added: Vec<String>,
    /// Names of skills whose content changed.
    pub updated: Vec<String>,
    /// Names of skills that were removed.
    pub removed: Vec<String>,
}

pub fn detect_skill_changes(old: &SkillRegistry, new: &SkillRegistry) -> Option<SkillChanges> {
    let old_names: std::collections::HashSet<&str> =
        old.names().iter().map(|s| s.as_str()).collect();
    let new_names: std::collections::HashSet<&str> =
        new.names().iter().map(|s| s.as_str()).collect();

    let mut added: Vec<String> = new_names
        .difference(&old_names)
        .map(|name| (*name).to_string())
        .collect();
    let mut removed: Vec<String> = old_names
        .difference(&new_names)
        .map(|s| s.to_string())
        .collect();
    let mut updated: Vec<String> = new_names
        .intersection(&old_names)
        .filter(|name| {
            new.get(name).map(|d| d.hash.as_str()) != old.get(name).map(|d| d.hash.as_str())
        })
        .map(|name| (*name).to_string())
        .collect();

    added.sort();
    removed.sort();
    updated.sort();

    if added.is_empty() && updated.is_empty() && removed.is_empty() {
        None
    } else {
        Some(SkillChanges {
            added,
            updated,
            removed,
        })
    }
}

fn compute_registry_hash(defs: &BTreeMap<String, SkillDefinition>, names: &[String]) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_registry() {
        let registry = SkillRegistry::builder().build();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert!(registry.get("anything").is_none());
    }

    #[test]
    fn registry_hash_is_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("test-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: test-skill\ndescription: Test\n---\n\nBody.\n",
        )
        .unwrap();

        let r1 = SkillRegistry::builder()
            .load_from_dir(dir.path(), SkillSource::Project)
            .unwrap()
            .build();
        let r2 = SkillRegistry::builder()
            .load_from_dir(dir.path(), SkillSource::Project)
            .unwrap()
            .build();
        assert_eq!(r1.hash(), r2.hash());
    }

    #[test]
    fn names_are_sorted() {
        let dir = tempfile::tempdir().unwrap();
        for name in &["zebra", "alpha", "middle"] {
            let skill_dir = dir.path().join(name);
            std::fs::create_dir_all(&skill_dir).unwrap();
            std::fs::write(
                skill_dir.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: Skill {name}\n---\n\nBody.\n"),
            )
            .unwrap();
        }

        let registry = SkillRegistry::builder()
            .load_from_dir(dir.path(), SkillSource::Project)
            .unwrap()
            .build();
        assert_eq!(registry.names(), &["alpha", "middle", "zebra"]);
    }

    #[test]
    fn last_writer_wins_on_duplicate_names() {
        let dir1 = tempfile::tempdir().unwrap();
        let dir2 = tempfile::tempdir().unwrap();
        for (dir, desc) in [(&dir1, "first"), (&dir2, "second")] {
            let skill_dir = dir.path().join("dup");
            std::fs::create_dir_all(&skill_dir).unwrap();
            std::fs::write(
                skill_dir.join("SKILL.md"),
                format!("---\nname: dup\ndescription: {desc}\n---\n\nBody.\n"),
            )
            .unwrap();
        }

        let registry = SkillRegistry::builder()
            .load_from_dir(dir1.path(), SkillSource::Project)
            .unwrap()
            .load_from_dir(dir2.path(), SkillSource::Project)
            .unwrap()
            .build();
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.get("dup").unwrap().description, "second");
    }

    #[test]
    fn detect_skill_changes_finds_added_updated_removed() {
        let dir1 = tempfile::tempdir().unwrap();
        let dir2 = tempfile::tempdir().unwrap();
        for (name, desc) in [("tdd", "v1"), ("shared", "v1")] {
            let d = dir1.path().join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(
                d.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: {desc}\n---\n\nBody.\n"),
            )
            .unwrap();
        }
        for (name, desc) in [("shared", "v2"), ("fresh", "new")] {
            let d = dir2.path().join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(
                d.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: {desc}\n---\n\nBody.\n"),
            )
            .unwrap();
        }
        let old = SkillRegistry::builder()
            .load_from_dir(dir1.path(), SkillSource::Project)
            .unwrap()
            .build();
        let new = SkillRegistry::builder()
            .load_from_dir(dir2.path(), SkillSource::Project)
            .unwrap()
            .build();
        let changes = detect_skill_changes(&old, &new).unwrap();
        assert_eq!(changes.added.len(), 1);
        assert_eq!(changes.added[0], "fresh");
        assert_eq!(changes.removed, vec!["tdd".to_string()]);
        assert_eq!(changes.updated.len(), 1);
        assert_eq!(changes.updated[0], "shared");
    }

    #[test]
    fn detect_skill_changes_outputs_are_sorted_by_name() {
        let old = SkillRegistry::builder()
            .with_definition(skill_definition("gamma", "old"))
            .with_definition(skill_definition("delta", "old"))
            .with_definition(skill_definition("shared-b", "old"))
            .with_definition(skill_definition("shared-a", "old"))
            .build();
        let new = SkillRegistry::builder()
            .with_definition(skill_definition("alpha", "new"))
            .with_definition(skill_definition("beta", "new"))
            .with_definition(skill_definition("shared-b", "new"))
            .with_definition(skill_definition("shared-a", "new"))
            .build();

        let changes = detect_skill_changes(&old, &new).unwrap();
        assert_eq!(
            changes
                .added
                .iter()
                .map(|entry| entry.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta"]
        );
        assert_eq!(
            changes.removed,
            vec!["delta".to_string(), "gamma".to_string()]
        );
        assert_eq!(
            changes
                .updated
                .iter()
                .map(|entry| entry.as_str())
                .collect::<Vec<_>>(),
            vec!["shared-a", "shared-b"]
        );
    }

    fn skill_definition(name: &str, description: &str) -> SkillDefinition {
        let mut definition = SkillDefinition {
            name: name.to_string(),
            description: description.to_string(),
            instructions: format!("{name} instructions"),
            source: SkillSource::Project,
            hash: String::new(),
            source_path: Some(format!("/skills/{name}")),
            allowed_tools: None,
            disallowed_tools: None,
            max_turns: None,
            model: None,
            license: None,
            compatibility: None,
            metadata: serde_json::Value::Null,
        };
        definition.hash = definition.compute_hash();
        definition
    }
}
