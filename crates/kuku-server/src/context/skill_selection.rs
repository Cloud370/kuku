use std::collections::BTreeSet;
use std::sync::Arc;

use crate::run_manager::{DomainError, SkillSelectionValidator, ValidatedSkillSelection};
use kuku::event::{SkillContextFact, SkillLoadOrigin, SkillsChangedFact, WorkspaceId};

use super::catalog_contract::CatalogEntries;

pub(crate) trait WorkspaceCatalogProvider: Send + Sync {
    fn for_workspace(&self, workspace_id: &WorkspaceId) -> Result<CatalogEntries, DomainError>;
}

impl<F> WorkspaceCatalogProvider for F
where
    F: Fn(&WorkspaceId) -> Result<CatalogEntries, DomainError> + Send + Sync,
{
    fn for_workspace(&self, workspace_id: &WorkspaceId) -> Result<CatalogEntries, DomainError> {
        self(workspace_id)
    }
}

pub(crate) struct CatalogSkillSelectionValidator {
    catalogs: Arc<dyn WorkspaceCatalogProvider>,
}

impl CatalogSkillSelectionValidator {
    pub(crate) fn new(catalogs: Arc<dyn WorkspaceCatalogProvider>) -> Self {
        Self { catalogs }
    }
}

impl SkillSelectionValidator for CatalogSkillSelectionValidator {
    fn validate(
        &self,
        workspace_id: &WorkspaceId,
        tier_id: &str,
        skill_ids: &[String],
    ) -> Result<ValidatedSkillSelection, DomainError> {
        let catalog = self.catalogs.for_workspace(workspace_id)?;
        SubmissionContextBuilder::from_catalog(&catalog).build(tier_id, skill_ids)
    }
}

pub(crate) struct SubmissionContextBuilder<'a> {
    catalog: &'a CatalogEntries,
}

impl<'a> SubmissionContextBuilder<'a> {
    pub(crate) fn from_catalog(catalog: &'a CatalogEntries) -> Self {
        Self { catalog }
    }

    pub(crate) fn build(
        self,
        tier_id: &str,
        skill_ids: &[String],
    ) -> Result<ValidatedSkillSelection, DomainError> {
        let tier_valid = self
            .catalog
            .tiers
            .iter()
            .any(|entry| entry.id == tier_id && entry.capabilities.selectable);
        if !tier_valid {
            return Err(DomainError::InvalidRequest);
        }
        let mut unique = BTreeSet::new();
        let mut selected_skills = Vec::with_capacity(skill_ids.len());
        for skill_id in skill_ids {
            if !unique.insert(skill_id.as_str()) {
                return Err(DomainError::InvalidRequest);
            }
            let entry = self
                .catalog
                .skills
                .iter()
                .find(|entry| entry.id == *skill_id && entry.capabilities.selectable)
                .ok_or(DomainError::InvalidRequest)?;
            selected_skills.push(SkillContextFact {
                skill_id: entry.id.clone(),
                source: entry.source.clone(),
                origin: SkillLoadOrigin::You,
                content_hash: entry.content_hash.clone(),
            });
        }
        Ok(ValidatedSkillSelection {
            selection: SkillsChangedFact {
                tier_id: tier_id.to_owned(),
                skill_ids: skill_ids.to_vec(),
            },
            selected_skills,
        })
    }
}
