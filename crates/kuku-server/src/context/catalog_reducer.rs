use crate::api::{
    AgentCatalogEntry, AgentSummary, ApiVersion, ContextCatalog, SkillCatalogEntry,
    TierCatalogEntry, TierSummary, ToolCatalogEntry,
};

use super::catalog_contract::{CatalogEntries, CatalogEntry};

pub(crate) struct CatalogReducer;

impl CatalogReducer {
    pub(crate) fn reduce(entries: &CatalogEntries) -> ContextCatalog {
        ContextCatalog {
            api_version: ApiVersion,
            revision: entries.revision.clone(),
            tiers: entries.tiers.iter().map(reduce_tier).collect(),
            skills: entries.skills.iter().map(reduce_skill).collect(),
            agents: entries.agents.iter().map(reduce_agent).collect(),
            tools: entries.tools.iter().map(reduce_tool).collect(),
        }
    }
}

fn reduce_tier(entry: &CatalogEntry) -> TierCatalogEntry {
    let metadata = entry
        .tier
        .as_ref()
        .expect("CatalogEntries validates Tier metadata");
    TierCatalogEntry {
        tier: TierSummary {
            tier_id: entry.id.clone(),
            label: entry.name.clone(),
            purpose: metadata.purpose.clone(),
            provider: metadata.provider.clone(),
            model: metadata.model.clone(),
            think: metadata.think.clone(),
            is_default: metadata.is_default,
        },
    }
}

fn reduce_skill(entry: &CatalogEntry) -> SkillCatalogEntry {
    SkillCatalogEntry {
        skill_id: entry.id.clone(),
        name: entry.name.clone(),
        description: entry.preview.clone(),
        source: entry.source.clone(),
    }
}

fn reduce_agent(entry: &CatalogEntry) -> AgentCatalogEntry {
    let metadata = entry
        .agent
        .as_ref()
        .expect("CatalogEntries validates Agent metadata");
    AgentCatalogEntry {
        agent: AgentSummary {
            agent_id: entry.id.clone(),
            name: entry.name.clone(),
            description: entry.preview.clone(),
        },
        tier_id: metadata.tier_id.clone(),
    }
}

fn reduce_tool(entry: &CatalogEntry) -> ToolCatalogEntry {
    ToolCatalogEntry {
        tool_id: entry.id.clone(),
        name: entry.name.clone(),
        description: entry.preview.clone(),
    }
}
