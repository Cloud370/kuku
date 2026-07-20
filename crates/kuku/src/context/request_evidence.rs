use std::sync::Arc;

use crate::event::{Cursor, RequestSnapshot, RequestStarted, TaskEvent};

use super::{ContextFactSink, ContextFactSinkError};

/// Records the pre-transport evidence pair for one provider request.
pub trait RequestEvidenceRecorder: std::fmt::Debug + Send + Sync {
    /// Appends snapshot then lifecycle start in one durable activity batch.
    fn record_before_provider(
        &self,
        snapshot: RequestSnapshot,
        started: RequestStarted,
    ) -> Result<Cursor, ContextFactSinkError>;
}

/// Durable request evidence recorder backed by a Context fact sink.
#[derive(Debug, Clone)]
pub struct DurableRequestEvidenceRecorder {
    sink: Arc<dyn ContextFactSink>,
}

impl DurableRequestEvidenceRecorder {
    /// Creates a recorder over the supplied Task-ledger sink.
    pub fn new(sink: Arc<dyn ContextFactSink>) -> Self {
        Self { sink }
    }
}

impl RequestEvidenceRecorder for DurableRequestEvidenceRecorder {
    fn record_before_provider(
        &self,
        snapshot: RequestSnapshot,
        started: RequestStarted,
    ) -> Result<Cursor, ContextFactSinkError> {
        if snapshot.scope != started.scope
            || snapshot.cause != started.cause
            || snapshot.provider != started.provider
            || snapshot.exact.parameters.model != started.model
        {
            return Err(ContextFactSinkError::MismatchedRequestEvidence);
        }
        self.sink.append_activity(vec![
            TaskEvent::RequestSnapshot(snapshot),
            TaskEvent::RequestStarted(started),
        ])
    }
}
