use std::collections::BTreeMap;

use super::definition::SkillDefinition;

#[derive(Debug, Clone)]
pub struct SkillRegistry {
    definitions: BTreeMap<String, SkillDefinition>,
    names: Vec<String>,
    hash: String,
}

impl SkillRegistry {
    pub fn get(&self, name: &str) -> Option<&SkillDefinition> {
        self.definitions.get(name)
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }

    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }

    pub fn hash(&self) -> &str {
        &self.hash
    }
}
