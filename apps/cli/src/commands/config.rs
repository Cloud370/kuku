use clap::Subcommand;

use kuku::config::load_config;

#[derive(Subcommand)]
pub enum ConfigSubcommand {
    /// Show redacted config
    Show,
    /// Validate config file
    Validate,
}

pub async fn run(cmd: ConfigSubcommand) -> Result<(), Box<dyn std::error::Error>> {
    match cmd {
        ConfigSubcommand::Show => {
            let home = kuku::session::kuku_home()?;
            let path = home.join("config.toml");
            let file = load_config(&path)?;
            let cfg = file.resolve()?;
            println!("{}", cfg.redacted_display());
            Ok(())
        }
        ConfigSubcommand::Validate => {
            let home = kuku::session::kuku_home()?;
            let path = home.join("config.toml");
            if !path.exists() {
                eprintln!("No config file at {}", path.display());
                eprintln!("Create ~/.kuku/config.toml to configure models and providers.");
                std::process::exit(1);
            }
            let file = load_config(&path)?;
            file.resolve()?;
            println!("Config is valid.");
            Ok(())
        }
    }
}
