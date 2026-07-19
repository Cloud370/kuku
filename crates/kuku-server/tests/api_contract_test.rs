use std::fs;

use kuku_server::api::{
    contract_export, ApiError, ApiErrorCode, ApiVersion, ContextSnapshot, PageCursor,
    PlatformCatalog, PlatformStatus, ReviewSnapshot, SettingsSnapshot, TaskChange, TaskDelta,
    TaskId, TaskProjection, TaskRevision, TaskStreamEvent, UpdateSettingsRequest, WebApiContract,
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
fn task_change_fixture_covers_every_change_and_both_outer_deltas() {
    let bundle = fixture("task_changes.json");
    let changes = bundle["changes"].as_array().unwrap();
    let decoded = changes
        .iter()
        .cloned()
        .map(serde_json::from_value::<TaskChange>)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    for (change, expected) in decoded.iter().zip(changes) {
        assert_eq!(serde_json::to_value(change).unwrap(), *expected);
    }
    let kinds = decoded
        .into_iter()
        .map(|change| serde_json::to_value(change).unwrap()["type"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
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
    );

    let deltas = serde_json::from_value::<Vec<TaskDelta>>(bundle["deltas"].clone()).unwrap();
    assert_eq!(deltas.len(), 2);
    assert_eq!(serde_json::to_value(deltas).unwrap(), bundle["deltas"]);
}

#[test]
fn stream_and_domain_family_fixtures_round_trip() {
    fn round_trip<T>(name: &str)
    where
        T: serde::de::DeserializeOwned + serde::Serialize,
    {
        let value = fixture(name);
        let decoded = serde_json::from_value::<T>(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), value, "{name}");
    }

    round_trip::<TaskStreamEvent>("task_stream_event.json");
    round_trip::<PlatformStatus>("platform_status.json");
    round_trip::<SettingsSnapshot>("settings_snapshot.json");
    round_trip::<PlatformCatalog>("platform_catalog.json");
    round_trip::<ContextSnapshot>("context_snapshot.json");
    round_trip::<ReviewSnapshot>("review_snapshot.json");
}

#[test]
fn platform_contract_exposes_connection_catalog_and_revision_fields() {
    let status: PlatformStatus = serde_json::from_value(fixture("platform_status.json")).unwrap();
    assert_eq!(
        status.connection.preferred_origin,
        "http://phone-host:17777/"
    );

    let settings: SettingsSnapshot =
        serde_json::from_value(fixture("settings_snapshot.json")).unwrap();
    assert_eq!(settings.credentials[0].provider_id, "fixture-provider");

    let catalog: PlatformCatalog =
        serde_json::from_value(fixture("platform_catalog.json")).unwrap();
    assert_eq!(catalog.default_tier.label, "Balanced");
    assert_eq!(catalog.credentials[0].provider_id, "fixture-provider");

    let request = serde_json::from_value::<UpdateSettingsRequest>(json!({
        "expected_revision": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "patch": {
            "default_tier": null,
            "default_workspace_id": null,
            "max_concurrent_runs": null
        }
    }))
    .unwrap();
    let encoded = serde_json::to_value(request).unwrap();
    assert!(encoded.get("expected_revision").is_some());
    assert!(encoded.get("expected_server_revision").is_none());
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
                    let nullable_type = schema["type"]
                        .as_array()
                        .is_some_and(|types| types.iter().any(|value| value == "null"));
                    let nullable =
                        schema == &Value::Bool(true)
                            || nullable_type
                            || schema.get("anyOf").and_then(Value::as_array).is_some_and(
                                |choices| choices.iter().any(|choice| choice["type"] == "null"),
                            );
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
fn exported_contract_keeps_representative_nulls_required_and_nullable() {
    fn accepts_null(property: &Value) -> bool {
        if property == &Value::Bool(true) {
            return true;
        }
        let nullable_type = property["type"]
            .as_array()
            .is_some_and(|types| types.iter().any(|value| value == "null"));
        nullable_type
            || property
                .get("anyOf")
                .and_then(Value::as_array)
                .is_some_and(|choices| choices.iter().any(|choice| choice["type"] == "null"))
    }

    fn assert_required_nullable(definition: &Value, name: &str) {
        let required = definition["required"].as_array().unwrap();
        assert!(
            required.iter().any(|entry| entry == name),
            "{name} is not required"
        );
        assert!(
            accepts_null(&definition["properties"][name]),
            "{name} rejects null"
        );
    }

    let schema: Value = serde_json::from_str(&contract_export::render_schema().unwrap()).unwrap();
    let definitions = schema["$defs"].as_object().unwrap();
    assert_required_nullable(&definitions["TaskProjection"], "active_run");
    assert_required_nullable(&definitions["ApiError"], "details");
    assert_required_nullable(&definitions["SettingsPatch"], "default_tier");
    assert_required_nullable(&definitions["ContextSnapshot"], "selected_request");
    assert_required_nullable(&definitions["ReviewSnapshot"], "next_cursor");

    let changes_applied = definitions["TaskDelta"]["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .find(|variant| variant["properties"]["type"]["const"] == "changes_applied")
        .unwrap();
    assert_required_nullable(changes_applied, "timeline_window");
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
        "fixtures/task_stream_event.json",
        "fixtures/platform_status.json",
        "fixtures/settings_snapshot.json",
        "fixtures/platform_catalog.json",
        "fixtures/context_snapshot.json",
        "fixtures/review_snapshot.json",
    ] {
        assert_eq!(
            fs::read(first.path().join(relative)).unwrap(),
            fs::read(second.path().join(relative)).unwrap(),
            "unstable contract output: {relative}"
        );
    }
}
