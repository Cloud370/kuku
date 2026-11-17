//! Arguments accepted by the explicit server and managed child entrypoints.

use std::path::PathBuf;

use clap::Parser;

/// Select the listener, configuration and credentials for an HTTP service.
#[derive(Parser)]
#[command(name = "kuku-server", about = "HTTP API host for kuku SDK")]
pub struct ServerArgs {
    /// TCP socket for the HTTP service.
    #[arg(long, default_value = "127.0.0.1:17777")]
    pub listen: String,

    /// Configuration file read by the server.
    #[arg(long)]
    pub config: Option<PathBuf>,

    /// Data directory that owns Sessions and local server discovery.
    #[arg(long)]
    pub data_dir: Option<PathBuf>,

    /// Access token for an explicitly started service.
    #[arg(long)]
    pub password: Option<String>,

    /// Maximum number of concurrently advancing Agent runs.
    #[arg(long, default_value = "16")]
    pub max_concurrent_runs: usize,

    /// Start the local service and publish private discovery.
    #[arg(long, hide = true)]
    pub managed: bool,
}
