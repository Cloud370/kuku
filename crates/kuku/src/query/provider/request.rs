#![allow(dead_code)]
use std::path::PathBuf;
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::event::{
    Cursor, EventPayload, EventStore, ProviderFact, ProviderFailureFact,
    ProviderFailureKind as ProviderFailureKindFact, ProviderUsage as ProviderUsageFact,
    RequestCompleted, RequestFailed, RequestScope, RequestStarted, TaskActivityBatch, TaskEvent,
    TaskLedgerRecord, JSON_SAFE_INTEGER_MAX,
};

pub(crate) trait RequestEvidenceRecorder: std::fmt::Debug + Send + Sync {
    fn record_before_provider(&self, started: RequestStarted) -> Result<()>;
    fn record_completed(&self, completed: RequestCompleted) -> Result<()>;
    fn record_failed(&self, failed: RequestFailed) -> Result<()>;

    fn record_exact_before_provider(
        &self,
        _snapshot: crate::event::RequestSnapshot,
        _started: RequestStarted,
    ) -> std::result::Result<Cursor, crate::context::ContextFactSinkError> {
        Err(crate::context::ContextFactSinkError::RecorderUnavailable)
    }
}

pub(crate) async fn begin_provider_request<T>(
    recorder: &dyn RequestEvidenceRecorder,
    started: RequestStarted,
    provider_call: impl std::future::Future<Output = T>,
) -> Result<(std::time::Instant, T)> {
    let started_at = std::time::Instant::now();
    recorder.record_before_provider(started)?;
    Ok((started_at, provider_call.await))
}

#[derive(Debug)]
pub(crate) struct LifecycleOnlyRecorder {
    event_store: Option<EventStore>,
    events_path: PathBuf,
}

impl LifecycleOnlyRecorder {
    pub(crate) fn new(events_path: impl Into<PathBuf>) -> Self {
        Self {
            event_store: None,
            events_path: events_path.into(),
        }
    }

    pub(crate) fn from_store(event_store: EventStore) -> Self {
        Self {
            events_path: event_store.path().to_path_buf(),
            event_store: Some(event_store),
        }
    }

    fn append(&self, event: TaskEvent) -> Result<()> {
        match self.event_store.as_ref() {
            Some(event_store) => append_activity(event_store.clone(), event),
            None => append_activity(EventStore::open(&self.events_path)?, event),
        }
    }
}

impl RequestEvidenceRecorder for LifecycleOnlyRecorder {
    fn record_before_provider(&self, started: RequestStarted) -> Result<()> {
        self.append(TaskEvent::RequestStarted(started))
    }

    fn record_completed(&self, completed: RequestCompleted) -> Result<()> {
        self.append(TaskEvent::RequestCompleted(completed))
    }

    fn record_failed(&self, failed: RequestFailed) -> Result<()> {
        self.append(TaskEvent::RequestFailed(failed))
    }

    fn record_exact_before_provider(
        &self,
        snapshot: crate::event::RequestSnapshot,
        started: RequestStarted,
    ) -> std::result::Result<Cursor, crate::context::ContextFactSinkError> {
        let store = match &self.event_store {
            Some(store) => store.clone(),
            None => EventStore::open(&self.events_path)
                .map_err(crate::context::ContextFactSinkError::Append)?,
        };
        let sink = crate::context::EventStoreContextFactSink::new(store)?;
        <crate::context::DurableRequestEvidenceRecorder as crate::context::RequestEvidenceRecorder>::record_before_provider(
            &crate::context::DurableRequestEvidenceRecorder::new(Arc::new(sink)),
            snapshot,
            started,
        )
    }
}

pub(crate) async fn begin_exact_provider_request<T>(
    recorder: &dyn RequestEvidenceRecorder,
    input: crate::context::SnapshotInput<'_>,
    started: RequestStarted,
    provider_call: impl std::future::Future<Output = T>,
) -> Result<(std::time::Instant, T)> {
    let snapshot = crate::context::RequestSnapshotBuilder::build(input).map_err(|error| {
        Error::InvalidEventStream(format!("request snapshot build failed: {error}"))
    })?;
    let started_at = std::time::Instant::now();
    recorder
        .record_exact_before_provider(snapshot, started)
        .map_err(|error| {
            Error::InvalidEventStream(format!("request evidence append failed: {error}"))
        })?;
    Ok((started_at, provider_call.await))
}

fn append_activity(mut event_store: EventStore, event: TaskEvent) -> Result<()> {
    let batch = TaskActivityBatch::try_new(vec![event]).map_err(|error| {
        Error::InvalidEventStream(format!("invalid request lifecycle activity: {error}"))
    })?;
    event_store.append_synced(EventPayload::TaskLedger(TaskLedgerRecord::Activity(batch)))?;
    Ok(())
}

pub(crate) fn completed(
    scope: RequestScope,
    started: std::time::Instant,
    provider_request_id: Option<String>,
    usage: Option<&crate::provider::types::ProviderUsage>,
) -> RequestCompleted {
    RequestCompleted {
        scope,
        usage: normalized_usage(usage),
        elapsed_ms: Some(elapsed_ms(started)),
        provider_request_id,
        cost: None,
    }
}

