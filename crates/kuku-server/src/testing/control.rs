use std::fmt;
use std::sync::Arc;

use tokio::sync::{Mutex, Notify};

/// The result observed when a scenario barrier is triggered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BarrierOutcome {
    /// Continue the active deterministic run.
    Released,
    /// Fail the active deterministic run with this fixture-controlled reason.
    Failed(String),
}

/// Errors returned by the authenticated scenario control surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlError {
    /// The requested barrier is not present in the active fixture.
    UnknownBarrier(String),
    /// A barrier has already been released or failed.
    AlreadyTriggered(String),
}

impl fmt::Display for ControlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownBarrier(name) => write!(formatter, "unknown scenario barrier: {name}"),
            Self::AlreadyTriggered(name) => {
                write!(formatter, "scenario barrier already triggered: {name}")
            }
        }
    }
}

impl std::error::Error for ControlError {}

#[derive(Debug, Default)]
struct ControlState {
    barriers: std::collections::BTreeMap<String, Option<BarrierOutcome>>,
}

/// Authenticated test-only controls for deterministic scenario barriers.
#[derive(Clone, Debug)]
pub struct ScenarioControl {
    state: Arc<Mutex<ControlState>>,
    notify: Arc<Notify>,
}

impl ScenarioControl {
    /// Creates controls for the named barriers in an active fixture.
    pub fn new<I, S>(barriers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let barriers = barriers
            .into_iter()
            .map(|name| (name.into(), None))
            .collect();
        Self {
            state: Arc::new(Mutex::new(ControlState { barriers })),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Releases a named barrier exactly once.
    pub async fn release(&self, name: &str) -> Result<(), ControlError> {
        self.trigger(name, BarrierOutcome::Released).await
    }

    /// Records a named fixture-controlled failure exactly once.
    pub async fn fail(&self, name: &str, reason: impl Into<String>) -> Result<(), ControlError> {
        self.trigger(name, BarrierOutcome::Failed(reason.into()))
            .await
    }

    /// Waits until a named barrier is released or failed.
    pub async fn wait(&self, name: &str) -> Result<BarrierOutcome, ControlError> {
        loop {
            let notified = self.notify.notified();
            {
                let state = self.state.lock().await;
                let Some(outcome) = state.barriers.get(name) else {
                    return Err(ControlError::UnknownBarrier(name.to_owned()));
                };
                if let Some(outcome) = outcome {
                    return Ok(outcome.clone());
                }
            }
            notified.await;
        }
    }

    async fn trigger(&self, name: &str, outcome: BarrierOutcome) -> Result<(), ControlError> {
        let mut state = self.state.lock().await;
        let Some(slot) = state.barriers.get_mut(name) else {
            return Err(ControlError::UnknownBarrier(name.to_owned()));
        };
        if slot.is_some() {
            return Err(ControlError::AlreadyTriggered(name.to_owned()));
        }
        *slot = Some(outcome);
        drop(state);
        self.notify.notify_waiters();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn barrier_release_wakes_waiter_once() {
        let control = ScenarioControl::new(["after-tool"]);
        let waiter = {
            let control = control.clone();
            tokio::spawn(async move { control.wait("after-tool").await })
        };
        control.release("after-tool").await.unwrap();
        assert_eq!(waiter.await.unwrap().unwrap(), BarrierOutcome::Released);
        assert_eq!(
            control.release("after-tool").await,
            Err(ControlError::AlreadyTriggered("after-tool".to_owned()))
        );
    }

    #[tokio::test]
    async fn unknown_barrier_is_rejected() {
        let control = ScenarioControl::new(["known"]);
        assert_eq!(
            control.release("missing").await,
            Err(ControlError::UnknownBarrier("missing".to_owned()))
        );
    }
}
