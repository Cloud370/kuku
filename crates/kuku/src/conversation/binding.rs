//! Conversation binding identity and hashing.

use serde::{Deserialize, Serialize};

use super::address::ConversationAddress;

/// Provenance for one input used to build a binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindingSource {
    pub kind: String,
    pub source: String,
    pub hash: String,
}

/// Resolved agent binding for a conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationBinding {
    pub conversation: ConversationAddress,
    pub binding_id: String,
    pub agent: String,
    pub tier: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub tools: Vec<String>,
    pub skills: Vec<String>,
    pub sources: Vec<BindingSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationBindingParts {
    pub agent: String,
    pub tier: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub tools: Vec<String>,
    pub skills: Vec<String>,
    pub sources: Vec<BindingSource>,
}

#[derive(Serialize)]
struct BindingIdentity<'a> {
    conversation: &'a ConversationAddress,
    agent: &'a str,
    tier: &'a str,
    provider: &'a Option<String>,
    model: &'a Option<String>,
    tools: &'a [String],
    skills: &'a [String],
    sources: &'a [BindingSource],
}

impl ConversationBinding {
    /// Create a binding and compute its deterministic binding id.
    pub fn new(conversation: ConversationAddress, parts: ConversationBindingParts) -> Self {
        let mut binding = Self {
            conversation,
            binding_id: String::new(),
            agent: parts.agent,
            tier: parts.tier,
            provider: parts.provider,
            model: parts.model,
            tools: parts.tools,
            skills: parts.skills,
            sources: parts.sources,
        };
        binding.refresh_binding_id();
        binding
    }

    /// Recompute the deterministic binding id from identity fields.
    pub fn refresh_binding_id(&mut self) {
        use sha2::{Digest, Sha256};

        let identity = BindingIdentity {
            conversation: &self.conversation,
            agent: &self.agent,
            tier: &self.tier,
            provider: &self.provider,
            model: &self.model,
            tools: &self.tools,
            skills: &self.skills,
            sources: &self.sources,
        };
        let canonical = serde_json::to_vec(&identity).expect("binding identity serializes");
        let digest = Sha256::digest(canonical);
        self.binding_id = format!("sha256:{digest:x}");
    }
}

#[derive(Serialize)]
struct SkillAttachmentIdentity<'a> {
    conversation: &'a ConversationAddress,
    base_binding_id: Option<&'a str>,
    skills: &'a [String],
    sources: &'a [BindingSource],
}

pub fn skill_attachment_binding_id(
    conversation: &ConversationAddress,
    base_binding_id: Option<&str>,
    skills: &[String],
    sources: &[BindingSource],
) -> String {
    use sha2::{Digest, Sha256};

    let identity = SkillAttachmentIdentity {
        conversation,
        base_binding_id,
        skills,
        sources,
    };
    let canonical = serde_json::to_vec(&identity).expect("skill attachment identity serializes");
    let digest = Sha256::digest(canonical);
    format!("sha256:{digest:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding() -> ConversationBinding {
        ConversationBinding::new(
            ConversationAddress::parse("review").unwrap(),
            ConversationBindingParts {
                agent: "reviewer".into(),
                tier: "balanced".into(),
                provider: Some("openai".into()),
                model: Some("gpt-5".into()),
                tools: vec!["read_file".into(), "search_text".into()],
                skills: vec!["tdd".into()],
                sources: vec![BindingSource {
                    kind: "context".into(),
                    source: "docs/spec.md".into(),
                    hash: "sha256:abc".into(),
                }],
            },
        )
    }

    #[test]
    fn identical_bindings_share_binding_id() {
        let left = binding();
        let right = binding();

        assert_eq!(left.binding_id, right.binding_id);
    }

    #[test]
    fn binding_id_changes_when_identity_changes() {
        let baseline = binding();

        let mut changed_agent = baseline.clone();
        changed_agent.agent = "planner".into();
        changed_agent.refresh_binding_id();
        assert_ne!(baseline.binding_id, changed_agent.binding_id);

        let mut changed_tier = baseline.clone();
        changed_tier.tier = "strong".into();
        changed_tier.refresh_binding_id();
        assert_ne!(baseline.binding_id, changed_tier.binding_id);

        let mut changed_model = baseline.clone();
        changed_model.model = Some("gpt-4.1".into());
        changed_model.refresh_binding_id();
        assert_ne!(baseline.binding_id, changed_model.binding_id);

        let mut changed_tools = baseline.clone();
        changed_tools.tools.push("find_files".into());
        changed_tools.refresh_binding_id();
        assert_ne!(baseline.binding_id, changed_tools.binding_id);

        let mut changed_skills = baseline.clone();
        changed_skills.skills.push("review".into());
        changed_skills.refresh_binding_id();
        assert_ne!(baseline.binding_id, changed_skills.binding_id);

        let mut changed_source_hash = baseline.clone();
        changed_source_hash.sources[0].hash = "sha256:def".into();
        changed_source_hash.refresh_binding_id();
        assert_ne!(baseline.binding_id, changed_source_hash.binding_id);
    }

    #[test]
    fn allows_main_conversation_binding() {
        let binding = ConversationBinding::new(
            ConversationAddress::MAIN,
            ConversationBindingParts {
                agent: "host".into(),
                tier: "balanced".into(),
                provider: None,
                model: None,
                tools: Vec::new(),
                skills: Vec::new(),
                sources: Vec::new(),
            },
        );

        assert_eq!("main", binding.conversation.as_str());
    }

    #[test]
    fn rejects_main_prefixed_agent_addresses() {
        assert!(ConversationAddress::parse("main/api").is_err());
    }

    #[test]
    fn skill_attachment_binding_id_changes_with_skills_and_sources() {
        let conversation = ConversationAddress::parse("review").unwrap();
        let sources = vec![BindingSource {
            kind: "skill_definition".into(),
            source: "/skills/review".into(),
            hash: "sha256:abc".into(),
        }];

        let baseline = skill_attachment_binding_id(
            &conversation,
            Some("sha256:base"),
            &["review".into()],
            &sources,
        );
        let changed_skill = skill_attachment_binding_id(
            &conversation,
            Some("sha256:base"),
            &["review".into(), "tdd".into()],
            &sources,
        );
        let changed_source = skill_attachment_binding_id(
            &conversation,
            Some("sha256:base"),
            &["review".into()],
            &[BindingSource {
                kind: "skill_definition".into(),
                source: "/skills/review".into(),
                hash: "sha256:def".into(),
            }],
        );

        assert_ne!(baseline, changed_skill);
        assert_ne!(baseline, changed_source);
    }
}
