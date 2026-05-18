use kuku::subagent::registry::SubagentRegistry;

use crate::cli_args::{AgentsArgs, AgentsSubcommand};

fn build_registry() -> Result<SubagentRegistry, Box<dyn std::error::Error>> {
    let workspace = kuku::session::current_workspace()?;
    let registry = SubagentRegistry::builder()
        .builtins()
        .load_claude_user_agents()?
        .load_claude_project_agents(&workspace)?
        .load_opencode_user_agents()?
        .load_opencode_project_agents(&workspace)?
        .build();
    Ok(registry)
}

pub fn run(args: AgentsArgs) -> Result<(), Box<dyn std::error::Error>> {
    let registry = build_registry()?;

    match args.cmd {
        None | Some(AgentsSubcommand::List) => {
            println!(
                "{:<16} {:<20} {:<10} {:<12} {}",
                "NAME", "SOURCE", "TIER", "TOOLS", "DESCRIPTION"
            );
            for def in registry.definitions() {
                println!(
                    "{:<16} {:<20} {:<10} {:<12} {}",
                    def.name,
                    def.source.as_str(),
                    def.tier,
                    def.tool_profile.as_str(),
                    truncate(&def.description, 60),
                );
            }
        }
        Some(AgentsSubcommand::Show { name }) => {
            let def = registry.get(&name).ok_or_else(|| {
                format!("agent '{name}' not found. Use `kuku agents list` to see available agents.")
            })?;
            println!("name:            {}", def.name);
            println!("description:     {}", def.description);
            println!("source:          {}", def.source.as_str());
            println!("tier:            {}", def.tier);
            println!("tool_profile:    {}", def.tool_profile.as_str());
            println!("max_turns:       {}", def.max_turns);
            println!("output_contract: {}", def.output_contract.as_str());
            println!("hash:            {}", def.hash);
            if let Some(ref path) = def.source_path {
                println!("source_path:     {path}");
            }
            println!();
            println!("instructions:");
            println!("{}", def.instructions);
        }
    }
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}...", s.chars().take(max).collect::<String>())
    }
}
