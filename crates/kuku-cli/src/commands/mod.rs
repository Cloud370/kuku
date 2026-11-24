//! CLI subcommand implementations.

pub mod agents;
mod auth;
pub mod config;
pub mod delete;
pub mod events;
pub mod list;
pub mod pending;
pub mod prompts;
pub mod run;
pub mod service;
pub mod show;
pub mod skills;

use crate::cli_args::{Command, ConnectionArgs};

/// The common exit contract for both command-line entrypoints.
pub type CliResult = Result<(), Box<dyn std::error::Error>>;

/// Dispatch a script or local configuration command from either binary.
pub async fn dispatch(command: Command, target: &ConnectionArgs) -> CliResult {
    match command {
        Command::Run(args) => run::run(args, target).await,
        Command::Reply(args) => run::reply(args, target).await,
        Command::Show(args) => show::run(args, target).await,
        Command::Events(args) => events::run(args, target).await,
        Command::List(args) => list::run(args, target).await,
        Command::Delete(args) => delete::run(args, target).await,
        Command::Pending(args) => pending::run(args, target).await,
        Command::Config(args) => {
            require_local(target)?;
            config::run(args, target).await
        }
        Command::Init => {
            require_local(target)?;
            kuku_tui::setup(target.options()).await?;
            Ok(())
        }
        Command::Auth(args) => {
            require_local(target)?;
            auth::run(args.command, target).await
        }
        Command::Prompts(args) => {
            require_local(target)?;
            prompts::run(args)
        }
        Command::Agents(args) => {
            require_local(target)?;
            agents::run(args, target)
        }
        Command::Skills(args) => {
            require_local(target)?;
            skills::run(args, target)
        }
        Command::Server(args) => service::standalone(args, target).await,
        Command::ManagedServer(args) => service::managed(args, target).await,
        #[cfg(feature = "server")]
        Command::Web(_) => {
            Err("use the unified `kuku web` binary to open the browser interface".into())
        }
    }
}

fn require_local(target: &ConnectionArgs) -> CliResult {
    if target.remote.is_some() {
        return Err("run this command on the service host to inspect or change its files".into());
    }
    Ok(())
}

fn config_path(target: &ConnectionArgs) -> Result<std::path::PathBuf, kuku_client::Error> {
    target.config.clone().map_or_else(
        || target.data_dir().map(|home| home.join("config.toml")),
        Ok,
    )
}
