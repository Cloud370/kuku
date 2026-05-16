use kuku::config::load_config;
use kuku::session::kuku_home;

use crate::cli_args::{ConfigArgs, ConfigSubcommand, PolicySubcommand};

/// Show or manage configuration: `kuku config [validate|policy]`
pub async fn run(args: ConfigArgs) -> Result<(), Box<dyn std::error::Error>> {
    match args.cmd {
        None | Some(ConfigSubcommand::Validate) => {
            let home = kuku_home()?;
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
        Some(ConfigSubcommand::Policy(policy_args)) => match policy_args.cmd {
            PolicySubcommand::Allow { risk: _ } => {
                // TODO: implement policy.md write
                eprintln!("kuku config policy: policy.md write not yet implemented");
                Ok(())
            }
            PolicySubcommand::Deny { risk: _ } => {
                // TODO: implement policy.md write
                eprintln!("kuku config policy: policy.md write not yet implemented");
                Ok(())
            }
        },
    }
}
