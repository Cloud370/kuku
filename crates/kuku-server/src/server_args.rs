use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "kuku-server", about = "HTTP API host for kuku SDK")]
pub struct ServerArgs {
    #[arg(long, default_value = "0.0.0.0:17777")]
    pub listen: String,

    #[arg(long)]
    pub config: Option<PathBuf>,

    #[arg(long)]
    pub auth_token_file: Option<PathBuf>,

    #[arg(long = "allow-origin")]
    pub allow_origin: Vec<String>,

    #[arg(long = "registration-root", value_name = "LABEL=PATH")]
    pub registration_root: Vec<String>,

    #[arg(long, default_value = "16")]
    pub max_concurrent_runs: usize,
}
