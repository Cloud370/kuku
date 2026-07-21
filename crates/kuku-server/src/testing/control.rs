use std::fmt;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
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

/// Builds the feature-only scenario control routes under `/api/v1`.
pub fn router(control: ScenarioControl) -> Router {
    Router::new()
        .route("/testing/barriers/{name}/release", post(release))
        .route("/testing/failures/{name}", post(fail))
        .with_state(control)
}

async fn release(
    State(control): State<ScenarioControl>,
    Path(name): Path<String>,
) -> Result<StatusCode, ControlHttpError> {
    control.release(&name).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FailureRequest {
    reason: String,
}

async fn fail(
    State(control): State<ScenarioControl>,
    Path(name): Path<String>,
    Json(request): Json<FailureRequest>,
) -> Result<StatusCode, ControlHttpError> {
    if request.reason.trim().is_empty() {
        return Err(ControlHttpError::InvalidReason);
    }
    control.fail(&name, request.reason).await?;
    Ok(StatusCode::NO_CONTENT)
}

enum ControlHttpError {
    Control(ControlError),
    InvalidReason,
}

impl From<ControlError> for ControlHttpError {
    fn from(error: ControlError) -> Self {
        Self::Control(error)
    }
}

impl IntoResponse for ControlHttpError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Control(ControlError::UnknownBarrier(name)) => (
                StatusCode::NOT_FOUND,
                crate::api::ApiErrorCode::RequestNotFound,
                format!("unknown scenario barrier: {name}"),
            ),
            Self::Control(ControlError::AlreadyTriggered(name)) => (
                StatusCode::CONFLICT,
                crate::api::ApiErrorCode::IdempotencyConflict,
                format!("scenario barrier already triggered: {name}"),
            ),
            Self::InvalidReason => (
                StatusCode::BAD_REQUEST,
                crate::api::ApiErrorCode::InvalidRequest,
                "scenario failure reason is required".to_owned(),
            ),
        };
        (
            status,
            Json(crate::api::ApiError::new(code, message, "scenario-control")),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

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

    #[tokio::test]
    async fn control_router_maps_declared_unknown_and_repeated_barriers() {
        let app = router(ScenarioControl::new([
            "after-tool",
            "continuity-before-finish",
        ]));
        let release = Request::post("/testing/barriers/after-tool/release")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(release).await.unwrap().status(),
            StatusCode::NO_CONTENT
        );
        let repeated = Request::post("/testing/barriers/after-tool/release")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(repeated).await.unwrap().status(),
            StatusCode::CONFLICT
        );
        let missing = Request::post("/testing/barriers/missing/release")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(missing).await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
        let failure = Request::post("/testing/failures/continuity-before-finish")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"reason":"fixture failure"}"#))
            .unwrap();
        assert_eq!(
            app.oneshot(failure).await.unwrap().status(),
            StatusCode::NO_CONTENT
        );
    }
}
