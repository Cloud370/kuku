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

pub mod skill {
    pub mod definition {
        pub use kuku::skill::definition::*;
    }

    pub mod registry {
        pub use kuku::skill::registry::*;
    }
}

#[allow(dead_code)]
#[path = "../src/context/catalog.rs"]
mod catalog;

use catalog::{
    entry_id, CatalogCapabilities, CatalogEntries, CatalogEntry, CatalogKind, CatalogSource,
};
use kuku::config::{
    Config, DiscoveryConfig, HandoffConfig, LogsConfig, PluginConfig, ThinkLevel, TierConfig,
    UpdateConfig,
};
use kuku::event::{SourceFact, SourceScope};
use kuku::skill::definition::{SkillDefinition, SkillSource};
use kuku::skill::registry::SkillRegistry;

fn config_with_tiers(tiers: &[(&str, &str)]) -> Config {
    Config {
        tiers: tiers
            .iter()
            .map(|(name, purpose)| {
                (
                    (*name).to_owned(),
                    TierConfig {
                        provider: "anthropic".to_owned(),
                        model: "test-model".to_owned(),
                        think: ThinkLevel::Off,
                        context_window: 128_000,
                        max_output_tokens: 8_000,
                        purpose: (*purpose).to_owned(),
                    },
                )
            })
            .collect(),
        providers: std::collections::BTreeMap::new(),
        default_tier: tiers[0].0.to_owned(),
        discovery: DiscoveryConfig::default(),
        handoff: HandoffConfig::default(),
        logs: LogsConfig::default(),
        plugin: PluginConfig::default(),
        update: UpdateConfig::default(),
    }
}

fn skill_entry(content_hash: &str) -> CatalogEntry {
    CatalogEntry::new(
        CatalogKind::Skill,
        CatalogSource::Project,
        "tdd",
        "Test-first development",
        SourceFact {
            scope: SourceScope::Project,
            id: "skill-source:project:tdd".to_owned(),
            relative_path: Some(
                kuku::event::WorkspaceRelativePath::parse(".agents/skills/tdd/SKILL.md").unwrap(),
            ),
        },
        content_hash,
        "Test-first development",
        CatalogCapabilities::selectable(),
    )
    .unwrap()
}

fn catalog_with_skill(content_hash: &str) -> CatalogEntries {
    CatalogEntries::new(
        Vec::new(),
        vec![skill_entry(content_hash)],
        Vec::new(),
        Vec::new(),
        7,
        11,
    )
    .unwrap()
}

#[test]
fn content_change_keeps_skill_id_and_changes_revision() {
    let a = catalog_with_skill("sha256:a");
    let b = catalog_with_skill("sha256:b");

    assert_eq!(a.skills[0].id, b.skills[0].id);
    assert_ne!(a.skills[0].revision, b.skills[0].revision);
    assert_ne!(a.revision, b.revision);
}

#[test]
fn catalog_order_does_not_change_revision() {
    let tdd = skill_entry("sha256:a");
    let review = CatalogEntry::new(
        CatalogKind::Skill,
        CatalogSource::Project,
        "review",
        "Review changes",
        SourceFact {
            scope: SourceScope::Project,
            id: "skill-source:project:review".to_owned(),
            relative_path: None,
        },
        "sha256:b",
        "Review changes",
        CatalogCapabilities::selectable(),
    )
    .unwrap();
    let first = CatalogEntries::new(
        Vec::new(),
        vec![tdd.clone(), review.clone()],
        Vec::new(),
        Vec::new(),
        1,
        2,
    )
    .unwrap();
    let second =
        CatalogEntries::new(Vec::new(), vec![review, tdd], Vec::new(), Vec::new(), 1, 2).unwrap();

    assert_eq!(first.skills, second.skills);
    assert_eq!(first.revision, second.revision);
}

#[test]
fn stable_ids_include_kind_and_source_without_paths() {
    assert_eq!(
        "skill:project:tdd",
        entry_id(CatalogKind::Skill, CatalogSource::Project, "tdd").unwrap()
    );
    assert_eq!(
        "tier:default",
        entry_id(CatalogKind::Tier, CatalogSource::System, "default").unwrap()
    );
    assert_eq!(
        "tool:read_file",
        entry_id(CatalogKind::Tool, CatalogSource::System, "read_file").unwrap()
    );
}

