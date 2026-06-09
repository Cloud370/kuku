use super::definition::AgentDefinition;
use super::registry::AgentRegistry;

pub fn render_agent_catalog(registry: &AgentRegistry) -> Option<String> {
    render_agent_directory(registry, 0)
        .map(|directory| format!("<kuku_agent_catalog>\n{directory}\n</kuku_agent_catalog>"))
}

pub fn render_agent_directory(
    registry: &AgentRegistry,
    open_conversations: usize,
) -> Option<String> {
    if registry.is_empty() {
        return None;
    }

    let mut lines = vec![
        "Available contacts:".to_string(),
        format!("open conversations: {open_conversations}"),
    ];

    for def in registry.definitions() {
        lines.push(format!(
            "- {}: {} | routing hint: {} | source: {} | tools: {} | max turns: {}",
            def.name,
            def.description,
            routing_hint(def),
            display_source(def),
            def.tool_profile.as_str(),
            def.max_turns,
        ));
    }

    Some(lines.join("\n"))
}

pub fn render_agent_definition_block(def: &AgentDefinition) -> String {
    let source = def.source_path.as_deref().unwrap_or(def.source.as_str());
    format!("<!-- agent-source: {source} -->\n\n{}", def.instructions)
}

fn display_source(def: &AgentDefinition) -> &str {
    def.source_path
        .as_deref()
        .unwrap_or(match def.source.as_str() {
            "builtin" => "builtin contact",
            other => other,
        })
}

fn routing_hint(def: &AgentDefinition) -> &'static str {
    match def.name.as_str() {
        "review" => "route correctness and boundary checks here",
        "explore" => "route broad codebase discovery here",
        _ => "route targeted delegated tasks here",
    }
}