pub(crate) fn failed(
    scope: RequestScope,
    started: std::time::Instant,
    provider_request_id: Option<String>,
    usage: Option<&crate::provider::types::ProviderUsage>,
    kind: crate::provider::types::ProviderFailureKind,
    summary: String,
) -> RequestFailed {
    RequestFailed {
        scope,
        usage: usage.map(|usage| normalized_usage(Some(usage))),
        elapsed_ms: Some(elapsed_ms(started)),
        provider_request_id,
        cost: None,
        failure: ProviderFailureFact {
            kind: failure_kind(kind),
            summary,
        },
    }
}

pub(crate) fn provider_fact(kind: &crate::provider::types::ProviderKind) -> ProviderFact {
    match kind {
        crate::provider::types::ProviderKind::Anthropic => ProviderFact::Anthropic,
        crate::provider::types::ProviderKind::OpenAiCompatible => ProviderFact::OpenAiCompatible,
        crate::provider::types::ProviderKind::OpenAiResponses => ProviderFact::OpenAiResponses,
    }
}

fn normalized_usage(usage: Option<&crate::provider::types::ProviderUsage>) -> ProviderUsageFact {
    ProviderUsageFact {
        input_tokens: usage.and_then(|value| json_safe(value.input_tokens)),
        output_tokens: usage.and_then(|value| json_safe(value.output_tokens)),
        cached_input_tokens: usage.and_then(|value| json_safe(value.cache_read_input_tokens)),
        cache_creation_input_tokens: usage
            .and_then(|value| json_safe(value.cache_creation_input_tokens)),
    }
}

fn json_safe(value: Option<u64>) -> Option<u64> {
    value.filter(|value| *value <= JSON_SAFE_INTEGER_MAX)
}

fn elapsed_ms(started: std::time::Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis())
        .unwrap_or(JSON_SAFE_INTEGER_MAX)
        .min(JSON_SAFE_INTEGER_MAX)
}

fn failure_kind(kind: crate::provider::types::ProviderFailureKind) -> ProviderFailureKindFact {
    match kind {
        crate::provider::types::ProviderFailureKind::Authentication => {
            ProviderFailureKindFact::Authentication
        }
        crate::provider::types::ProviderFailureKind::RateLimited => {
            ProviderFailureKindFact::RateLimited
        }
        crate::provider::types::ProviderFailureKind::ContextTooLarge => {
            ProviderFailureKindFact::ContextTooLarge
        }
        crate::provider::types::ProviderFailureKind::InvalidRequest => {
            ProviderFailureKindFact::InvalidRequest
        }
        crate::provider::types::ProviderFailureKind::ProviderUnavailable => {
            ProviderFailureKindFact::ProviderUnavailable
        }
        crate::provider::types::ProviderFailureKind::Transport => {
            ProviderFailureKindFact::Transport
        }
        crate::provider::types::ProviderFailureKind::Internal => ProviderFailureKindFact::Internal,
        crate::provider::types::ProviderFailureKind::Unknown => ProviderFailureKindFact::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::*;
    use crate::event::{RequestCause, RequestStarted};

    #[derive(Debug)]
    struct FailingRecorder;

    impl RequestEvidenceRecorder for FailingRecorder {
        fn record_before_provider(&self, _started: RequestStarted) -> Result<()> {
            Err(Error::InvalidEventStream(
                "injected evidence failure".to_string(),
            ))
        }

        fn record_completed(&self, _completed: RequestCompleted) -> Result<()> {
            unreachable!()
        }

        fn record_failed(&self, _failed: RequestFailed) -> Result<()> {
            unreachable!()
        }
    }

    #[test]
    fn lifecycle_recorder_notifies_durable_event_observers() {
        let directory = tempfile::tempdir().unwrap();
        let events_path = directory.path().join("events.jsonl");
        let observed = Arc::new(AtomicUsize::new(0));
        let store = EventStore::open(&events_path).unwrap();
        store.register_observer({
            let observed = Arc::clone(&observed);
            Arc::new(move |_| {
                observed.fetch_add(1, Ordering::SeqCst);
            })
        });
        let recorder = LifecycleOnlyRecorder::new(&events_path);

        recorder
            .record_before_provider(RequestStarted {
                scope: crate::event::test_request_scope("durable observer"),
                cause: RequestCause::UserSubmission,
                provider: ProviderFact::Anthropic,
                model: "test-model".to_string(),
                started_at: "2026-07-20T00:00:00Z".to_string(),
            })
            .unwrap();

        assert_eq!(observed.load(Ordering::SeqCst), 1);
        assert_eq!(EventStore::replay(events_path).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn evidence_append_failure_prevents_provider_transport() {
        let transport_requests = AtomicUsize::new(0);
        let started = RequestStarted {
            scope: crate::event::test_request_scope("append failure"),
            cause: RequestCause::UserSubmission,
            provider: ProviderFact::Anthropic,
            model: "test-model".to_string(),
            started_at: "2026-07-20T00:00:00Z".to_string(),
        };

        let result = begin_provider_request(&FailingRecorder, started, async {
            transport_requests.fetch_add(1, Ordering::SeqCst);
        })
        .await;

        assert!(result.is_err());
        assert_eq!(transport_requests.load(Ordering::SeqCst), 0);
    }
}
