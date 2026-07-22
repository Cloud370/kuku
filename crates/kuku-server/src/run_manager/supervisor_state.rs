use std::future::Future;
use std::sync::Mutex;
use std::time::Duration;

use super::DomainError;

const RETRY_DELAYS_MS: [u64; 3] = [100, 200, 400];

enum RuntimeHealth {
    Healthy,
    Fatal(DomainError),
}

pub(super) struct PersistenceState {
    health: Mutex<RuntimeHealth>,
    lifecycle: Mutex<()>,
}

impl Default for PersistenceState {
    fn default() -> Self {
        Self {
            health: Mutex::new(RuntimeHealth::Healthy),
            lifecycle: Mutex::new(()),
        }
    }
}

impl PersistenceState {
    pub(super) fn admit<T>(&self, operation: impl FnOnce() -> T) -> Result<T, DomainError> {
        let _lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.ensure_healthy()?;
        Ok(operation())
    }

    pub(super) fn cleanup<T>(&self, operation: impl FnOnce() -> T) -> T {
        let _lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        operation()
    }

    pub(super) fn ensure_healthy(&self) -> Result<(), DomainError> {
        match &*self
            .health
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            RuntimeHealth::Healthy => Ok(()),
            RuntimeHealth::Fatal(error) => Err(error.clone()),
        }
    }

    pub(super) async fn persist<T, Operation, OperationFuture>(
        &self,
        mut operation: Operation,
    ) -> Result<T, DomainError>
    where
        Operation: FnMut() -> OperationFuture,
        OperationFuture: Future<Output = Result<T, DomainError>>,
    {
        let mut retry = 0;
        loop {
            self.ensure_healthy()?;
            match operation().await {
                Ok(value) => return Ok(value),
                Err(error) => match retry_delay(&error, retry) {
                    Some(delay) => {
                        retry += 1;
                        tokio::time::sleep(delay).await;
                    }
                    None => return Err(self.record_fatal(error)),
                },
            }
        }
    }

    fn record_fatal(&self, error: DomainError) -> DomainError {
        let mut health = self
            .health
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let RuntimeHealth::Fatal(existing) = &*health {
            return existing.clone();
        }
        *health = RuntimeHealth::Fatal(error.clone());
        error
    }
}

fn retry_delay(error: &DomainError, retry: usize) -> Option<Duration> {
    if !matches!(
        error,
        DomainError::LedgerCorrupt | DomainError::StorageExhausted
    ) {
        return None;
    }
    RETRY_DELAYS_MS
        .get(retry)
        .copied()
        .map(Duration::from_millis)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc, Barrier};

    use super::*;

    #[test]
    fn fatal_cleanup_waits_for_an_in_flight_admit_then_observes_it() {
        let state = Arc::new(PersistenceState::default());
        let admitted = Arc::new(AtomicBool::new(false));
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let admit = {
            let state = state.clone();
            let admitted = admitted.clone();
            let entered = entered.clone();
            let release = release.clone();
            std::thread::spawn(move || {
                state
                    .admit(|| {
                        entered.wait();
                        release.wait();
                        admitted.store(true, Ordering::Release);
                    })
                    .unwrap();
            })
        };
        entered.wait();
        state.record_fatal(DomainError::LedgerCorrupt);
        let (observed_tx, observed_rx) = mpsc::channel();
        let cleanup = {
            let state = state.clone();
            let admitted = admitted.clone();
            std::thread::spawn(move || {
                state.cleanup(|| {
                    observed_tx.send(admitted.load(Ordering::Acquire)).unwrap();
                });
            })
        };
        assert!(observed_rx.recv_timeout(Duration::from_millis(50)).is_err());
        release.wait();
        admit.join().unwrap();
        cleanup.join().unwrap();
        assert!(observed_rx.recv().unwrap());
    }
}
