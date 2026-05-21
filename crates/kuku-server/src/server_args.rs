use clap::Parser;

#[derive(Parser)]
#[command(name = "kuku-server", about = "HTTP API host for kuku SDK")]
pub struct ServerArgs {
    #[arg(long, default_value = "127.0.0.1:17777")]
    pub listen: String,

    #[arg(long)]
    pub config: Option<String>,

    #[arg(long)]
    pub password: Option<String>,

    #[arg(long, default_value = "16")]
    pub max_concurrent_runs: usize,
}
