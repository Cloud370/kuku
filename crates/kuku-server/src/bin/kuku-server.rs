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

    let prepared = kuku_server::prepare_server(args)
        .await
        .unwrap_or_else(|error| {
            eprintln!("error: {error}");
            std::process::exit(1);
        });
    prepared.print_connection_info(false);
    println!("warning: LAN plaintext connections are visible to the local network; use external TLS for untrusted networks");
    prepared.serve().await.unwrap_or_else(|error| {
        eprintln!("error: {error}");
        std::process::exit(1);
    });
}
