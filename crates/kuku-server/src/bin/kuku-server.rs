use clap::Parser;
use kuku_server::server_args::ServerArgs;

#[tokio::main]
async fn main() {
    if let Err(error) = kuku_server::service::serve(ServerArgs::parse(), None).await {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
