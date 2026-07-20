pub mod event {
    pub use kuku::event::*;
}

#[allow(dead_code)]
#[path = "../../kuku/src/context/observations.rs"]
mod sdk_observations;

use crate::sdk_observations as observation_contract;

#[path = "../src/context/observation_reducer.rs"]
mod observation_reducer;

use std::collections::BTreeMap;

use kuku::event::{RequestScope, WorkspaceRelativePath};
use observation_contract::ObservationState;
use observation_contract::{ObservationBuilder, ToolObservation, ToolObservationData};
use observation_reducer::{
    CurrentObservationState, ObservationHashProvider, ObservationProjection, ObservationReducer,
};

fn scope(request_id: &str) -> RequestScope {
    serde_json::from_value(serde_json::json!({
        "execution": {
            "workspace_id": "wsp_000000000000000000000000",
            "task_id": "tsk_000000000000000000000000",
            "run_id": "run_000000000000000000000000",
            "turn_id": "trn_000000000000000000000000",
            "conversation_id": "con_000000000000000000000000",
            "turn_index": 0
        },
        "request_id": request_id
    }))
    .unwrap()
}

fn read_fact(request_id: &str, path: &str, hash: &str) -> kuku::event::ObservationFact {
    ObservationBuilder::from_tool(
        scope(request_id),
        format!("call-{path}"),
        ToolObservation {
            summary: format!("read {path}"),
            truncated: false,
            summarized: false,
            data: ToolObservationData::FileRead {
                path: path.to_owned(),
                observed_hash: Some(hash.to_owned()),
                start_line: 1,
                line_count: 1,
            },
        },
    )
    .unwrap()
}

#[derive(Default)]
struct Hashes {
    values: BTreeMap<String, CurrentObservationState>,
}

impl ObservationHashProvider for Hashes {
    fn current_state(
        &self,
        _workspace_id: &kuku::event::WorkspaceId,
        path: &WorkspaceRelativePath,
    ) -> CurrentObservationState {
        self.values
            .get(path.as_str())
            .cloned()
            .unwrap_or(CurrentObservationState::Missing)
    }
}

#[test]
fn changed_deleted_and_inaccessible_are_explicit() {
    let facts = vec![
        read_fact(
            "req_000000000000000000000000",
            "src/changed.rs",
            "sha256:old",
        ),
        read_fact(
            "req_000000000000000000000000",
            "src/deleted.rs",
            "sha256:old",
        ),
        read_fact(
            "req_000000000000000000000000",
            "src/secret.rs",
            "sha256:old",
        ),
    ];
    let hashes = Hashes {
        values: BTreeMap::from([
            (
                "src/changed.rs".to_owned(),
                CurrentObservationState::Present("sha256:new".to_owned()),
            ),
            (
                "src/deleted.rs".to_owned(),
                CurrentObservationState::Missing,
            ),
            (
                "src/secret.rs".to_owned(),
                CurrentObservationState::Inaccessible,
            ),
        ]),
    };

    let projected = ObservationReducer::for_request(
        &scope("req_000000000000000000000000"),
        &scope("req_000000000000000000000000").execution.workspace_id,
        &facts,
        &hashes,
    );

    assert_eq!(3, projected.len());
    assert_eq!(
        ObservationState::ChangedSinceObservation,
        projected[0].current_drift
    );
    assert_eq!(
        ObservationState::NoLongerPresent,
        projected[1].current_drift
    );
    assert_eq!(ObservationState::Inaccessible, projected[2].current_drift);
    assert_eq!(
        "sha256:old",
        projected[0].fact.observed_hash.as_deref().unwrap()
    );
}

#[test]
fn projection_filters_request_and_keeps_command_without_path() {
    let selected = read_fact("req_000000000000000000000000", "src/lib.rs", "sha256:old");
    let other = read_fact("req_000000000000000000000001", "src/lib.rs", "sha256:old");
    let command = ObservationBuilder::from_tool(
        scope("req_000000000000000000000000"),
        "call-command",
        ToolObservation {
            summary: "command output".to_owned(),
            truncated: false,
            summarized: false,
            data: ToolObservationData::Command {
                command: "cargo test".to_owned(),
                exit_code: Some(0),
            },
        },
    )
    .unwrap();
    let hashes = Hashes {
        values: BTreeMap::from([(
            "src/lib.rs".to_owned(),
            CurrentObservationState::Present("sha256:old".to_owned()),
        )]),
    };

    let projected = ObservationReducer::for_request(
        &scope("req_000000000000000000000000"),
        &scope("req_000000000000000000000000").execution.workspace_id,
        &[selected, other, command],
        &hashes,
    );

    assert_eq!(2, projected.len());
    assert_eq!(ObservationState::Present, projected[0].current_drift);
    assert_eq!(ObservationState::NotApplicable, projected[1].current_drift);
    assert!(matches!(projected[1], ObservationProjection { .. }));
}
