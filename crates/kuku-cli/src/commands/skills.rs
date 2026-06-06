use kuku::skill::registry::SkillRegistry;

use crate::cli_args::{SkillsArgs, SkillsSubcommand};
use crate::display::util::truncate;

fn build_registry() -> Result<SkillRegistry, Box<dyn std::error::Error>> {
    let workspace = kuku::session::current_workspace()?;
    let kuku_home = kuku::session::kuku_home()?;
    build_registry_for(&workspace, &kuku_home)
}

fn build_registry_for(
    workspace: &std::path::Path,
    kuku_home: &std::path::Path,
) -> Result<SkillRegistry, Box<dyn std::error::Error>> {
    let config_path = kuku_home.join("config.toml");
    let config = kuku::config::load_config(&config_path)
        .ok()
        .and_then(|file| file.resolve().ok())
        .unwrap_or_else(|| kuku::config::Config {
            tiers: std::collections::BTreeMap::new(),
            providers: std::collections::BTreeMap::new(),
            default_tier: String::new(),
            discovery: kuku::config::DiscoveryConfig::default(),
            handoff: kuku::config::HandoffConfig::default(),
            logs: kuku::config::LogsConfig::default(),
            plugin: kuku::config::PluginConfig::default(),
            update: kuku::config::UpdateConfig::default(),
        });
    Ok(kuku::skill::build_registry_snapshot_for_host(
        kuku_home, workspace, &config,
    )?)
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

#[cfg(test)]
mod tests {
    use super::build_registry_for;

    fn temp_dir(prefix: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "{}-{}",
            prefix,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn build_registry_for_includes_package_skills() {
        let workspace = temp_dir("kuku-skills-workspace");
        let home = temp_dir("kuku-skills-home");

        std::fs::write(home.join("config.toml"), kuku::config::generate_default()).unwrap();

        let pkg_dir = workspace
            .join(".kuku")
            .join("packages")
            .join("pkg-with-skill");
        std::fs::create_dir_all(pkg_dir.join("skills").join("packaged-skill")).unwrap();
        std::fs::write(
            pkg_dir.join("kuku.toml"),
            "[package]\nname = \"pkg-with-skill\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        std::fs::write(
            pkg_dir
                .join("skills")
                .join("packaged-skill")
                .join("SKILL.md"),
            "---\nname: packaged-skill\ndescription: From package\n---\n\n# Packaged\n",
        )
        .unwrap();

        let registry = build_registry_for(&workspace, &home).unwrap();

        assert!(registry.get("packaged-skill").is_some());

        let _ = std::fs::remove_dir_all(workspace);
        let _ = std::fs::remove_dir_all(home);
    }
}
