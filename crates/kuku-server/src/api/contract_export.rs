use std::fmt;
use std::fs;
use std::path::Path;

use schemars::schema_for;
use serde::Serialize;

use super::WebApiContract;

const FIXTURES: &[(&str, &str)] = &[
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
    for (name, contents) in FIXTURES {
        let value: serde_json::Value = serde_json::from_str(contents)?;
        fs::write(fixture_dir.join(name), render_pretty(&value)?)?;
    }

    let manifest_inputs = FIXTURES
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
    schema
        .get("anyOf")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|choices| {
            choices.iter().any(|choice| {
                choice.get("type").and_then(serde_json::Value::as_str) == Some("null")
            })
        })
}
