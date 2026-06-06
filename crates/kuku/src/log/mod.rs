mod hub;
mod paths;
mod prune;
mod store;
mod types;

pub use hub::LogHub;
pub use paths::{host_log_path, logs_root, runtime_log_path, session_log_path};
pub use prune::{
    prune_logs, select_prunable_files, PruneCandidate, PruneOptions, PrunePlan, StartupPruneGate,
};
pub use store::BufferedLogWriter;
pub use types::{HostKind, LogLevel, LogRecord, LogScope};
