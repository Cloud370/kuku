#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("invalid event stream: {0}")]
    InvalidEventStream(String),

    #[error("invalid workspace path: {0}")]
    InvalidWorkspacePath(String),
}

pub type Result<T> = std::result::Result<T, Error>;
