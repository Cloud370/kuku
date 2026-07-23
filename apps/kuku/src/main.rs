use clap::Parser;
use kuku_cli::cli_args::{Cli, Command};

mod web;

fn command_or_default(command: Option<Command>) -> Command {
    command.unwrap_or_else(|| Command::Web(Default::default()))
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let result = match command_or_default(cli.command) {
        Command::Run(args) => kuku_cli::commands::run::run(args).await,
        Command::Show(args) => kuku_cli::commands::show::run(args).await,
        Command::Events(args) => kuku_cli::commands::events::run(args).await,
        Command::List(args) => kuku_cli::commands::list::run(args).await,
        Command::Delete(args) => kuku_cli::commands::delete::run(args).await,
        Command::Config(args) => kuku_cli::commands::config::run(args).await,
        Command::Init => kuku_cli::commands::init::run(),
        Command::Prompts(args) => kuku_cli::commands::prompts::run(args),
        Command::Agents(args) => kuku_cli::commands::agents::run(args),
        Command::Skills(args) => kuku_cli::commands::skills::run(args),
        Command::Server(args) => web::run_server(args, false).await,
        Command::Web(args) => web::run_server(args, true).await,
    };

    if let Err(err) = result {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_command_defaults_to_web() {
        assert!(matches!(command_or_default(None), Command::Web(_)));
    }
}
