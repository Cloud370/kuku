//! Deterministic, revisioned Context catalog values.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::agent::definition::DefinitionSource;
use crate::agent::registry::AgentRegistry;
use crate::config::Config;
use crate::event::{RevisionToken, SourceFact, SourceScope, WorkspaceRelativePath};
use crate::prompt::PromptCatalog;
use crate::skill::definition::SkillSource;
use crate::skill::registry::SkillRegistry;

const MAX_PREVIEW_CHARS: usize = 280;

/// Failure to construct a deterministic catalog.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogError {
    /// An entry identity or required value is invalid.
    #[error("catalog entry is invalid: {0}")]
    InvalidEntry(String),
    /// The same stable identity occurs more than once.
    #[error("catalog entry ID is duplicated: {0}")]
    DuplicateId(String),
    /// Canonical catalog hashing failed.
    #[error("catalog could not be hashed")]
    Hash,
}

/// Finite kind of a discoverable catalog entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogKind {
    /// Model Tier selectable for a Run.
    Tier,
    /// Skill selectable for a Run.
    Skill,
    /// Agent available for delegation.
    Agent,
    /// Tool available to the selected execution context.
    Tool,
}

/// Stable ownership scope used in catalog identities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogSource {
    /// Built-in runtime or host source.
    System,
    /// User-level source.
    User,
    /// Project-level source.
    Project,
    /// Registered workspace source.
    Workspace,
    /// Agent-specific source.
    Agent,
}

impl CatalogSource {
    /// Returns the stable identity segment for this source.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Project => "project",
            Self::Workspace => "workspace",
            Self::Agent => "agent",
        }
    }
}

/// Finite capabilities advertised for one catalog entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogCapabilities {
    /// The entry may be selected on a Run submission.
    pub selectable: bool,
    /// The entry may be delegated to.
    pub delegatable: bool,
    /// The entry may be invoked as a Tool.
    pub invokable: bool,
    /// Invocation cannot mutate the workspace.
    pub read_only: bool,
}

impl CatalogCapabilities {
    /// Returns capabilities for a selectable Tier or Skill.
    pub fn selectable() -> Self {
        Self {
            selectable: true,
            delegatable: false,
            invokable: false,
            read_only: true,
        }
    }

    /// Returns capabilities for a delegatable Agent.
    pub fn delegatable() -> Self {
        Self {
            selectable: false,
            delegatable: true,
            invokable: false,
            read_only: true,
        }
    }

    /// Returns capabilities for an invokable Tool.
    pub fn invokable(read_only: bool) -> Self {
        Self {
            selectable: false,
            delegatable: false,
            invokable: true,
            read_only,
        }
    }
}

/// Tier-specific metadata retained by the SDK catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierMetadata {
    /// Human-readable purpose.
    pub purpose: String,
    /// Provider configuration identity.
    pub provider: String,
    /// Provider model identity.
    pub model: String,
    /// Configured thinking level when available.
    pub think: Option<String>,
    /// Whether this is the workspace default Tier.
    pub is_default: bool,
}

/// Agent-specific metadata retained by the SDK catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMetadata {
    /// Stable Tier ID used by the Agent.
    pub tier_id: String,
}

/// One dependency-free SDK catalog entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogEntry {
    /// Stable entry identity.
    pub id: String,
    /// Entry kind.
    pub kind: CatalogKind,
    /// Stable source scope.
    pub source_scope: CatalogSource,
    /// Display name.
    pub name: String,
    /// Safe compact description.
    pub description: String,
    /// Stable source fact without an ambient path.
    pub source: SourceFact,
    /// Hash of the complete underlying definition.
    pub content_hash: String,
    /// Revision of this entry.
    pub revision: RevisionToken,
    /// Read-only compact preview, never full instructions.
    pub preview: String,
    /// Supported catalog actions.
    pub capabilities: CatalogCapabilities,
    /// Tier metadata for Tier entries.
    pub tier: Option<TierMetadata>,
    /// Agent metadata for Agent entries.
    pub agent: Option<AgentMetadata>,
}