#[test]
fn generation_change_revisions_the_catalog() {
    let skill = skill_entry("sha256:a");
    let a = CatalogEntries::new(
        Vec::new(),
        vec![skill.clone()],
        Vec::new(),
        Vec::new(),
        1,
        1,
    )
    .unwrap();
    let b = CatalogEntries::new(Vec::new(), vec![skill], Vec::new(), Vec::new(), 2, 1).unwrap();

    assert_ne!(a.revision, b.revision);
}

#[test]
fn registries_build_typed_entries_without_ambient_paths_or_instructions() {
    let workspace = tempfile::tempdir().unwrap();
    let config = config_with_tiers(&[
        ("default", "General work"),
        ("balanced", "Balanced work"),
        ("light", "Light work"),
    ]);
    let skill = SkillDefinition {
        name: "tdd".to_owned(),
        description: "Test-first development".to_owned(),
        instructions: "private full instructions".to_owned(),
        source: SkillSource::Project,
        hash: "sha256:skill".to_owned(),
        source_path: Some("/ambient/project/.agents/skills/tdd/SKILL.md".to_owned()),
        allowed_tools: None,
        disallowed_tools: None,
        max_turns: None,
        model: None,
        license: None,
        compatibility: None,
        metadata: serde_json::Value::Null,
    };
    let skills = SkillRegistry::builder().with_definition(skill).build();
    let prompts = kuku::prompt::builtin_prompt_catalog();
    let agents = kuku::agent::registry::AgentRegistry::builder()
        .builtins(&prompts)
        .build();
    let tool = CatalogEntry::new(
        CatalogKind::Tool,
        CatalogSource::System,
        "read_file",
        "Read a workspace file",
        SourceFact {
            scope: SourceScope::System,
            id: "tool-source:read_file".to_owned(),
            relative_path: None,
        },
        "sha256:tool",
        "Read a workspace file",
        CatalogCapabilities::invokable(true),
    )
    .unwrap();

    let catalog = CatalogEntries::from_registries(
        &config,
        &skills,
        &agents,
        &prompts,
        vec![tool],
        workspace.path(),
        3,
        5,
    )
    .unwrap();

    assert!(catalog.tiers.iter().any(|entry| entry.id == "tier:default"));
    assert_eq!("skill:project:tdd", catalog.skills[0].id);
    assert_eq!(None, catalog.skills[0].source.relative_path);
    assert_eq!("Test-first development", catalog.skills[0].preview);
    assert!(!catalog.skills[0]
        .preview
        .contains("private full instructions"));
    assert!(catalog.agents.iter().all(|entry| entry.agent.is_some()));
    assert!(catalog.tools[0].capabilities.read_only);
}

#[test]
fn prompt_catalog_hash_is_deterministic_and_content_sensitive() {
    let first = kuku::prompt::builtin_prompt_catalog();
    let mut changed = first.clone();
    changed.system.hash = "sha256:changed".to_owned();

    assert_eq!(first.hash(), kuku::prompt::builtin_prompt_catalog().hash());
    assert_ne!(first.hash(), changed.hash());
}

#[test]
fn builtin_tools_are_exposed_as_safe_catalog_entries() {
    let entries = kuku::tool::builtin_catalog_entries(true, true).unwrap();

    assert!(entries.iter().any(|entry| entry.id == "tool:read_file"));
    assert!(entries
        .iter()
        .all(|entry| entry.source.relative_path.is_none()));
    assert!(entries.iter().all(|entry| entry.capabilities.invokable));
}

#[test]
fn valid_long_and_empty_metadata_produce_bounded_fallback_previews() {
    let workspace = tempfile::tempdir().unwrap();
    let agent_dir = workspace.path().join("agents");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(agent_dir.join("empty.md"), "Agent instructions only.\n").unwrap();
    let prompts = kuku::prompt::builtin_prompt_catalog();
    let agents = kuku::agent::registry::AgentRegistry::builder()
        .load_from_dir(
            &agent_dir,
            kuku::agent::definition::DefinitionSource::Project,
        )
        .unwrap()
        .build();
    let long = "x".repeat(400);
    let skill = SkillDefinition {
        name: "tdd".to_owned(),
        description: long.clone(),
        instructions: "Test first".to_owned(),
        source: SkillSource::Project,
        hash: "sha256:skill".to_owned(),
        source_path: None,
        allowed_tools: None,
        disallowed_tools: None,
        max_turns: None,
        model: None,
        license: None,
        compatibility: None,
        metadata: serde_json::Value::Null,
    };
    let skills = SkillRegistry::builder().with_definition(skill).build();

    let catalog = CatalogEntries::from_registries(
        &config_with_tiers(&[("balanced", long.as_str())]),
        &skills,
        &agents,
        &prompts,
        Vec::new(),
        workspace.path(),
        1,
        1,
    )
    .unwrap();

    assert_eq!(280, catalog.tiers[0].preview.chars().count());
    assert_eq!(280, catalog.skills[0].preview.chars().count());
    assert_eq!("empty", catalog.agents[0].preview);
}

