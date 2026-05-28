use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;
use kuku_server::run_manager::RunManager;
use kuku_server::server_args::ServerArgs;
use tokio::sync::Mutex;

#[tokio::main]
async fn main() {
    let args = ServerArgs::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let listen_addr: SocketAddr = match args.listen.parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: invalid listen address: {e}");
            std::process::exit(1);
        }
    };

    if !listen_addr.ip().is_loopback() && args.password.is_none() {
        eprintln!("error: --password is required for non-loopback addresses");
        std::process::exit(1);
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
        eprintln!("error: config file not found: {}", config_path.display());
        std::process::exit(1);
    }

    let config = match kuku::config::load_and_patch_config(&config_path).and_then(|f| f.resolve()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: failed to load config: {e}");
            std::process::exit(1);
        }
    };

    let config_store = std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(config));
    let _watcher = kuku_server::config_watcher::ConfigWatcher::start(
        config_path.clone(),
        config_store.clone(),
    );

    let state = Arc::new(kuku_server::AppState {
        run_manager: Mutex::new(RunManager::new(args.max_concurrent_runs)),
        config: config_store,
        password: args.password,
    });

    let app = kuku_server::build_app(state.clone());

    let listener = tokio::net::TcpListener::bind(listen_addr)
        .await
        .unwrap_or_else(|e| {
            eprintln!("error: failed to bind {listen_addr}: {e}");
            std::process::exit(1);
        });

    tracing::info!("listening on {listen_addr}");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(kuku_server::shutdown_signal(state.clone()))
    .await
    .unwrap();
}