impl CatalogEntry {
    /// Constructs a validated catalog entry and its content revision.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        kind: CatalogKind,
        source_scope: CatalogSource,
        name: impl Into<String>,
        description: impl Into<String>,
        source: SourceFact,
        content_hash: impl Into<String>,
        preview: impl Into<String>,
        capabilities: CatalogCapabilities,
    ) -> Result<Self, CatalogError> {
        let name = name.into();
        let id = entry_id(kind, source_scope, &name)?;
        let description = description.into();
        let content_hash = content_hash.into();
        let preview = preview.into();
        validate_entry_values(&description, &content_hash, &preview)?;
        let mut entry = Self {
            id,
            kind,
            source_scope,
            name,
            description,
            source,
            content_hash,
            revision: empty_revision(),
            preview,
            capabilities,
            tier: None,
            agent: None,
        };
        entry.refresh_revision()?;
        Ok(entry)
    }

    /// Attaches required Tier metadata and refreshes the entry revision.
    pub fn with_tier_metadata(mut self, metadata: TierMetadata) -> Result<Self, CatalogError> {
        if self.kind != CatalogKind::Tier
            || metadata.provider.is_empty()
            || metadata.model.is_empty()
        {
            return Err(CatalogError::InvalidEntry(self.id));
        }
        self.tier = Some(metadata);
        self.refresh_revision()?;
        Ok(self)
    }

    /// Attaches required Agent metadata and refreshes the entry revision.
    pub fn with_agent_metadata(mut self, metadata: AgentMetadata) -> Result<Self, CatalogError> {
        if self.kind != CatalogKind::Agent || !metadata.tier_id.starts_with("tier:") {
            return Err(CatalogError::InvalidEntry(self.id));
        }
        self.agent = Some(metadata);
        self.refresh_revision()?;
        Ok(self)
    }

    fn refresh_revision(&mut self) -> Result<(), CatalogError> {
        self.revision = digest(&EntryRevisionMaterial {
            id: &self.id,
            kind: self.kind,
            source_scope: self.source_scope,
            name: &self.name,
            description: &self.description,
            source: &self.source,
            content_hash: &self.content_hash,
            preview: &self.preview,
            capabilities: self.capabilities,
            tier: self.tier.as_ref(),
            agent: self.agent.as_ref(),
        })?;
        Ok(())
    }
}

#[derive(Serialize)]
struct EntryRevisionMaterial<'a> {
    id: &'a str,
    kind: CatalogKind,
    source_scope: CatalogSource,
    name: &'a str,
    description: &'a str,
    source: &'a SourceFact,
    content_hash: &'a str,
    preview: &'a str,
    capabilities: CatalogCapabilities,
    tier: Option<&'a TierMetadata>,
    agent: Option<&'a AgentMetadata>,
}

/// A deterministic snapshot of all catalog kinds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogEntries {
    /// Catalog-wide revision.
    pub revision: RevisionToken,
    /// Sorted Tier entries.
    pub tiers: Vec<CatalogEntry>,
    /// Sorted Skill entries.
    pub skills: Vec<CatalogEntry>,
    /// Sorted Agent entries.
    pub agents: Vec<CatalogEntry>,
    /// Sorted Tool entries.
    pub tools: Vec<CatalogEntry>,
}

impl CatalogEntries {
    /// Constructs a sorted snapshot bound to workspace and config generations.
    pub fn new(
        tiers: Vec<CatalogEntry>,
        skills: Vec<CatalogEntry>,
        agents: Vec<CatalogEntry>,
        tools: Vec<CatalogEntry>,
        workspace_generation: u64,
        config_generation: u64,
    ) -> Result<Self, CatalogError> {
        Self::new_with_source_hashes(
            tiers,
            skills,
            agents,
            tools,
            workspace_generation,
            config_generation,
            Vec::new(),
        )
    }

