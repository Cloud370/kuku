use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;
use kuku_server::server_args::ServerArgs;

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

    let config_path = args
        .config
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            home::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("/"))
                .join(".kuku")
                .join("config.toml")
        });

    let config = match config_path
        .exists()
        .then(|| kuku::config::load_and_patch_config(&config_path).and_then(|f| f.resolve()))
        .transpose()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: failed to load config: {e}");
            std::process::exit(1);
        }
    };

    let kuku_home = std::env::var_os("KUKU_HOME")
        .map(PathBuf::from)
        .or_else(|| home::home_dir().map(|dir| dir.join(".kuku")))
        .unwrap_or_else(|| PathBuf::from(".kuku"));

    let state = kuku_server::AppState::open(
        &kuku_home,
        config,
        args.auth_token_file
            .map(std::path::PathBuf::from)
            .map(std::fs::read_to_string)
            .transpose()
            .unwrap_or_else(|error| {
                eprintln!("error: failed to read auth token: {error}");
                std::process::exit(1)
            }),
        vec![kuku_server::platform::RegistrationRootSpec {
            label: "Current directory".to_owned(),
            path: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }],
        format!("http://{listen_addr}"),
        args.max_concurrent_runs,
    )
    .await
    .unwrap_or_else(|e| {
        eprintln!("error: failed to initialize server: {e:?}");
        std::process::exit(1);
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
