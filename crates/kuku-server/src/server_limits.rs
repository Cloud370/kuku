#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerLimits {
    pub max_concurrent_runs: usize,
    pub http_body_bytes: usize,
    pub max_queued_runs: usize,
    pub max_total_streams: usize,
    pub max_streams_per_task: usize,
    pub max_tasks_per_page: usize,
    pub max_timeline_items: usize,
    pub max_timeline_bytes: usize,
    pub max_request_summaries: usize,
    pub max_delegated_messages: usize,
    pub max_delegated_bytes: usize,
    pub max_request_snapshot_bytes: usize,
}

impl ServerLimits {
    pub const HTTP_BODY_BYTES: usize = 10 * 1024 * 1024;

    pub fn with_max_concurrent_runs(max_concurrent_runs: usize) -> Result<Self, String> {
        if !(1..=64).contains(&max_concurrent_runs) {
            return Err("max concurrent runs must be between 1 and 64".to_owned());
        }
        Ok(Self {
            max_concurrent_runs,
            http_body_bytes: Self::HTTP_BODY_BYTES,
            max_queued_runs: 64,
            max_total_streams: 64,
            max_streams_per_task: 8,
            max_tasks_per_page: 100,
            max_timeline_items: 500,
            max_timeline_bytes: 16 * 1024 * 1024,
            max_request_summaries: 100,
            max_delegated_messages: 500,
            max_delegated_bytes: 10 * 1024 * 1024,
            max_request_snapshot_bytes: 16 * 1024 * 1024,
        })
    }
}

impl Default for ServerLimits {
    fn default() -> Self {
        Self::with_max_concurrent_runs(16).expect("default server limits are valid")
    }
}
