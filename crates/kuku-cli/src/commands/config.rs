use kuku::adapter::inspect_model;
use kuku::config::{load_config, set_value, show_redacted};
use kuku::session::AgentConfig;
use kuku::Error;

use crate::cli_args::{ConfigArgs, ConfigSubcommand, ConnectionArgs, PolicySubcommand};

/// Show or manage configuration: `kuku config [show|validate|set|policy]`
pub async fn run(
    args: ConfigArgs,
    target: &ConnectionArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = super::config_path(target)?;

    match args.cmd {
        None | Some(ConfigSubcommand::Show) => {
            if !path.exists() {
                eprintln!("error: config file not found: {}", path.display());
                eprintln!("hint: run `kuku init` to open interactive settings");
                std::process::exit(1);
            }
            let output = show_redacted(&path)?;
            print!("{output}");
            Ok(())
        }
        Some(ConfigSubcommand::Models) => {
            let path = if target.config.is_none() {
                target
                    .options()
                    .local_service()?
                    .status()
                    .await?
                    .and_then(|connection| {
                        connection
                            .status()
                            .config_path
                            .as_deref()
                            .map(std::path::PathBuf::from)
                    })
                    .unwrap_or(path)
            } else {
                path
            };
            let config = load_config(&path)?.resolve()?;
            let routes = config
                .tiers
                .iter()
                .map(|(name, tier)| {
                    let selection = inspect_model(
                        &config,
                        &AgentConfig {
                            tier: Some(name.clone()),
                            ..AgentConfig::default()
                        },
                    );
                    let mut route = serde_json::json!({"name": name, "purpose": tier.purpose});
                    match selection {
                        Ok(selection) => route["selection"] = serde_json::to_value(selection)?,
                        Err(error) => route["error"] = serde_json::Value::String(error.to_string()),
                    }
                    Ok::<_, serde_json::Error>(route)
                })
                .collect::<Result<Vec<_>, _>>()?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "default_tier": config.default_tier,
                    "tiers": routes,
                }))?
            );
            Ok(())
        }
        Some(ConfigSubcommand::Validate) => {
            if !path.exists() {
                eprintln!("error: config file not found: {}", path.display());
                eprintln!("hint: run `kuku init` to open interactive settings");
                std::process::exit(1);
            }
            let config = load_config(&path)?.resolve()?;
            for tier in config.tiers.keys() {
                let selection = AgentConfig {
                    tier: Some(tier.clone()),
                    ..AgentConfig::default()
                };
                inspect_model(&config, &selection)
                    .map_err(|error| Error::ConfigLoad(format!("tier '{tier}': {error}")))?;
            }
            println!("Config is valid.");
            Ok(())
        }
        Some(ConfigSubcommand::Set { key, value }) => {
            if !path.exists() {
                eprintln!("error: config file not found: {}", path.display());
                eprintln!("hint: run `kuku init` to open interactive settings");
                std::process::exit(1);
            }
            set_value(&path, &key, &value)?;
            let display_value = if key.contains("api_key") {
                if value.starts_with('$') {
                    value.clone()
                } else {
                    "<redacted>".to_string()
                }
            } else if value.parse::<i64>().is_ok() {
                value.clone()
            } else {
                format!("\"{value}\"")
            };
            println!("updated {key} = {display_value}");
            Ok(())
        }
        Some(ConfigSubcommand::Policy(policy_args)) => match policy_args.cmd {
            PolicySubcommand::Allow { risk: _ } => {
                eprintln!("kuku config policy: policy.md write not yet implemented");
                Ok(())
            }
            PolicySubcommand::Deny { risk: _ } => {
                eprintln!("kuku config policy: policy.md write not yet implemented");
                Ok(())
            }
        },
    }
}
