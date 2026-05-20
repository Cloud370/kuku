use kuku::skill::registry::SkillRegistry;

use crate::cli_args::{SkillsArgs, SkillsSubcommand};

fn build_registry() -> Result<SkillRegistry, Box<dyn std::error::Error>> {
    let workspace = kuku::session::current_workspace()?;
    let registry = SkillRegistry::builder()
        .load_claude_user_skills()?
        .load_claude_project_skills(&workspace)?
        .load_opencode_user_skills()?
        .load_opencode_project_skills(&workspace)?
        .load_kuku_user_skills()?
        .load_kuku_project_skills(&workspace)?
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

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}...", s.chars().take(max).collect::<String>())
    }
}
