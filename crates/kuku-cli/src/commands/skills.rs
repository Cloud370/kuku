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
    let config = kuku::config::load_config(&config_path)?.resolve()?;
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

    #[test]
    fn build_registry_for_excludes_package_skills_when_plugins_are_disabled() {
        let workspace = temp_dir("kuku-skills-disabled-workspace");
        let home = temp_dir("kuku-skills-disabled-home");

        let config = kuku::config::generate_default()
            .replace("[plugin]\nenabled = true", "[plugin]\nenabled = false")
            .replace("auto_discover = true", "auto_discover = false");
        std::fs::write(home.join("config.toml"), config).unwrap();

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

        assert!(registry.get("packaged-skill").is_none());

        let _ = std::fs::remove_dir_all(workspace);
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn build_registry_for_surfaces_config_errors() {
        let workspace = temp_dir("kuku-skills-invalid-workspace");
        let home = temp_dir("kuku-skills-invalid-home");

        std::fs::write(home.join("config.toml"), "[model\ninvalid = true\n").unwrap();

        let error = build_registry_for(&workspace, &home).expect_err("expected config error");

        let _ = std::fs::remove_dir_all(workspace);
        let _ = std::fs::remove_dir_all(home);

        assert!(error.to_string().contains("invalid config"));
    }
}
