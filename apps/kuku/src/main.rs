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
        Some(Command::Delete(args)) => kuku_cli::commands::delete::run(args).await,
        Some(Command::Config(args)) => kuku_cli::commands::config::run(args).await,
        Some(Command::Init) => kuku_cli::commands::init::run(),
        Some(Command::Prompts(args)) => kuku_cli::commands::prompts::run(args),
        Some(Command::Agents(args)) => kuku_cli::commands::agents::run(args),
        Some(Command::Skills(args)) => kuku_cli::commands::skills::run(args),
        Some(Command::Server(args)) => run_server(args).await,
        None => kuku_cli::commands::run::interactive(None).await,
    };

    if let Err(err) = result {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

async fn run_server(
    args: kuku_server::server_args::ServerArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    use kuku_server::run_manager::RunManager;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let listen_addr: SocketAddr = args.listen.parse()?;

    if !listen_addr.ip().is_loopback() && args.password.is_none() {
        return Err("--password is required for non-loopback addresses".into());
    }

    let config_path = args
        .config
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            home::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("/"))
                .join(".kuku")
                .join("config.toml")
        });

    if !config_path.exists() {
        return Err(format!("config file not found: {}", config_path.display()).into());
    }

    let config = kuku::config::load_config(&config_path).and_then(|f| f.resolve())?;

    let config_store = Arc::new(arc_swap::ArcSwap::from_pointee(config));
    let _watcher =
        kuku_server::config_watcher::ConfigWatcher::start(config_path, config_store.clone());

    let state = Arc::new(kuku_server::AppState {
        run_manager: Mutex::new(RunManager::new(args.max_concurrent_runs)),
        config: config_store,
        password: args.password,
    });

    let app = kuku_server::build_app(state.clone());
    let listener = tokio::net::TcpListener::bind(listen_addr).await?;
    tracing::info!("listening on {listen_addr}");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(kuku_server::shutdown_signal(state.clone()))
    .await?;

    Ok(())
}
