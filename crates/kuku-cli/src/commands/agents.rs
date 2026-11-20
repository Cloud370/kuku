use kuku::agent::registry::AgentRegistry;
use kuku::prompt::builtin_prompt_catalog;

use crate::cli_args::{AgentsArgs, AgentsSubcommand, ConnectionArgs};
use crate::display::util::truncate;

fn build_registry(target: &ConnectionArgs) -> Result<AgentRegistry, Box<dyn std::error::Error>> {
    let workspace = target.workspace()?;
    let config_path = super::config_path(target)?;
    let discovery_config = kuku::config::load_config(&config_path)
        .ok()
        .and_then(|f| f.discovery)
        .unwrap_or_default();
    let registry = AgentRegistry::builder()
        .builtins(&builtin_prompt_catalog())
        .build_with_discovery(&workspace, &discovery_config)?
        .build();
    Ok(registry)
}

pub fn run(args: AgentsArgs, target: &ConnectionArgs) -> Result<(), Box<dyn std::error::Error>> {
    let registry = build_registry(target)?;

    match args.cmd {
        None | Some(AgentsSubcommand::List) => {
            println!(
                "{:<16} {:<20} {:<10} {:<12} DESCRIPTION",
                "NAME", "SOURCE", "TIER", "TOOLS"
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
                format!("agent '{name}' not found. Use `kuku agents list` to see available contact cards.")
            })?;
            println!("contact card:");
            println!("name:            {}", def.name);
            println!("description:     {}", def.description);
            println!("source:          {}", def.source.as_str());
            println!("tier:            {}", def.tier);
            println!("tool_profile:    {}", def.tool_profile.as_str());
            println!("max_turns:       {}", def.max_turns);
            println!("hash:            {}", def.hash);
            if let Some(ref path) = def.source_path {
                println!("source_path:     {path}");
            }
            println!();
            println!("full instructions:");
            println!("{}", def.instructions);
        }
    }
    Ok(())
}