#[test]
fn builtin_agent_asset_paths_are_not_workspace_provenance() {
    let workspace = tempfile::tempdir().unwrap();
    let prompts = kuku::prompt::builtin_prompt_catalog();
    let agents = kuku::agent::registry::AgentRegistry::builder()
        .builtins(&prompts)
        .build();
    let catalog = CatalogEntries::from_registries(
        &config_with_tiers(&[("balanced", "Balanced"), ("light", "Light")]),
        &SkillRegistry::builder().build(),
        &agents,
        &prompts,
        Vec::new(),
        workspace.path(),
        1,
        1,
    )
    .unwrap();

    assert!(catalog
        .agents
        .iter()
        .all(|entry| entry.source.relative_path.is_none()));
}

#[test]
fn project_source_outside_workspace_is_not_workspace_provenance() {
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let source_path = outside.path().join("skills/tdd/SKILL.md");
    std::fs::create_dir_all(source_path.parent().unwrap()).unwrap();
    std::fs::write(&source_path, "Test first").unwrap();
    let skill = SkillDefinition {
        name: "tdd".to_owned(),
        description: "Test-first development".to_owned(),
        instructions: "Test first".to_owned(),
        source: SkillSource::Project,
        hash: "sha256:skill".to_owned(),
        source_path: Some(source_path.to_string_lossy().into_owned()),
        allowed_tools: None,
        disallowed_tools: None,
        max_turns: None,
        model: None,
        license: None,
        compatibility: None,
        metadata: serde_json::Value::Null,
    };
    let skills = SkillRegistry::builder()
        .with_definition(skill.clone())
        .build();
    let prompts = kuku::prompt::builtin_prompt_catalog();
    let agents = kuku::agent::registry::AgentRegistry::builder()
        .builtins(&prompts)
        .build();

    let catalog = CatalogEntries::from_registries(
        &config_with_tiers(&[
            ("default", "Default"),
            ("balanced", "Balanced"),
            ("light", "Light"),
        ]),
        &skills,
        &agents,
        &prompts,
        Vec::new(),
        workspace.path(),
        1,
        1,
    )
    .unwrap();

    assert_eq!(None, catalog.skills[0].source.relative_path);

    let inside_path = workspace.path().join("skills/tdd/SKILL.md");
    std::fs::create_dir_all(inside_path.parent().unwrap()).unwrap();
    std::fs::write(&inside_path, "Test first").unwrap();
    let mut inside_skill = skill;
    inside_skill.source_path = Some(inside_path.to_string_lossy().into_owned());
    let inside_skills = SkillRegistry::builder()
        .with_definition(inside_skill)
        .build();
    let inside_catalog = CatalogEntries::from_registries(
        &config_with_tiers(&[
            ("default", "Default"),
            ("balanced", "Balanced"),
            ("light", "Light"),
        ]),
        &inside_skills,
        &agents,
        &prompts,
        Vec::new(),
        workspace.path(),
        1,
        1,
    )
    .unwrap();

    assert_eq!(
        Some("skills/tdd/SKILL.md"),
        inside_catalog.skills[0]
            .source
            .relative_path
            .as_ref()
            .map(kuku::event::WorkspaceRelativePath::as_str)
    );
}

#[test]
fn agent_tier_must_reference_a_catalog_tier() {
    let workspace = tempfile::tempdir().unwrap();
    let prompts = kuku::prompt::builtin_prompt_catalog();
    let agents = kuku::agent::registry::AgentRegistry::builder()
        .builtins(&prompts)
        .build();
    let result = CatalogEntries::from_registries(
        &config_with_tiers(&[("default", "Default")]),
        &SkillRegistry::builder().build(),
        &agents,
        &prompts,
        Vec::new(),
        workspace.path(),
        1,
        1,
    );

    assert!(result.is_err());
}
