use std::error::Error;

use clap::Parser;
use kuku_cli::cli_args::{Cli, Command};
use kuku_cli::commands::service::manage;

mod web;

#[tokio::main]
async fn main() {
    if let Err(error) = run(Cli::parse()).await {
        eprintln!("error: {error}");
        std::process::exit(kuku_cli::commands::exit_status(error.as_ref()));
    }
}

async fn run(cli: Cli) -> Result<(), Box<dyn Error>> {
    match cli.command {
        None => kuku_tui::run(cli.target.options(), kuku_server::BUILD_VERSION).await?,
        Some(Command::ManagedServer(args)) => {
            kuku_server::service::serve_managed(args.into_server_args(&cli.target), web::assets())
                .await?;
        }
        Some(Command::Server(args)) => match args.command {
            Some(command) => manage(command, cli.target.options()).await?,
            None => {
                if cli.target.remote.is_some() {
                    return Err("start the foreground server on its host with `kuku server --listen ADDRESS`".into());
                }
                kuku_server::service::serve(args.into_server_args(&cli.target), web::assets())
                    .await?;
            }
        },
        Some(command) => kuku_cli::commands::dispatch(command, &cli.target).await?,
    }
    Ok(())
}
