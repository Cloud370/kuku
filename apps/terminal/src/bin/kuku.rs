use clap::Parser;
use kuku_terminal::cli_args::{Cli, Command};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Some(Command::Run(args)) => kuku_terminal::commands::run::run(args).await,
        Some(Command::Show(args)) => kuku_terminal::commands::show::run(args).await,
        Some(Command::Events(args)) => kuku_terminal::commands::events::run(args).await,
        Some(Command::List(args)) => kuku_terminal::commands::list::run(args).await,
        Some(Command::Config(args)) => kuku_terminal::commands::config::run(args).await,
        Some(Command::Init) => kuku_terminal::commands::init::run(),
        Some(Command::Prompts(args)) => kuku_terminal::commands::prompts::run(args),
        None => kuku_terminal::commands::run::interactive(None).await,
    };

    if let Err(err) = result {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
