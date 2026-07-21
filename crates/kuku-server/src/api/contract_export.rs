use std::fmt;
use std::fs;
use std::path::Path;

use schemars::schema_for;
use serde::Serialize;
use serde_json::{json, Value};

use super::WebApiContract;

const SOURCE_FIXTURES: &[(&str, &str)] = &[
    (
        "api_error.json",
        include_str!("../../tests/fixtures/api/v1/api_error.json"),
    ),
    (
        "task_projection.json",
        include_str!("../../tests/fixtures/api/v1/task_projection.json"),
    ),
    (
        "task_changes.json",
        include_str!("../../tests/fixtures/api/v1/task_changes.json"),
    ),
    (
        "task_stream_event.json",
        include_str!("../../tests/fixtures/api/v1/task_stream_event.json"),
    ),
    (
        "platform_status.json",
        include_str!("../../tests/fixtures/api/v1/platform_status.json"),
    ),
    (
        "settings_snapshot.json",
        include_str!("../../tests/fixtures/api/v1/settings_snapshot.json"),
    ),
    (
        "platform_catalog.json",
        include_str!("../../tests/fixtures/api/v1/platform_catalog.json"),
    ),
    (
        "context_snapshot.json",
        include_str!("../../tests/fixtures/api/v1/context_snapshot.json"),
    ),
    (
        "review_snapshot.json",
        include_str!("../../tests/fixtures/api/v1/review_snapshot.json"),
    ),
];

#[derive(Debug)]
pub enum ExportError {
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl fmt::Display for ExportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "contract export I/O failed: {error}"),
            Self::Json(error) => write!(formatter, "contract serialization failed: {error}"),
        }
    }
}

impl std::error::Error for ExportError {}

impl From<std::io::Error> for ExportError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for ExportError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

pub fn render_schema() -> Result<String, serde_json::Error> {
    let mut schema = serde_json::to_value(schema_for!(WebApiContract))?;
    require_nullable_properties(&mut schema);
    render_pretty(&schema)
}

pub fn export_to(output_dir: &Path) -> Result<(), ExportError> {
    fs::create_dir_all(output_dir)?;
    fs::write(output_dir.join("schema.json"), render_schema()?)?;

    let fixture_dir = output_dir.join("fixtures");
    fs::create_dir_all(&fixture_dir)?;
    let fixtures = fixture_outputs()?;
    for (name, value) in &fixtures {
        fs::write(fixture_dir.join(name), render_pretty(&value)?)?;
    }

    let manifest_inputs = fixtures
        .iter()
        .map(|(name, _)| format!("fixtures/{name}"))
        .chain(std::iter::once("schema.json".to_owned()))
        .collect::<Vec<_>>();
    fs::write(
        output_dir.join("manifest-inputs.json"),
        render_pretty(&manifest_inputs)?,
    )?;
    Ok(())
}

fn fixture_outputs() -> Result<Vec<(&'static str, Value)>, serde_json::Error> {
    let mut fixtures = SOURCE_FIXTURES
        .iter()
        .map(|(name, contents)| serde_json::from_str(contents).map(|value| (*name, value)))
        .collect::<Result<Vec<_>, _>>()?;
    let projection: Value = serde_json::from_str(
        SOURCE_FIXTURES
            .iter()
            .find(|(name, _)| *name == "task_projection.json")
            .expect("task projection fixture is registered")
            .1,
    )?;

    fixtures.extend([
        (
            "task_stream_event.projection_replaced.json",
            json!({
                "api_version": 1,
                "cursor": projection["cursor"],
                "task_revision": projection["task_revision"],
                "task_id": projection["task"]["task_id"],
                "event": {
                    "type": "projection_replaced",
                    "projection": projection,
                },
            }),
        ),
        (
            "task_stream_event.timeline_single_eviction.json",
            task_stream_event(501, 1, vec![message_appended(500)], vec![message_item(0)]),
        ),
        (
            "task_stream_event.timeline_atomic_batch.json",
            task_stream_event(
                501,
                1,
                (0..=500).map(message_appended).collect(),
                vec![message_item(0)],
            ),
        ),
    ]);
    Ok(fixtures)
}

fn task_stream_event(
    cursor: u64,
    task_revision: u64,
    changes: Vec<Value>,
    evicted_items: Vec<Value>,
) -> Value {
    json!({
        "api_version": 1,
        "cursor": cursor,
        "task_revision": task_revision,
        "task_id": "tsk_000000000000000000000001",
        "event": {
            "type": "changes_applied",
            "changes": changes,
            "timeline_window": {
                "next_cursor": "timeline:v1:fixture-history",
                "evicted_items": evicted_items,
            },
        },
    })
}

fn message_appended(index: u64) -> Value {
    json!({
        "type": "message_appended",
        "item": message_item(index),
    })
}

fn message_item(index: u64) -> Value {
    json!({
        "type": "message",
        "item": {
            "message_id": format!("message-{index:06}"),
            "role": "agent",
            "text": format!("Fixture message {index}"),
            "finalized": true,
            "request_ids": [],
            "file_references": [],
            "order_key": index,
        },
    })
}

fn render_pretty<T>(value: &T) -> Result<String, serde_json::Error>
where
    T: Serialize,
{
    let mut rendered = serde_json::to_string_pretty(value)?;
    rendered.push('\n');
    Ok(rendered)
}

fn require_nullable_properties(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Array(values) => {
            for value in values {
                require_nullable_properties(value);
            }
        }
        serde_json::Value::Object(object) => {
            let nullable = object
                .get("properties")
                .and_then(serde_json::Value::as_object)
                .map(|properties| {
                    properties
                        .iter()
                        .filter(|(_, schema)| schema_accepts_null(schema))
                        .map(|(name, _)| name.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            if !nullable.is_empty() {
                let required = object
                    .entry("required")
                    .or_insert_with(|| serde_json::Value::Array(Vec::new()));
                if let serde_json::Value::Array(required) = required {
                    for name in nullable {
                        if !required.iter().any(|value| value.as_str() == Some(&name)) {
                            required.push(serde_json::Value::String(name));
                        }
                    }
                    required.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
                }
            }

            for value in object.values_mut() {
                require_nullable_properties(value);
            }
        }
        _ => {}
    }
}

fn schema_accepts_null(schema: &serde_json::Value) -> bool {
    if schema == &serde_json::Value::Bool(true) {
        return true;
    }
    let nullable_type = schema
        .get("type")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|types| types.iter().any(|value| value.as_str() == Some("null")));
    nullable_type
        || schema
            .get("anyOf")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|choices| {
                choices.iter().any(|choice| {
                    choice.get("type").and_then(serde_json::Value::as_str) == Some("null")
                })
            })
}
