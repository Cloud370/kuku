use kuku::subagent::registry::SubagentRegistry;

#[test]
fn builtin_registry_is_loadable_and_queryable() {
    let registry = SubagentRegistry::builder().builtins().build();
    assert_eq!(registry.len(), 2);

    let review = registry.get("review").expect("review should exist");
    assert_eq!(review.tier, "balanced");
    assert_eq!(review.max_turns, 4);
    assert_eq!(review.tool_profile.as_str(), "read");

    let explore = registry.get("explore").expect("explore should exist");
    assert_eq!(explore.tier, "light");
    assert_eq!(explore.max_turns, 3);
}

#[test]
fn registry_hash_is_stable() {
    let r1 = SubagentRegistry::builder().builtins().build();
    let r2 = SubagentRegistry::builder().builtins().build();
    assert_eq!(r1.hash(), r2.hash());
    assert_eq!(r1.names(), r2.names());
}
