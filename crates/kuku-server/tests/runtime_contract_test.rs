use std::collections::BTreeSet;

use kuku_server::api::{
    contract_export, ApiError, SubmitRunResponse, TaskChange, TaskDelta, TaskPage, TaskProjection,
    TaskStreamEvent,
};
use serde_json::Value;

const FORBIDDEN_WIRE_VOCABULARY: [&str; 4] = [
    "session_id",
    "conversation_address",
    "ui_event",
    "provider_chunk",
];

fn fixture_projection() -> TaskProjection {
    serde_json::from_str(include_str!("fixtures/runtime/task_draft.json")).unwrap()
}

#[test]
fn runtime_wire_matches_golden_and_never_leaks_sdk_names() {
    let json = serde_json::to_string_pretty(&fixture_projection()).unwrap();
    assert_eq!(
        format!("{json}\n"),
        include_str!("fixtures/runtime/task_draft.json")
    );
    for forbidden in FORBIDDEN_WIRE_VOCABULARY {
        assert!(!json.contains(forbidden), "wire leaked {forbidden}");
    }
}

#[test]
fn runtime_response_and_error_fixtures_are_byte_stable() {
    round_trip::<TaskPage>("task_page.json");
    round_trip::<SubmitRunResponse>("run_accepted.json");
    round_trip::<ApiError>("task_busy.json");
    round_trip::<ApiError>("stale_command.json");
}

#[test]
fn subscription_fixture_is_strict_ndjson_with_atomic_changes() {
    let source = include_str!("fixtures/runtime/subscription.ndjson");
    assert!(source.ends_with('\n'));
    assert!(!source.ends_with("\n\n"));

    let frames = source
        .lines()
        .map(|line| {
            let value: Value = serde_json::from_str(line).unwrap();
            let keys = value
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<BTreeSet<_>>();
            assert_eq!(
                keys,
                ["api_version", "cursor", "event", "task_id", "task_revision"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            );
            for forbidden in FORBIDDEN_WIRE_VOCABULARY {
                assert!(!line.contains(forbidden), "wire leaked {forbidden}");
            }
            serde_json::from_value::<TaskStreamEvent>(value).unwrap()
        })
        .collect::<Vec<_>>();

    assert!(matches!(
        frames.first().map(|frame| &frame.event),
        Some(TaskDelta::ProjectionReplaced { .. })
    ));
    assert!(frames
        .windows(2)
        .all(|pair| pair[0].cursor < pair[1].cursor));

    let mut kinds = Vec::new();
    let mut has_atomic_control_frame = false;
    for frame in &frames[1..] {
        let TaskDelta::ChangesApplied {
            changes,
            timeline_window,
        } = &frame.event
        else {
            panic!("only the first frame may replace the projection");
        };
        if contains_atomic_submission(changes) {
            has_atomic_control_frame = true;
        }
        for change in changes {
            kinds.push(change_kind(change));
        }
        if changes.iter().any(changes_timeline) {
            assert!(timeline_window.is_some());
        } else {
            assert!(timeline_window.is_none());
        }
    }
    assert!(has_atomic_control_frame);
    assert_eq!(
        kinds.into_iter().collect::<BTreeSet<_>>(),
        [
            "message_appended",
            "message_patched",
            "activity_upserted",
            "interaction_upserted",
            "run_state_changed",
            "skills_changed",
            "context_summary_changed",
            "review_submissions_changed",
        ]
        .into_iter()
        .collect()
    );
}

#[test]
fn runtime_dtos_remain_in_the_single_integration_schema() {
    let schema: Value = serde_json::from_str(&contract_export::render_schema().unwrap()).unwrap();
    let definitions = schema
        .get("$defs")
        .or_else(|| schema.get("definitions"))
        .and_then(Value::as_object)
        .unwrap();
    for definition in [
        "TaskPage",
        "TaskProjection",
        "TaskStreamEvent",
        "SubmitRunRequest",
    ] {
        assert!(definitions.contains_key(definition), "missing {definition}");
    }
}

fn round_trip<T>(name: &str)
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let path = format!(
        "{}/tests/fixtures/runtime/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    let bytes = std::fs::read_to_string(path).unwrap();
    let decoded = serde_json::from_str::<T>(&bytes).unwrap();
    assert_eq!(
        format!("{}\n", serde_json::to_string_pretty(&decoded).unwrap()),
        bytes
    );
}

fn contains_atomic_submission(changes: &[TaskChange]) -> bool {
    changes
        .iter()
        .any(|change| matches!(change, TaskChange::MessageAppended { .. }))
        && changes
            .iter()
            .any(|change| matches!(change, TaskChange::SkillsChanged { .. }))
        && changes
            .iter()
            .any(|change| matches!(change, TaskChange::RunStateChanged { .. }))
}

fn change_kind(change: &TaskChange) -> &'static str {
    match change {
        TaskChange::MessageAppended { .. } => "message_appended",
        TaskChange::MessagePatched { .. } => "message_patched",
        TaskChange::ActivityUpserted { .. } => "activity_upserted",
        TaskChange::InteractionUpserted { .. } => "interaction_upserted",
        TaskChange::RunStateChanged { .. } => "run_state_changed",
        TaskChange::SkillsChanged { .. } => "skills_changed",
        TaskChange::ContextSummaryChanged { .. } => "context_summary_changed",
        TaskChange::ReviewSubmissionsChanged { .. } => "review_submissions_changed",
    }
}

fn changes_timeline(change: &TaskChange) -> bool {
    matches!(
        change,
        TaskChange::MessageAppended { .. }
            | TaskChange::MessagePatched { .. }
            | TaskChange::ActivityUpserted { .. }
            | TaskChange::InteractionUpserted { .. }
    )
}
