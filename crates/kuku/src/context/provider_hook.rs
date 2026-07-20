use std::future::Future;
use std::time::Instant;

use crate::event::{Cursor, RequestStarted};

use crate::context::{
    ContextFactSinkError, RequestEvidenceRecorder, RequestSnapshotBuilder, SnapshotBuildError,
    SnapshotInput,
};

/// Failure while preparing or durably recording provider request evidence.
#[derive(Debug, thiserror::Error)]
pub enum ProviderHookError {
    /// The exact request could not be built or exceeded its size bound.
    #[error(transparent)]
    Snapshot(#[from] SnapshotBuildError),
    /// The snapshot/start pair could not be appended durably.
    #[error(transparent)]
    Evidence(#[from] ContextFactSinkError),
}

/// Records immutable request evidence before polling a provider transport future.
pub async fn begin_provider_request<T, F>(
    recorder: &dyn RequestEvidenceRecorder,
    input: SnapshotInput<'_>,
    started: RequestStarted,
    provider_call: F,
) -> Result<(Instant, T), ProviderHookError>
where
    F: Future<Output = T>,
{
    let snapshot = RequestSnapshotBuilder::build(input)?;
    let started_at = Instant::now();
    let _cursor: Cursor = recorder.record_before_provider(snapshot, started)?;
    Ok((started_at, provider_call.await))
}
