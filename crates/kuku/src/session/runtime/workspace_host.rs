use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::{Error, Result};

/// Shared capacity for disposable JavaScript workers on a single execution host.
#[derive(Debug, Clone)]
pub struct WorkspaceExecutionHost(Arc<Capacity>);

#[derive(Debug)]
struct Capacity {
    workers: Arc<Semaphore>,
    admission: Arc<Semaphore>,
    operations: Arc<Semaphore>,
    active: AtomicUsize,
    peak: AtomicUsize,
    starts: AtomicUsize,
}

/// A point-in-time observation of worker usage since this host was created.
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct WorkspaceExecutionMetrics {
    /// Workers that currently hold an execution permit.
    pub active_workers: usize,
    /// Highest simultaneous worker count observed.
    pub peak_workers: usize,
    /// Cumulative worker admissions.
    pub started_workers: usize,
}

impl WorkspaceExecutionHost {
    /// Create an independent capacity pool. Clone it into Sessions on the same host.
    pub fn new(max_workers: usize, max_host_operations: usize) -> Result<Self> {
        if !(1..=256).contains(&max_workers) || !(1..=4096).contains(&max_host_operations) {
            return Err(Error::InvalidArgument(
                "workspace host requires 1..=256 workers and 1..=4096 concurrent host operations"
                    .into(),
            ));
        }
        Ok(Self(Arc::new(Capacity {
            workers: Arc::new(Semaphore::new(max_workers)),
            admission: Arc::new(Semaphore::new(max_workers * 8)),
            operations: Arc::new(Semaphore::new(max_host_operations)),
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            starts: AtomicUsize::new(0),
        })))
    }

    /// Sample resource usage without starting a worker.
    pub fn metrics(&self) -> WorkspaceExecutionMetrics {
        WorkspaceExecutionMetrics {
            active_workers: self.0.active.load(Ordering::Acquire),
            peak_workers: self.0.peak.load(Ordering::Acquire),
            started_workers: self.0.starts.load(Ordering::Acquire),
        }
    }

    pub(crate) fn operations(&self) -> Arc<Semaphore> {
        self.0.operations.clone()
    }

    pub(crate) async fn acquire_worker(&self) -> Result<WorkspaceWorkerPermit> {
        let _admitted = self
            .0
            .admission
            .try_acquire()
            .map_err(|_| Error::InvalidArgument("workspace worker queue is full".into()))?;
        let worker = self
            .0
            .workers
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::InvalidArgument("workspace execution host is closed".into()))?;
        let count = self.0.active.fetch_add(1, Ordering::AcqRel) + 1;
        self.0.peak.fetch_max(count, Ordering::AcqRel);
        self.0.starts.fetch_add(1, Ordering::AcqRel);
        Ok(WorkspaceWorkerPermit {
            capacity: self.0.clone(),
            _worker: worker,
        })
    }
}

impl Default for WorkspaceExecutionHost {
    fn default() -> Self {
        static PROCESS: OnceLock<WorkspaceExecutionHost> = OnceLock::new();
        PROCESS
            .get_or_init(|| Self::new(2, 8).expect("fixed capacity is valid"))
            .clone()
    }
}

pub(crate) struct WorkspaceWorkerPermit {
    capacity: Arc<Capacity>,
    _worker: OwnedSemaphorePermit,
}

impl Drop for WorkspaceWorkerPermit {
    fn drop(&mut self) {
        self.capacity.active.fetch_sub(1, Ordering::AcqRel);
    }
}
