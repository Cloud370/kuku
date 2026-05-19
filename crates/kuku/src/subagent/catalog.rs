use super::registry::SubagentRegistry;

/// Render the agent catalog block for injection into runtime_context.
/// Returns None if the registry is empty.
pub fn render_agent_catalog(registry: &SubagentRegistry) -> Option<String> {
    if registry.is_empty() {
        return None;
    }

    let mut entries = String::new();
    for def in registry.definitions() {
        entries.push_str(&format!(
            "  <agent name=\"{name}\" source=\"{source}\" tier=\"{tier}\" tools=\"{tools}\" max_turns=\"{max_turns}\" hash=\"{hash}\">\n    <description>{description}</description>\n  </agent>\n",
            name = def.name,
            source = def.source.as_str(),
            tier = def.tier,
            tools = def.tool_profile.as_str(),
            max_turns = def.max_turns,
            hash = def.hash,
            description = def.description,
        ));
    }

    Some(format!(
        "<kuku_agent_catalog version=\"{version}\">\n{entries}</kuku_agent_catalog>",
        version = registry.hash(),
        entries = entries,
    ))
}

/// Render the full agent definition block for a child session's user message.
pub fn render_agent_definition_block(def: &super::definition::SubagentDefinition) -> String {
    format!(
        "<kuku_agent_definition name=\"{name}\" source=\"{source}\" hash=\"{hash}\">\n  <kuku_agent_instructions>\n{instructions}\n  </kuku_agent_instructions>\n</kuku_agent_definition>",
        name = def.name,
        source = def.source.as_str(),
        hash = def.hash,
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
        assert!(catalog.contains("name=\"review\""));
        assert!(catalog.contains("name=\"explore\""));
        assert!(catalog.contains("source=\"builtin\""));
        assert!(catalog.contains("tools=\"read\""));
        assert!(
            !catalog.contains("instructions"),
            "catalog must NOT include full instructions"
        );
    }

    #[test]
    fn catalog_is_none_for_empty_registry() {
        let registry = SubagentRegistry::builder().build();
        assert!(render_agent_catalog(&registry).is_none());
    }

    #[test]
    fn definition_block_includes_full_instructions() {
        let review = SubagentRegistry::builder()
            .builtins()
            .build()
            .get("review")
            .cloned()
            .unwrap();
        let block = render_agent_definition_block(&review);
        assert!(block.contains("<kuku_agent_definition"));
        assert!(block.contains("<kuku_agent_instructions>"));
        assert!(block.contains("code and document reviewer"));
        assert!(block.contains(review.instructions.as_str()));
        assert!(!block.contains("kuku_agent_output_contract"));
    }
}
