mod commands;
mod display;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "kuku", about = "file-native agent runtime")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Execute an agent query
    Query(commands::query::QueryArgs),
    /// Show session details (output, events)
    Show {
        #[command(subcommand)]
        cmd: commands::show::ShowCommand,
    },
    /// List sessions for current workspace
    List(commands::show::ListArgs),
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Query(args) => commands::query::run(args).await,
        Command::Show { cmd } => commands::show::run(cmd).await,
        Command::List(args) => commands::show::list(args).await,
    };
    if let Err(err) = result {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
