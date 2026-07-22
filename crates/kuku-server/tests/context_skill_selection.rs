pub mod agent {
    pub mod definition {
        pub use kuku::agent::definition::*;
    }

    pub mod registry {
        pub use kuku::agent::registry::*;
    }
}

pub mod config {
    pub use kuku::config::*;
}

pub mod event {
    pub use kuku::event::*;
}

pub mod prompt {
    pub use kuku::prompt::*;
}

pub mod api {
    pub use kuku_server::api::*;
}

pub mod run_manager {
    pub use kuku_server::run_manager::*;
}

pub mod skill {
    pub mod definition {
        pub use kuku::skill::definition::*;
    }

    pub mod registry {
        pub use kuku::skill::registry::*;
    }
}

#[allow(dead_code)]
#[path = "../../kuku/src/context/catalog.rs"]
mod sdk_catalog;

use crate::sdk_catalog as catalog_contract;

#[path = "../src/context/catalog_reducer.rs"]
mod catalog_reducer;
#[path = "../src/context/skill_selection.rs"]
mod skill_selection;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use catalog_reducer::CatalogReducer;
use kuku::event::{SkillLoadOrigin, SourceFact, SourceScope, WorkspaceId};
use kuku_server::run_manager::{DomainError, SkillSelectionValidator};
use sdk_catalog::{
    CatalogCapabilities, CatalogEntries, CatalogEntry, CatalogKind, CatalogSource, TierMetadata,
};
use skill_selection::{CatalogSkillSelectionValidator, WorkspaceCatalogProvider};

struct Catalogs {
    requested: AtomicUsize,
    by_workspace: BTreeMap<WorkspaceId, CatalogEntries>,
}

impl WorkspaceCatalogProvider for Catalogs {
    fn for_workspace(&self, workspace_id: &WorkspaceId) -> Result<CatalogEntries, DomainError> {
        self.requested.fetch_add(1, Ordering::SeqCst);
        self.by_workspace
            .get(workspace_id)
            .cloned()
            .ok_or(DomainError::WorkspaceNotFound)
    }
}

fn workspace_id() -> WorkspaceId {
    serde_json::from_value(serde_json::json!("wsp_000000000000000000000000")).unwrap()
}

fn entry(kind: CatalogKind, source: CatalogSource, name: &str, hash: &str) -> CatalogEntry {
    CatalogEntry::new(
        kind,
        source,
        name,
        format!("{name} description"),
        SourceFact {
            scope: match source {
                CatalogSource::System => SourceScope::System,
                CatalogSource::User => SourceScope::User,
                CatalogSource::Project => SourceScope::Project,
                CatalogSource::Workspace => SourceScope::Workspace,
                CatalogSource::Agent => SourceScope::Agent,
            },
            id: format!("source:{name}"),
            relative_path: None,
        },
        hash,
        format!("{name} preview"),
        CatalogCapabilities::selectable(),
    )
    .unwrap()
}

fn catalog() -> CatalogEntries {
    let tier = entry(
        CatalogKind::Tier,
        CatalogSource::System,
        "default",
        "sha256:tier",
    )
    .with_tier_metadata(TierMetadata {
        purpose: "General work".to_owned(),
        provider: "anthropic".to_owned(),
        model: "test-model".to_owned(),
        think: Some("off".to_owned()),
        is_default: true,
    })
    .unwrap();
    CatalogEntries::new(
        vec![tier],
        vec![entry(
            CatalogKind::Skill,
            CatalogSource::Project,
            "tdd",
            "sha256:skill",
        )],
        Vec::new(),
        Vec::new(),
        1,
        1,
    )
    .unwrap()
}

fn fixture_validator() -> (Arc<Catalogs>, CatalogSkillSelectionValidator) {
    let provider = Arc::new(Catalogs {
        requested: AtomicUsize::new(0),
        by_workspace: BTreeMap::from([(workspace_id(), catalog())]),
    });
    let validator = CatalogSkillSelectionValidator::new(provider.clone());
    (provider, validator)
}

#[test]
fn invalid_selection_never_builds_submission_context() {
    let (_, validator) = fixture_validator();
    let error = validator
        .validate(
            &workspace_id(),
            "tier:default",
            &["skill:missing".to_owned()],
        )
        .unwrap_err();

    assert_eq!(DomainError::InvalidRequest, error);
}

#[test]
fn invalid_tier_and_duplicate_skills_are_rejected_atomically() {
    let (_, validator) = fixture_validator();
    assert_eq!(
        DomainError::InvalidRequest,
        validator
            .validate(
                &workspace_id(),
                "tier:missing",
                &["skill:project:tdd".to_owned()],
            )
            .unwrap_err()
    );
    assert_eq!(
        DomainError::InvalidRequest,
        validator
            .validate(
                &workspace_id(),
                "tier:default",
                &[
                    "skill:project:tdd".to_owned(),
                    "skill:project:tdd".to_owned(),
                ],
            )
            .unwrap_err()
    );
}

#[test]
fn valid_selection_preserves_requested_ids() {
    let (provider, validator) = fixture_validator();
    let validated = validator
        .validate(
            &workspace_id(),
            "tier:default",
            &["skill:project:tdd".to_owned()],
        )
        .unwrap();

    assert_eq!("tier:default", validated.selection.tier_id);
    assert_eq!(vec!["skill:project:tdd"], validated.selection.skill_ids);
    assert_eq!(1, validated.selected_skills.len());
    assert_eq!("skill:project:tdd", validated.selected_skills[0].skill_id);
    assert_eq!("source:tdd", validated.selected_skills[0].source.id);
    assert_eq!("sha256:skill", validated.selected_skills[0].content_hash);
    assert_eq!(SkillLoadOrigin::You, validated.selected_skills[0].origin);
    assert_eq!(1, provider.requested.load(Ordering::SeqCst));
}

#[test]
fn reducer_maps_catalog_without_full_skill_content() {
    let reduced = CatalogReducer::reduce(&catalog());

    assert_eq!("tier:default", reduced.tiers[0].tier.tier_id);
    assert_eq!("skill:project:tdd", reduced.skills[0].skill_id);
    assert_eq!("tdd preview", reduced.skills[0].description);
}
