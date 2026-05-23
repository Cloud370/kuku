//! Skill registry with multi-source builder and change detection.

use std::collections::BTreeMap;
use std::path::Path;

use crate::error::Result;

use super::definition::{SkillDefinition, SkillSource};

#[derive(Debug, Clone)]
pub struct SkillRegistry {
    definitions: BTreeMap<String, SkillDefinition>,
    names: Vec<String>,
    hash: String,
}

impl SkillRegistry {
    pub fn builder() -> SkillRegistryBuilder {
        SkillRegistryBuilder::default()
    }

    pub fn get(&self, name: &str) -> Option<&SkillDefinition> {
        self.definitions.get(name)
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }

    pub fn definitions(&self) -> Vec<&SkillDefinition> {
        self.names
            .iter()
            .filter_map(|n| self.definitions.get(n))
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
pub struct SkillRegistryBuilder {
    definitions: BTreeMap<String, SkillDefinition>,
}

impl SkillRegistryBuilder {
    pub fn load_claude_user_skills(mut self) -> Result<Self> {
        let home = dirs_next().ok_or_else(|| {
            crate::error::Error::InvalidArgument("cannot determine home directory".to_string())
        })?;
        let dir = home.join(".claude").join("skills");
        if dir.exists() {
            let loaded = super::loader::load_from_dir(&dir, SkillSource::ClaudeCodeUser)?;
            for def in loaded {
                self.add(def);
            }
        }
        Ok(self)
    }

    pub fn load_claude_project_skills(mut self, workspace: &Path) -> Result<Self> {
        let dir = workspace.join(".claude").join("skills");
        if dir.exists() {
            let loaded = super::loader::load_from_dir(&dir, SkillSource::ClaudeCodeProject)?;
            for def in loaded {
                self.add(def);
            }
        }
        Ok(self)
    }

    pub fn load_opencode_user_skills(mut self) -> Result<Self> {
        let home = dirs_next().ok_or_else(|| {
            crate::error::Error::InvalidArgument("cannot determine home directory".to_string())
        })?;
        let dir = home.join(".config").join("opencode").join("skills");
        if dir.exists() {
            let loaded = super::loader::load_from_dir(&dir, SkillSource::OpenCodeUser)?;
            for def in loaded {
                self.add(def);
            }
        }
        Ok(self)
    }

    pub fn load_opencode_project_skills(mut self, workspace: &Path) -> Result<Self> {
        let dir = workspace.join(".opencode").join("skills");
        if dir.exists() {
            let loaded = super::loader::load_from_dir(&dir, SkillSource::OpenCodeProject)?;
            for def in loaded {
                self.add(def);
            }
        }
        Ok(self)
    }

    pub fn load_kuku_user_skills(mut self) -> Result<Self> {
        let home = dirs_next().ok_or_else(|| {
            crate::error::Error::InvalidArgument("cannot determine home directory".to_string())
        })?;
        let dir = home.join(".kuku").join("skills");
        if dir.exists() {
            let loaded = super::loader::load_from_dir(&dir, SkillSource::KukuUser)?;
            for def in loaded {
                self.add(def);
            }
        }
        Ok(self)
    }

    pub fn load_kuku_project_skills(mut self, workspace: &Path) -> Result<Self> {
        let dir = workspace.join(".kuku").join("skills");
        if dir.exists() {
            let loaded = super::loader::load_from_dir(&dir, SkillSource::KukuProject)?;
            for def in loaded {
                self.add(def);
            }
        }
        Ok(self)
    }

    fn add(&mut self, def: SkillDefinition) {
        self.definitions.insert(def.name.clone(), def);
    }

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

pub struct SkillChanges {
    pub added: Vec<crate::notice::types::SkillChangeEntry>,
    pub updated: Vec<crate::notice::types::SkillChangeEntry>,
    pub removed: Vec<String>,
}

pub fn detect_skill_changes(old: &SkillRegistry, new: &SkillRegistry) -> Option<SkillChanges> {
    use crate::notice::types::SkillChangeEntry;

    let old_names: std::collections::HashSet<&str> =
        old.names().iter().map(|s| s.as_str()).collect();
    let new_names: std::collections::HashSet<&str> =
        new.names().iter().map(|s| s.as_str()).collect();

    let added: Vec<SkillChangeEntry> = new_names
        .difference(&old_names)
        .filter_map(|&name| {
            new.get(name).map(|def| SkillChangeEntry {
                name: def.name.clone(),
                description: def.description.clone(),
                path: def
                    .source_path
                    .clone()
                    .unwrap_or_else(|| format!("{}/", def.source.base_dir())),
            })
        })
        .collect();
    let removed: Vec<String> = old_names
        .difference(&new_names)
        .map(|s| s.to_string())
        .collect();
    let updated: Vec<SkillChangeEntry> = new_names
        .intersection(&old_names)
        .filter(|name| {
            new.get(name).map(|d| d.hash.as_str()) != old.get(name).map(|d| d.hash.as_str())
        })
        .filter_map(|&name| {
            new.get(name).map(|def| SkillChangeEntry {
                name: def.name.clone(),
                description: def.description.clone(),
                path: def
                    .source_path
                    .clone()
                    .unwrap_or_else(|| format!("{}/", def.source.base_dir())),
            })
        })
        .collect();

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

fn dirs_next() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .or_else(|| {
            let drive = std::env::var_os("HOMEDRIVE")?;
            let path = std::env::var_os("HOMEPATH")?;
            Some(std::path::PathBuf::from(drive).join(path))
        })
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
        let skill_dir = dir.path().join(".kuku").join("skills").join("test-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: test-skill\ndescription: Test\n---\n\nBody.\n",
        )
        .unwrap();

        let r1 = SkillRegistry::builder()
            .load_kuku_project_skills(dir.path())
            .unwrap()
            .build();
        let r2 = SkillRegistry::builder()
            .load_kuku_project_skills(dir.path())
            .unwrap()
            .build();
        assert_eq!(r1.hash(), r2.hash());
    }

    #[test]
    fn names_are_sorted() {
        let dir = tempfile::tempdir().unwrap();
        for name in &["zebra", "alpha", "middle"] {
            let skill_dir = dir.path().join(".kuku").join("skills").join(name);
            std::fs::create_dir_all(&skill_dir).unwrap();
            std::fs::write(
                skill_dir.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: Skill {name}\n---\n\nBody.\n"),
            )
            .unwrap();
        }

        let registry = SkillRegistry::builder()
            .load_kuku_project_skills(dir.path())
            .unwrap()
            .build();
        assert_eq!(registry.names(), &["alpha", "middle", "zebra"]);
    }

    #[test]
    fn last_writer_wins_on_duplicate_names() {
        let dir1 = tempfile::tempdir().unwrap();
        let dir2 = tempfile::tempdir().unwrap();
        for (dir, desc) in [(&dir1, "first"), (&dir2, "second")] {
            let skill_dir = dir.path().join(".kuku").join("skills").join("dup");
            std::fs::create_dir_all(&skill_dir).unwrap();
            std::fs::write(
                skill_dir.join("SKILL.md"),
                format!("---\nname: dup\ndescription: {desc}\n---\n\nBody.\n"),
            )
            .unwrap();
        }

        let registry = SkillRegistry::builder()
            .load_kuku_project_skills(dir1.path())
            .unwrap()
            .load_kuku_project_skills(dir2.path())
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
            let d = dir1.path().join(".kuku").join("skills").join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(
                d.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: {desc}\n---\n\nBody.\n"),
            )
            .unwrap();
        }
        for (name, desc) in [("shared", "v2"), ("fresh", "new")] {
            let d = dir2.path().join(".kuku").join("skills").join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(
                d.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: {desc}\n---\n\nBody.\n"),
            )
            .unwrap();
        }
        let old = SkillRegistry::builder()
            .load_kuku_project_skills(dir1.path())
            .unwrap()
            .build();
        let new = SkillRegistry::builder()
            .load_kuku_project_skills(dir2.path())
            .unwrap()
            .build();
        let changes = detect_skill_changes(&old, &new).unwrap();
        assert_eq!(changes.added.len(), 1);
        assert_eq!(changes.added[0].name, "fresh");
        assert_eq!(changes.added[0].description, "new");
        let expected_added = std::path::Path::new(".kuku").join("skills").join("fresh");
        let expected_added_str = expected_added.to_string_lossy().into_owned();
        assert!(changes.added[0].path.ends_with(&expected_added_str));
        assert_eq!(changes.removed, vec!["tdd".to_string()]);
        assert_eq!(changes.updated.len(), 1);
        assert_eq!(changes.updated[0].name, "shared");
        assert_eq!(changes.updated[0].description, "v2");
        let expected_updated = std::path::Path::new(".kuku").join("skills").join("shared");
        let expected_updated_str = expected_updated.to_string_lossy().into_owned();
        assert!(changes.updated[0].path.ends_with(&expected_updated_str));
    }
}
