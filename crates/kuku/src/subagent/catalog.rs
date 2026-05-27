use super::registry::SubagentRegistry;

/// Render the agent catalog block for injection into runtime_context.
/// Returns None if the registry is empty.
pub fn render_agent_catalog(registry: &SubagentRegistry) -> Option<String> {
    if registry.is_empty() {
        return None;
    }

    let mut entries = String::new();
    for def in registry.definitions() {
        let path = def
            .source_path
            .as_deref()
            .unwrap_or(match def.source.as_str() {
                "builtin" => "(builtin)",
                s => s,
            });
        entries.push_str(&format!(
            "- {} — {} ({}, {}, {} turns)\n",
            def.name,
            def.description,
            path,
            def.tool_profile.as_str(),
            def.max_turns,
        ));
    }

    Some(format!(
        "<kuku_agent_catalog>\nAvailable agents:\n{entries}</kuku_agent_catalog>",
        entries = entries,
    ))
}

/// Render the full agent definition block for a child session's user message.
pub fn render_agent_definition_block(def: &super::definition::SubagentDefinition) -> String {
    let path = def
        .source_path
        .as_deref()
        .unwrap_or(def.source.as_str());
    format!(
        "<!-- loaded: {path} -->\n\n{instructions}",
        path = path,
        instructions = def.instructions,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subagent::registry::SubagentRegistry;

    #[test]
    fn catalog_renders_builtin_agents() {
        let registry = SubagentRegistry::builder().builtins().build();
        let catalog = render_agent_catalog(&registry).expect("catalog should render");
        assert!(catalog.contains("<kuku_agent_catalog"));
        assert!(catalog.contains("Available agents:"));
        assert!(catalog.contains("- review —"));
        assert!(catalog.contains("- explore —"));
        assert!(catalog.contains("(builtin"));
        assert!(
            !catalog.contains("instructions"),
            "catalog must NOT include full instructions"
        );
        assert!(!catalog.contains("<agent "), "no XML agent tags");
    }

    #[test]
    fn catalog_is_none_for_empty_registry() {
        let registry = SubagentRegistry::builder().build();
        assert!(render_agent_catalog(&registry).is_none());
    }

    #[test]
    fn definition_block_uses_loaded_comment_format() {
        let review = SubagentRegistry::builder()
            .builtins()
            .build()
            .get("review")
            .cloned()
            .unwrap();
        let block = render_agent_definition_block(&review);
        assert!(block.contains("<!-- loaded: "));
        assert!(block.contains(review.instructions.as_str()));
        assert!(!block.contains("kuku_agent_definition"));
        assert!(!block.contains("kuku_agent_instructions"));
    }
}
