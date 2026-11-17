use clap::Parser;
use kuku_cli::cli_args::Cli;

#[tokio::main(worker_threads = 4)]
async fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Some(command) => kuku_cli::commands::dispatch(command, &cli.target).await,
        None => Err(
            "use `kuku-cli run --json PROMPT` for scripts or `kuku-cli --help` for commands".into(),
        ),
    };

    if let Err(err) = result {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
