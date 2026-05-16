mod commands;
mod display;
mod view;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "kuku", version, about = "file-native agent runtime")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// The prompt to run (default action)
    #[arg(trailing_var_arg = true)]
    prompt: Vec<String>,

    /// Print mode: output final text only, deny all permission requests
    #[arg(short = 'p')]
    print_mode: bool,

    /// Model alias
    #[arg(long = "model")]
    model: Option<String>,

    /// Continue an existing session
    #[arg(short = 's', long = "session")]
    session: Option<String>,

    /// Continue the most recent session
    #[arg(short = 'c', long = "continue")]
    cont: bool,
}

#[derive(Subcommand)]
enum Command {
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
        Some(Command::Show { cmd }) => commands::show::run(cmd).await,
        Some(Command::List(args)) => commands::show::list(args).await,
        None => {
            if cli.prompt.is_empty() {
                eprintln!("Usage: kuku [OPTIONS] <PROMPT>");
                eprintln!("       kuku show output|events <SESSION_ID>");
                eprintln!("       kuku list");
                eprintln!("\nRun 'kuku --help' for more information.");
                return;
            }
            let args = commands::query::QueryArgs {
                prompt: cli.prompt,
                print_mode: cli.print_mode,
                model: cli.model,
                session: cli.session,
                cont: cli.cont,
            };
            commands::query::run(args).await
        }
    };

    if let Err(err) = result {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
