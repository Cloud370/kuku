#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("time format error: {0}")]
    TimeFormat(#[from] time::error::Format),

    #[error("missing home directory; set KUKU_HOME")]
    MissingHomeDirectory,

    #[error("invalid KUKU_HOME: {0}")]
    InvalidKukuHome(String),

    #[error("invalid event stream: {0}")]
    InvalidEventStream(String),

    #[error("invalid session id: {0}")]
    InvalidSessionId(String),

    #[error("invalid workspace path: {0}")]
    InvalidWorkspacePath(String),
}

pub type Result<T> = std::result::Result<T, Error>;
