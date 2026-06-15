use kuku::agent::catalog::{render_agent_catalog, render_agent_directory};
use kuku::prompt::builtin_prompt_catalog;

#[test]
fn catalog_does_not_leak_full_instructions() {
    let registry = kuku::agent::registry::AgentRegistry::builder()
        .builtins(&builtin_prompt_catalog())
        .build();
    let catalog =
        render_agent_catalog(&registry, &builtin_prompt_catalog()).expect("catalog should render");
    assert!(
        !catalog.contains("code and document reviewer"),
        "catalog leaks review instructions"
    );
    assert!(
        !catalog.contains("code explorer"),
        "catalog leaks explore instructions"
    );
    assert!(catalog.contains("<kuku_agent_catalog>"));
    assert!(catalog.contains("- review:"));
    assert!(catalog.contains("- explore:"));
    assert!(catalog.contains("Available contacts:"));
}

#[test]
fn directory_does_not_leak_full_instructions() {
    let registry = kuku::agent::registry::AgentRegistry::builder()
        .builtins(&builtin_prompt_catalog())
        .build();
    let directory = render_agent_directory(&registry, 3).expect("directory should render");
    assert!(directory.contains("Available contacts:"));
    assert!(directory.contains("routing hint:"));
    assert!(directory.contains("open conversations: 3"));
    assert!(!directory.contains("You are a code and document reviewer."));
    assert!(!directory.contains("You are a code explorer."));
}
