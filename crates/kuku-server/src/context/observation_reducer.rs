use kuku::event::{ObservationFact, RequestScope, WorkspaceId, WorkspaceRelativePath};

use super::observation_contract::{ObservationState, ObservationTracker};

/// Current host-side state returned for one contained workspace path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CurrentObservationState {
    Present(String),
    Missing,
    Inaccessible,
}

/// Provides contained current state for observed workspace paths.
pub(crate) trait ObservationHashProvider: Send + Sync {
    fn current_state(
        &self,
        workspace_id: &WorkspaceId,
        path: &WorkspaceRelativePath,
    ) -> CurrentObservationState;
}

/// One immutable fact plus a derived current-drift overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ObservationProjection {
    pub(crate) fact: ObservationFact,
    pub(crate) current_drift: ObservationState,
}

pub(crate) struct ObservationReducer;

impl ObservationReducer {
    pub(crate) fn for_request(
        scope: &RequestScope,
        workspace_id: &WorkspaceId,
        facts: &[ObservationFact],
        hashes: &dyn ObservationHashProvider,
    ) -> Vec<ObservationProjection> {
        Self::for_requests(std::slice::from_ref(scope), workspace_id, facts, hashes)
    }

    pub(crate) fn for_requests(
        scopes: &[RequestScope],
        workspace_id: &WorkspaceId,
        facts: &[ObservationFact],
        hashes: &dyn ObservationHashProvider,
    ) -> Vec<ObservationProjection> {
        facts
            .iter()
            .filter(|fact| scopes.contains(&fact.scope))
            .map(|fact| project(fact, workspace_id, hashes))
            .collect()
    }
}

fn project(
    fact: &ObservationFact,
    workspace_id: &WorkspaceId,
    hashes: &dyn ObservationHashProvider,
) -> ObservationProjection {
    let current_drift = match fact.relative_path.as_ref() {
        None => ObservationState::NotApplicable,
        Some(path) => match hashes.current_state(workspace_id, path) {
            CurrentObservationState::Present(current_hash) => {
                ObservationTracker::new(fact).compare(Some(current_hash.as_str()))
            }
            CurrentObservationState::Missing => ObservationState::NoLongerPresent,
            CurrentObservationState::Inaccessible => ObservationState::Inaccessible,
        },
    };
    ObservationProjection {
        fact: fact.clone(),
        current_drift,
    }
}
