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

impl Default for ServerArgs {
    fn default() -> Self {
        Self {
            listen: "0.0.0.0:17777".to_owned(),
            config: None,
            auth_token_file: None,
            allow_origin: Vec::new(),
            registration_root: Vec::new(),
            max_concurrent_runs: 16,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ServerArgs;

    #[test]
    fn rust_defaults_match_cli_defaults() {
        let args = ServerArgs::default();

        assert_eq!("0.0.0.0:17777", args.listen);
        assert_eq!(16, args.max_concurrent_runs);
        assert!(args.config.is_none());
        assert!(args.auth_token_file.is_none());
        assert!(args.allow_origin.is_empty());
        assert!(args.registration_root.is_empty());
    }
}
