use kuku::agent::definition::{AgentDefinition, DefinitionSource, ToolProfile};
use kuku::agent::registry::AgentRegistry;
use kuku::prompt::builtin_prompt_catalog;

#[test]
fn builtin_registry_is_loadable_and_queryable() {
    let registry = AgentRegistry::builder()
        .builtins(&builtin_prompt_catalog())
        .build();
    assert_eq!(registry.len(), 2);

    let review = registry.get("review").expect("review should exist");
    assert_eq!(review.tier, "balanced");
    assert_eq!(review.max_turns, 10);
    assert_eq!(review.tool_profile.as_str(), "read");

    let explore = registry.get("explore").expect("explore should exist");
    assert_eq!(explore.tier, "light");
    assert_eq!(explore.max_turns, 10);
}

#[test]
fn registry_hash_is_stable() {
    let r1 = AgentRegistry::builder()
        .builtins(&builtin_prompt_catalog())
        .build();
    let r2 = AgentRegistry::builder()
        .builtins(&builtin_prompt_catalog())
        .build();
    assert_eq!(r1.hash(), r2.hash());
    assert_eq!(r1.names(), r2.names());
}

#[test]
fn definition_hash_changes_when_instructions_change() {
    let base = sample_definition();
    let mut changed = base.clone();
    changed.instructions.push_str(" Extra guidance.");
    assert_ne!(base.compute_hash(), changed.compute_hash());
}

#[test]
fn definition_hash_changes_when_tier_changes() {
    let base = sample_definition();
    let mut changed = base.clone();
    changed.tier = "strong".into();
    assert_ne!(base.compute_hash(), changed.compute_hash());
}

#[test]
fn definition_hash_changes_when_tools_change() {
    let base = sample_definition();
    let mut changed = base.clone();
    changed.tools = Some(vec!["find_files".into(), "read_file".into()]);
    assert_ne!(base.compute_hash(), changed.compute_hash());
}

fn sample_definition() -> AgentDefinition {
    AgentDefinition {
        name: "review".into(),
        description: "Review code".into(),
        instructions: "Review carefully.".into(),
        tier: "balanced".into(),
        tool_profile: ToolProfile::Read,
        tools: Some(vec!["find_files".into()]),
        max_turns: 4,
        source: DefinitionSource::Builtin,
        hash: String::new(),
        source_path: None,
        metadata: serde_json::Value::Null,
    }
}
