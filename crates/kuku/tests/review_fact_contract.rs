use std::collections::BTreeSet;

use kuku::event::{
    AnnotationSide, ReviewAnnotationFact, ReviewSubmissionId, ReviewSubmissionRecorded,
    RevisionToken, RunId, TaskId, TaskRevision,
};

fn revision(character: char) -> RevisionToken {
    RevisionToken::parse(character.to_string().repeat(64)).unwrap()
}

fn annotation(path: &str, revision: RevisionToken, side: AnnotationSide) -> ReviewAnnotationFact {
    ReviewAnnotationFact {
        path: path.to_string(),
        revision,
        side,
        start_line: 4,
        end_line: 6,
        excerpt: "first\nsecond\nthird".to_string(),
        comment: format!("Review {path}"),
    }
}

fn submission() -> ReviewSubmissionRecorded {
    ReviewSubmissionRecorded {
        submission_id: ReviewSubmissionId::parse("rsub_0123456789abcdef01234567").unwrap(),
        task_id: TaskId::parse("tsk_0123456789abcdef01234567").unwrap(),
        run_id: RunId::parse("run_0123456789abcdef01234567").unwrap(),
        task_revision: TaskRevision::try_new(7).unwrap(),
        submitted_at: "2026-07-18T12:00:00Z".to_string(),
        notes: vec![
            annotation("src/current.rs", revision('a'), AnnotationSide::File),
            annotation("src/old.rs", revision('b'), AnnotationSide::Old),
            annotation("src/new.rs", revision('c'), AnnotationSide::New),
        ],
    }
}

#[test]
fn annotation_sides_round_trip_as_frozen_snake_case_values() {
    for (side, expected) in [
        (AnnotationSide::File, "file"),
        (AnnotationSide::Old, "old"),
        (AnnotationSide::New, "new"),
    ] {
        let json = serde_json::to_value(side).unwrap();
        assert_eq!(serde_json::json!(expected), json);
        assert_eq!(side, serde_json::from_value(json).unwrap());
    }

    let schema = serde_json::to_value(schemars::schema_for!(AnnotationSide)).unwrap();
    assert_eq!(serde_json::json!(["file", "old", "new"]), schema["enum"]);
}

#[test]
fn submission_round_trip_preserves_ordered_original_annotations() {
    let submission = submission();
    let json = serde_json::to_value(&submission).unwrap();

    assert_eq!(
        serde_json::json!({
            "submission_id": "rsub_0123456789abcdef01234567",
            "task_id": "tsk_0123456789abcdef01234567",
            "run_id": "run_0123456789abcdef01234567",
            "task_revision": 7,
            "submitted_at": "2026-07-18T12:00:00Z",
            "notes": [
                {
                    "path": "src/current.rs",
                    "revision": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "side": "file",
                    "start_line": 4,
                    "end_line": 6,
                    "excerpt": "first\nsecond\nthird",
                    "comment": "Review src/current.rs"
                },
                {
                    "path": "src/old.rs",
                    "revision": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    "side": "old",
                    "start_line": 4,
                    "end_line": 6,
                    "excerpt": "first\nsecond\nthird",
                    "comment": "Review src/old.rs"
                },
                {
                    "path": "src/new.rs",
                    "revision": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                    "side": "new",
                    "start_line": 4,
                    "end_line": 6,
                    "excerpt": "first\nsecond\nthird",
                    "comment": "Review src/new.rs"
                }
            ]
        }),
        json
    );
    assert_eq!(
        submission,
        serde_json::from_value::<ReviewSubmissionRecorded>(json).unwrap()
    );
}

#[test]
fn durable_review_fact_schema_has_only_required_non_nullable_fields() {
    let schema = serde_json::to_value(schemars::schema_for!(ReviewSubmissionRecorded)).unwrap();
    let required = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect::<BTreeSet<_>>();
    let expected = [
        "submission_id",
        "task_id",
        "run_id",
        "task_revision",
        "submitted_at",
        "notes",
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();

    assert_eq!(expected, required);
    assert!(!schema.to_string().contains("\"null\""));

    let annotation_schema =
        serde_json::to_value(schemars::schema_for!(ReviewAnnotationFact)).unwrap();
    let annotation_required = annotation_schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect::<BTreeSet<_>>();
    let annotation_expected = [
        "path",
        "revision",
        "side",
        "start_line",
        "end_line",
        "excerpt",
        "comment",
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();

    assert_eq!(annotation_expected, annotation_required);
    assert!(!annotation_schema.to_string().contains("\"null\""));
}

#[test]
fn review_facts_reject_non_lowercase_revision_tokens() {
    let mut json = serde_json::to_value(submission()).unwrap();
    json["notes"][0]["revision"] = serde_json::json!("A".repeat(64));
    assert!(serde_json::from_value::<ReviewSubmissionRecorded>(json).is_err());

    let mut prefixed = serde_json::to_value(submission()).unwrap();
    prefixed["notes"][0]["revision"] = serde_json::json!(format!("sha256:{}", "a".repeat(64)));
    assert!(serde_json::from_value::<ReviewSubmissionRecorded>(prefixed).is_err());
}

#[test]
fn review_fact_module_keeps_the_sdk_dependency_boundary() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = std::fs::read_to_string(root.join("src/event/types/review.rs")).unwrap();

    for forbidden in [
        "kuku_server",
        "axum",
        "std::fs",
        "tokio::fs",
        "std::process",
        "tokio::process",
        "crate::api",
        "PathBuf",
    ] {
        assert!(
            !source.contains(forbidden),
            "review facts cross dependency boundary through {forbidden}"
        );
    }
    assert!(source.lines().count() < 1000);
}
