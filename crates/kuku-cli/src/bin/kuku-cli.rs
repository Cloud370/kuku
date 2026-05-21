use clap::Parser;
use kuku_cli::cli_args::{Cli, Command};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Some(Command::Run(args)) => kuku_cli::commands::run::run(args).await,
        Some(Command::Show(args)) => kuku_cli::commands::show::run(args).await,
        Some(Command::Events(args)) => kuku_cli::commands::events::run(args).await,
        Some(Command::List(args)) => kuku_cli::commands::list::run(args).await,
        Some(Command::Config(args)) => kuku_cli::commands::config::run(args).await,
        Some(Command::Init) => kuku_cli::commands::init::run(),
        Some(Command::Prompts(args)) => kuku_cli::commands::prompts::run(args),
        Some(Command::Agents(args)) => kuku_cli::commands::agents::run(args),
        Some(Command::Skills(args)) => kuku_cli::commands::skills::run(args),
        #[cfg(feature = "server")]
        Some(Command::Server(_args)) => {
            eprintln!("error: use `kuku server` instead of `kuku-cli server`");
            std::process::exit(1);
        }
        None => kuku_cli::commands::run::interactive(None).await,
    };

    if let Err(err) = result {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
