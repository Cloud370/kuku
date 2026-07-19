use std::fs;

use kuku_server::api::{
    contract_export, ApiError, ApiErrorCode, ApiVersion, PageCursor, TaskId, TaskProjection,
    TaskRevision, WebApiContract,
};
use schemars::schema_for;
use serde_json::{json, Value};

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/tests/fixtures/api/v1/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn api_version_is_the_literal_one() {
    assert_eq!(serde_json::to_value(ApiVersion).unwrap(), json!(1));
    assert_eq!(
        serde_json::from_value::<ApiVersion>(json!(1)).unwrap(),
        ApiVersion
    );
    assert!(serde_json::from_value::<ApiVersion>(json!(2)).is_err());

    let schema = serde_json::to_value(schema_for!(ApiVersion)).unwrap();
    assert_eq!(schema["const"], json!(1));
    assert_eq!(schema["type"], json!("integer"));
}

#[test]
fn page_cursor_is_bounded_and_opaque() {
    let cursor = PageCursor::try_new("task:v1:opaque").unwrap();
    assert_eq!(cursor.as_str(), "task:v1:opaque");
    assert_eq!(
        serde_json::to_value(cursor).unwrap(),
        json!("task:v1:opaque")
    );
    assert!(PageCursor::try_new("").is_err());
    assert!(PageCursor::try_new("x".repeat(2_049)).is_err());
    assert!(serde_json::from_value::<PageCursor>(json!(42)).is_err());
}

#[test]
fn api_error_uses_http_independent_typed_code() {
    let error = ApiError::task_busy("trace_fixture_task_busy");
    assert_eq!(error.code, ApiErrorCode::TaskBusy);
    assert_eq!(
        serde_json::to_value(error).unwrap(),
        fixture("api_error.json")
    );
}

#[test]
fn task_projection_fixture_has_stable_ids_state_and_cursor() {
    let projection: TaskProjection =
        serde_json::from_value(fixture("task_projection.json")).unwrap();
    assert_eq!(
        serde_json::to_value(projection).unwrap(),
        fixture("task_projection.json")
    );
}

#[test]
fn optional_contract_keys_are_required_nullable() {
    let mut projection = fixture("task_projection.json");
    assert!(projection["active_run"].is_null());
    projection.as_object_mut().unwrap().remove("active_run");
    assert!(serde_json::from_value::<TaskProjection>(projection).is_err());
}

#[test]
fn exported_nullable_properties_are_always_required() {
    fn inspect(value: &Value) {
        if let Some(object) = value.as_object() {
            if let Some(properties) = object.get("properties").and_then(Value::as_object) {
                let required = object
                    .get("required")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for (name, schema) in properties {
                    let nullable =
                        schema
                            .get("anyOf")
                            .and_then(Value::as_array)
                            .is_some_and(|choices| {
                                choices.iter().any(|choice| choice["type"] == "null")
                            });
                    if nullable {
                        assert!(
                            required.iter().any(|entry| entry == name),
                            "{name} is optional"
                        );
                    }
                }
            }
            for child in object.values() {
                inspect(child);
            }
        } else if let Some(array) = value.as_array() {
            for child in array {
                inspect(child);
            }
        }
    }

    let schema: Value = serde_json::from_str(&contract_export::render_schema().unwrap()).unwrap();
    inspect(&schema);
}

#[test]
fn sole_contract_root_and_export_are_deterministic() {
    let _schema = schema_for!(WebApiContract);
    let rendered_once = contract_export::render_schema().unwrap();
    let rendered_twice = contract_export::render_schema().unwrap();
    assert_eq!(rendered_once, rendered_twice);
    let exported = serde_json::from_str::<Value>(&rendered_once).unwrap();
    assert_eq!(exported["title"], json!("WebApiContract"));
    assert!(exported["$defs"]
        .as_object()
        .unwrap()
        .contains_key("TaskProjection"));
}

#[test]
fn api_identity_types_are_sdk_reexports() {
    fn accepts_sdk_task_id(value: kuku::event::TaskId) -> kuku::event::TaskId {
        value
    }
    fn accepts_sdk_revision(value: kuku::event::TaskRevision) -> kuku::event::TaskRevision {
        value
    }

    let task_id = TaskId::parse("tsk_000000000000000000000001").unwrap();
    let revision = TaskRevision::try_new(3).unwrap();
    assert_eq!(
        accepts_sdk_task_id(task_id).as_str(),
        "tsk_000000000000000000000001"
    );
    assert_eq!(accepts_sdk_revision(revision).get(), 3);
}

#[test]
fn api_sources_do_not_redeclare_sdk_ids_or_revisions() {
    let api_dir = format!("{}/src/api", env!("CARGO_MANIFEST_DIR"));
    let forbidden = [
        "TaskId",
        "RunId",
        "TurnId",
        "RequestId",
        "InteractionId",
        "ConversationId",
        "WorkspaceId",
        "ReviewSubmissionId",
        "RevisionToken",
        "Cursor",
        "TaskRevision",
    ];

    for entry in fs::read_dir(api_dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|value| value.to_str()) != Some("rs") {
            continue;
        }
        let source = fs::read_to_string(&path).unwrap();
        for name in forbidden {
            assert!(
                !source.contains(&format!("pub struct {name}")),
                "{} redeclares SDK type {name}",
                path.display()
            );
        }
    }
}

#[test]
fn exporter_writes_stable_schema_fixture_inputs() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    contract_export::export_to(first.path()).unwrap();
    contract_export::export_to(second.path()).unwrap();

    for relative in [
        "schema.json",
        "manifest-inputs.json",
        "fixtures/api_error.json",
        "fixtures/task_projection.json",
        "fixtures/task_changes.json",
    ] {
        assert_eq!(
            fs::read(first.path().join(relative)).unwrap(),
            fs::read(second.path().join(relative)).unwrap(),
            "unstable contract output: {relative}"
        );
    }
}