    /// Builds catalog entries from current SDK registries and typed Tool entries.
    #[allow(clippy::too_many_arguments)]
    pub fn from_registries(
        config: &Config,
        skills: &SkillRegistry,
        agents: &AgentRegistry,
        prompts: &PromptCatalog,
        tools: Vec<CatalogEntry>,
        workspace_generation: u64,
        config_generation: u64,
    ) -> Result<Self, CatalogError> {
        let tiers = config
            .tiers
            .iter()
            .map(|(name, tier)| tier_entry(config, name, tier))
            .collect::<Result<Vec<_>, _>>()?;
        let skill_entries = skills
            .definitions()
            .into_iter()
            .map(skill_entry)
            .collect::<Result<Vec<_>, _>>()?;
        let agent_entries = agents
            .definitions()
            .into_iter()
            .map(agent_entry)
            .collect::<Result<Vec<_>, _>>()?;
        Self::new_with_source_hashes(
            tiers,
            skill_entries,
            agent_entries,
            tools,
            workspace_generation,
            config_generation,
            vec![
                skills.hash().to_owned(),
                agents.hash().to_owned(),
                prompts.hash(),
            ],
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new_with_source_hashes(
        mut tiers: Vec<CatalogEntry>,
        mut skills: Vec<CatalogEntry>,
        mut agents: Vec<CatalogEntry>,
        mut tools: Vec<CatalogEntry>,
        workspace_generation: u64,
        config_generation: u64,
        source_hashes: Vec<String>,
    ) -> Result<Self, CatalogError> {
        validate_kinds(&tiers, CatalogKind::Tier)?;
        validate_kinds(&skills, CatalogKind::Skill)?;
        validate_kinds(&agents, CatalogKind::Agent)?;
        validate_kinds(&tools, CatalogKind::Tool)?;
        tiers.sort_by(|left, right| left.id.cmp(&right.id));
        skills.sort_by(|left, right| left.id.cmp(&right.id));
        agents.sort_by(|left, right| left.id.cmp(&right.id));
        tools.sort_by(|left, right| left.id.cmp(&right.id));
        validate_unique_ids([&tiers, &skills, &agents, &tools])?;
        let revision = digest(&CatalogRevisionMaterial {
            tiers: &tiers,
            skills: &skills,
            agents: &agents,
            tools: &tools,
            workspace_generation,
            config_generation,
            source_hashes: &source_hashes,
        })?;
        Ok(Self {
            revision,
            tiers,
            skills,
            agents,
            tools,
        })
    }
}

#[derive(Serialize)]
struct CatalogRevisionMaterial<'a> {
    tiers: &'a [CatalogEntry],
    skills: &'a [CatalogEntry],
    agents: &'a [CatalogEntry],
    tools: &'a [CatalogEntry],
    workspace_generation: u64,
    config_generation: u64,
    source_hashes: &'a [String],
}

/// Computes a stable catalog ID without embedding a source path.
pub fn entry_id(
    kind: CatalogKind,
    source: CatalogSource,
    name: &str,
) -> Result<String, CatalogError> {
    if name.is_empty()
        || name.chars().any(char::is_whitespace)
        || name.bytes().any(|byte| byte.is_ascii_control())
        || name.contains([':', '/', '\\'])
    {
        return Err(CatalogError::InvalidEntry(name.to_owned()));
    }
    Ok(match kind {
        CatalogKind::Tier => format!("tier:{name}"),
        CatalogKind::Skill => format!("skill:{}:{name}", source.as_str()),
        CatalogKind::Agent => format!("agent:{}:{name}", source.as_str()),
        CatalogKind::Tool => format!("tool:{name}"),
    })
}

fn tier_entry(
    config: &Config,
    name: &str,
    tier: &crate::config::TierConfig,
) -> Result<CatalogEntry, CatalogError> {
    let hash = content_digest(&(
        &tier.provider,
        &tier.model,
        tier.think.as_str(),
        tier.context_window,
        tier.max_output_tokens,
        &tier.purpose,
    ))?;
    CatalogEntry::new(
        CatalogKind::Tier,
        CatalogSource::System,
        name,
        &tier.purpose,
        source_fact(SourceScope::System, format!("tier-config:{name}"), None),
        hash,
        &tier.purpose,
        CatalogCapabilities::selectable(),
    )?
    .with_tier_metadata(TierMetadata {
        purpose: tier.purpose.clone(),
        provider: tier.provider.clone(),
        model: tier.model.clone(),
        think: Some(tier.think.as_str().to_owned()),
        is_default: config.default_tier() == name,
    })
}

fn skill_entry(
    definition: &crate::skill::definition::SkillDefinition,
) -> Result<CatalogEntry, CatalogError> {
    let (source_scope, fact_scope) = match definition.source {
        SkillSource::User => (CatalogSource::User, SourceScope::User),
        SkillSource::Project => (CatalogSource::Project, SourceScope::Project),
        SkillSource::Workspace => (CatalogSource::Workspace, SourceScope::Workspace),
    };
    CatalogEntry::new(
        CatalogKind::Skill,
        source_scope,
        &definition.name,
        &definition.description,
        source_fact(
            fact_scope,
            format!("skill-source:{}:{}", source_scope.as_str(), definition.name),
            contained_path(definition.source_path.as_deref()),
        ),
        &definition.hash,
        &definition.description,
        CatalogCapabilities::selectable(),
    )
}

fn agent_entry(
    definition: &crate::agent::definition::AgentDefinition,
) -> Result<CatalogEntry, CatalogError> {
    let (source_scope, fact_scope) = match definition.source {
        DefinitionSource::Builtin => (CatalogSource::System, SourceScope::System),
        DefinitionSource::User => (CatalogSource::User, SourceScope::User),
        DefinitionSource::Project => (CatalogSource::Project, SourceScope::Project),
    };
    let tier_id = if definition.tier.starts_with("tier:") {
        definition.tier.clone()
    } else {
        format!("tier:{}", definition.tier)
    };
    CatalogEntry::new(
        CatalogKind::Agent,
        source_scope,
        &definition.name,
        &definition.description,
        source_fact(
            fact_scope,
            format!("agent-source:{}:{}", source_scope.as_str(), definition.name),
            contained_path(definition.source_path.as_deref()),
        ),
        &definition.hash,
        &definition.description,
        CatalogCapabilities::delegatable(),
    )?
    .with_agent_metadata(AgentMetadata { tier_id })
}

fn source_fact(
    scope: SourceScope,
    id: String,
    relative_path: Option<WorkspaceRelativePath>,
) -> SourceFact {
    SourceFact {
        scope,
        id,
        relative_path,
    }
}

fn contained_path(path: Option<&str>) -> Option<WorkspaceRelativePath> {
    path.and_then(|value| WorkspaceRelativePath::parse(value).ok())
}

fn validate_entry_values(
    description: &str,
    content_hash: &str,
    preview: &str,
) -> Result<(), CatalogError> {
    if description.is_empty()
        || content_hash.is_empty()
        || preview.is_empty()
        || preview.chars().count() > MAX_PREVIEW_CHARS
    {
        return Err(CatalogError::InvalidEntry(
            "description, content hash, and compact preview are required".to_owned(),
        ));
    }
    Ok(())
}

fn validate_kinds(entries: &[CatalogEntry], expected: CatalogKind) -> Result<(), CatalogError> {
    if let Some(entry) = entries.iter().find(|entry| entry.kind != expected) {
        return Err(CatalogError::InvalidEntry(entry.id.clone()));
    }
    if expected == CatalogKind::Tier && entries.iter().any(|entry| entry.tier.is_none()) {
        return Err(CatalogError::InvalidEntry(
            "Tier metadata is required".to_owned(),
        ));
    }
    if expected == CatalogKind::Agent && entries.iter().any(|entry| entry.agent.is_none()) {
        return Err(CatalogError::InvalidEntry(
            "Agent metadata is required".to_owned(),
        ));
    }
    Ok(())
}

fn validate_unique_ids<'a>(
    groups: impl IntoIterator<Item = &'a Vec<CatalogEntry>>,
) -> Result<(), CatalogError> {
    let mut ids = BTreeSet::new();
    for entry in groups.into_iter().flatten() {
        if !ids.insert(entry.id.as_str()) {
            return Err(CatalogError::DuplicateId(entry.id.clone()));
        }
    }
    Ok(())
}

fn content_digest(value: &impl Serialize) -> Result<String, CatalogError> {
    let bytes = serde_json::to_vec(value).map_err(|_| CatalogError::Hash)?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn digest(value: &impl Serialize) -> Result<RevisionToken, CatalogError> {
    let bytes = serde_json::to_vec(value).map_err(|_| CatalogError::Hash)?;
    RevisionToken::parse(format!("{:x}", Sha256::digest(bytes))).map_err(|_| CatalogError::Hash)
}

fn empty_revision() -> RevisionToken {
    RevisionToken::parse("0".repeat(64)).expect("zero digest is a valid revision")
}
