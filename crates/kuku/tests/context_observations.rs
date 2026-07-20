pub mod event {
    pub use kuku::event::*;
}

#[allow(dead_code)]
#[path = "../src/context/observations.rs"]
mod observations;

use kuku::event::{
    ExecutionScope, ObservationKind, ObservationRetention, RequestId, RequestScope,
    WorkspaceRelativePath,
};
use observations::{
    ObservationBuilder, ObservationState, ObservationTracker, ToolObservation, ToolObservationData,
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

fn file_read(truncated: bool, summarized: bool) -> ToolObservation {
    ToolObservation {
        summary: "read src/lib.rs".to_owned(),
        truncated,
        summarized,
        data: ToolObservationData::FileRead {
            path: "src/lib.rs".to_owned(),
            observed_hash: Some("sha256:old".to_owned()),
            start_line: 3,
            line_count: 4,
        },
    }
}

#[test]
fn changed_deleted_and_truncated_are_distinct() {
    let fact = ObservationBuilder::from_tool(
        scope("req_000000000000000000000000"),
        "call-1",
        file_read(false, false),
    )
    .unwrap();
    let tracker = ObservationTracker::new(&fact);

    assert_eq!(
        ObservationState::ChangedSinceObservation,
        tracker.compare(Some("sha256:new"))
    );
    assert_eq!(ObservationState::NoLongerPresent, tracker.compare(None));
    assert_eq!(ObservationRetention::Retained, fact.retention);

    let truncated = ObservationBuilder::from_tool(
        scope("req_000000000000000000000000"),
        "call-2",
        file_read(true, false),
    )
    .unwrap();
    assert_eq!(ObservationRetention::Truncated, truncated.retention);
}

#[test]
fn summary_retention_is_recorded_without_retaining_full_content() {
    let fact = ObservationBuilder::from_tool(
        scope("req_000000000000000000000000"),
        "call-search",
        ToolObservation {
            summary: "two matches".to_owned(),
            truncated: false,
            summarized: true,
            data: ToolObservationData::Search {
                query: "ObservationFact".to_owned(),
                path: Some("src".to_owned()),
            },
        },
    )
    .unwrap();

    assert_eq!(ObservationRetention::Summarized, fact.retention);
    assert_eq!(
        ObservationKind::Search {
            query: "ObservationFact".to_owned()
        },
        fact.kind
    );
    assert_eq!(
        Some(WorkspaceRelativePath::parse("src").unwrap()),
        fact.relative_path
    );
}

#[test]
fn list_and_command_observations_do_not_invent_file_hashes() {
    let list = ObservationBuilder::from_tool(
        scope("req_000000000000000000000000"),
        "call-list",
        ToolObservation {
            summary: "listed workspace".to_owned(),
            truncated: false,
            summarized: false,
            data: ToolObservationData::FileList { path: None },
        },
    )
    .unwrap();
    assert_eq!(ObservationKind::FileList, list.kind);
    assert_eq!(None, list.relative_path);
    assert_eq!(None, list.observed_hash);

    let command = ObservationBuilder::from_tool(
        scope("req_000000000000000000000000"),
        "call-command",
        ToolObservation {
            summary: "cargo check completed".to_owned(),
            truncated: false,
            summarized: false,
            data: ToolObservationData::Command {
                command: "cargo check".to_owned(),
                exit_code: Some(0),
            },
        },
    )
    .unwrap();
    assert!(matches!(command.kind, ObservationKind::Command { .. }));
    assert_eq!(None, command.relative_path);
    assert_eq!(None, command.observed_hash);
}

#[test]
fn invalid_paths_and_empty_summaries_are_rejected() {
    let invalid_path = ObservationBuilder::from_tool(
        scope("req_000000000000000000000000"),
        "call-invalid",
        ToolObservation {
            summary: "bad".to_owned(),
            truncated: false,
            summarized: false,
            data: ToolObservationData::FileRead {
                path: "../secret".to_owned(),
                observed_hash: Some("sha256:old".to_owned()),
                start_line: 1,
                line_count: 1,
            },
        },
    );
    assert!(invalid_path.is_err());

    let empty_summary = ObservationBuilder::from_tool(
        scope("req_000000000000000000000000"),
        "call-empty",
        ToolObservation {
            summary: String::new(),
            truncated: false,
            summarized: false,
            data: ToolObservationData::FileList { path: None },
        },
    );
    assert!(empty_summary.is_err());
}

#[test]
fn current_hash_is_a_derived_overlay_only() {
    let fact = ObservationBuilder::from_tool(
        scope("req_000000000000000000000000"),
        "call-1",
        file_read(false, false),
    )
    .unwrap();
    let original = fact.clone();
    let tracker = ObservationTracker::new(&fact);
    assert_eq!(
        ObservationState::Present,
        tracker.compare(Some("sha256:old"))
    );
    assert_eq!(original, fact);
    let _request_id: RequestId = fact.scope.request_id;
    let _execution: ExecutionScope = fact.scope.execution;
}
