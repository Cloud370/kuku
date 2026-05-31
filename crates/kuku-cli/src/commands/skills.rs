use kuku::skill::registry::SkillRegistry;

use crate::cli_args::{SkillsArgs, SkillsSubcommand};
use crate::display::util::truncate;

fn build_registry() -> Result<SkillRegistry, Box<dyn std::error::Error>> {
    let workspace = kuku::session::current_workspace()?;
    let config_path = kuku::session::kuku_home()?.join("config.toml");
    let discovery_config = kuku::config::load_config(&config_path)
        .ok()
        .and_then(|f| f.discovery)
        .unwrap_or_default();
    let registry = SkillRegistry::builder()
        .build_with_discovery(&workspace, &discovery_config)?
        .build();
    Ok(registry)
}

pub fn run(args: SkillsArgs) -> Result<(), Box<dyn std::error::Error>> {
    let registry = build_registry()?;

    match args.cmd {
        None | Some(SkillsSubcommand::List) => {
            println!("{:<20} {:<20} DESCRIPTION", "NAME", "SOURCE");
            for def in registry.definitions() {
                println!(
                    "{:<20} {:<20} {}",
                    def.name,
                    def.source.as_str(),
                    truncate(&def.description, 60),
                );
            }
        }
        Some(SkillsSubcommand::Show { name }) => {
            let def = registry.get(&name).ok_or_else(|| {
                format!("skill '{name}' not found. Use `kuku skills list` to see available skills.")
            })?;
            println!("name:            {}", def.name);
            println!("description:     {}", def.description);
            println!("source:          {}", def.source.as_str());
            println!("hash:            {}", def.hash);
            if let Some(ref path) = def.source_path {
                println!("source_path:     {path}");
            }
            if let Some(ref license) = def.license {
                println!("license:         {license}");
            }
            if let Some(ref compat) = def.compatibility {
                println!("compatibility:   {compat}");
            }
            if let Some(ref tools) = def.allowed_tools {
                println!("allowed-tools:   {}", tools.join(", "));
            }
            if let Some(ref tools) = def.disallowed_tools {
                println!("disallowed-tools: {}", tools.join(", "));
            }
            if let Some(mt) = def.max_turns {
                println!("max-turns:       {mt}");
            }
            if let Some(ref model) = def.model {
                println!("model:           {model}");
            }
            println!();
            println!("instructions:");
            println!("{}", def.instructions);
        }
    }
    Ok(())
}
