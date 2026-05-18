use kuku::subagent::catalog::render_agent_catalog;

#[test]
fn catalog_does_not_leak_full_instructions() {
    let registry = kuku::subagent::registry::SubagentRegistry::builder()
        .builtins()
        .build();
    let catalog = render_agent_catalog(&registry).expect("catalog should render");
    assert!(
        !catalog.contains("code and document reviewer"),
        "catalog leaks review instructions"
    );
    assert!(
        !catalog.contains("code explorer"),
        "catalog leaks explore instructions"
    );
    assert!(catalog.contains("name=\"review\""));
    assert!(catalog.contains("name=\"explore\""));
}
