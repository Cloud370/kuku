use kuku::config::load_config;
use kuku::session::kuku_home;

use crate::cli_args::{ConfigArgs, ConfigSubcommand, PolicySubcommand};

/// Show or manage configuration: `kuku config [validate|policy]`
pub async fn run(args: ConfigArgs) -> Result<(), Box<dyn std::error::Error>> {
    let path = if let Some(p) = &args.config {
        std::path::PathBuf::from(p)
    } else {
        kuku_home()?.join("config.toml")
    };

    match args.cmd {
        None | Some(ConfigSubcommand::Validate) => {
            if !path.exists() {
                eprintln!("error: 未找到配置文件 {}", path.display());
                eprintln!("提示: 运行 `kuku init` 初始化配置");
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
