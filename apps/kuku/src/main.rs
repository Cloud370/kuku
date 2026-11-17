use std::error::Error;

use clap::Parser;
use kuku_cli::cli_args::{Cli, Command};
use kuku_cli::commands::service::manage;

mod web;

#[tokio::main]
async fn main() {
    if let Err(error) = run(Cli::parse()).await {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), Box<dyn Error>> {
    match cli.command {
        None => kuku_tui::run(cli.target.options()).await?,
        Some(Command::ManagedServer(args)) => {
            let mut args = args.into_server_args(&cli.target);
            args.managed = true;
            kuku_server::service::serve(args, web::assets()).await?;
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
        Some(Command::Web(args)) => {
            let connection = cli.target.options().connect().await?;
            let session = args.session.as_deref();
            let agent = args.agent.as_deref();
            let url = if connection.local.is_some() {
                connection
                    .client
                    .browser_url(&connection.workspace, session, agent)
                    .await?
            } else {
                connection
                    .client
                    .web_url(&connection.workspace, session, agent)?
            };
            if args.print_url {
                println!("{url}");
            } else {
                kuku_tui::open_browser(url.as_str())?;
            }
        }
        Some(command) => kuku_cli::commands::dispatch(command, &cli.target).await?,
    }
    Ok(())
}
