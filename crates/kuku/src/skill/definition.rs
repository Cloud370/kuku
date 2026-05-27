//! Skill definition types and content hashing.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SkillSource {
    #[serde(rename = "user")]
    User,
    #[serde(rename = "project")]
    Project,
}

impl SkillSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
        }
    }
}

impl From<crate::discovery::Scope> for SkillSource {
    fn from(scope: crate::discovery::Scope) -> Self {
        match scope {
            crate::discovery::Scope::User => Self::User,
            crate::discovery::Scope::Project => Self::Project,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillDefinition {
    pub name: String,
    pub description: String,
    pub instructions: String,
    pub source: SkillSource,
    pub hash: String,
    pub source_path: Option<String>,
    pub allowed_tools: Option<Vec<String>>,
    pub disallowed_tools: Option<Vec<String>>,
    pub max_turns: Option<u32>,
    pub model: Option<String>,
    pub license: Option<String>,
    pub compatibility: Option<String>,
    pub metadata: serde_json::Value,
}

impl SkillDefinition {
    pub fn compute_hash(&self) -> String {
        use sha2::{Digest, Sha256};
        let allowed = match &self.allowed_tools {
            Some(v) => v.join(","),
            None => String::new(),
        };
        let disallowed = match &self.disallowed_tools {
            Some(v) => v.join(","),
            None => String::new(),
        };
        let canonical = format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}",
            self.name,
            self.description,
            self.instructions,
            allowed,
            disallowed,
            self.max_turns.map_or(String::new(), |v| v.to_string()),
            self.model.as_deref().unwrap_or(""),
            self.license.as_deref().unwrap_or(""),
            self.compatibility.as_deref().unwrap_or(""),
        );
        let digest = Sha256::digest(canonical.as_bytes());
        format!("sha256:{digest:x}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_skill(name: &str) -> SkillDefinition {
        SkillDefinition {
            name: name.into(),
            description: "Test skill".into(),
            instructions: "Do the thing.".into(),
            source: SkillSource::Project,
            hash: String::new(),
            source_path: None,
            allowed_tools: None,
            disallowed_tools: None,
            max_turns: None,
            model: None,
            license: None,
            compatibility: None,
            metadata: serde_json::Value::Null,
        }
    }

    #[test]
    fn skill_hash_is_deterministic() {
        let def = minimal_skill("tdd");
        let hash = def.compute_hash();
        assert!(hash.starts_with("sha256:"));
        assert_eq!(hash, def.compute_hash());
    }

    #[test]
    fn skill_hash_changes_with_content() {
        let mut def = minimal_skill("tdd");
        let h1 = def.compute_hash();
        def.instructions = "Different.".into();
        let h2 = def.compute_hash();
        assert_ne!(h1, h2);
    }

    #[test]
    fn skill_hash_excludes_source_and_path() {
        let mut def = minimal_skill("tdd");
        let h1 = def.compute_hash();
        def.source = SkillSource::User;
        def.source_path = Some("/some/path".into());
        assert_eq!(h1, def.compute_hash());
    }

    #[test]
    fn serde_round_trip() {
        let sources = [SkillSource::User, SkillSource::Project];
        for src in sources {
            let json = serde_json::to_string(&src).unwrap();
            let back: SkillSource = serde_json::from_str(&json).unwrap();
            assert_eq!(src, back);
        }
    }

    #[test]
    fn serde_values() {
        assert_eq!(
            serde_json::to_string(&SkillSource::User).unwrap(),
            "\"user\""
        );
        assert_eq!(
            serde_json::to_string(&SkillSource::Project).unwrap(),
            "\"project\""
        );
    }
}
