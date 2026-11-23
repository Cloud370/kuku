use serde::Deserialize;
use serde_json::{json, Value};

use crate::session::runtime::OutputSelector;
use crate::tool::{ToolDefinition, ToolResultEnvelope};
use crate::{
    Error, NamedValueSource, OutputReference, Result, Session, MAX_INPUT_VALUES,
    MAX_VALUE_NAME_BYTES,
};

const MAX_LIST_VALUES: usize = 32;
const MAX_LITERAL_BYTES: usize = 16 * 1024;
const MAX_RESULT_BYTES: usize = 16 * 1024;

pub(super) fn save_definition() -> ToolDefinition {
    ToolDefinition {
        name: "save_value".into(),
        description: "Give an existing output or a small JSON value a name. Use replace to change a saved name.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": name_schema(),
                "output_id": id_schema(),
                "value": {"description": "Small JSON literal. Strings are saved as raw UTF-8 text."},
                "replace": {"type": "boolean", "description": "Defaults false."}
            },
            "required": ["name"],
            "oneOf": [{"required": ["output_id"]}, {"required": ["value"]}],
            "additionalProperties": false
        }),
        read_only: false,
        max_result_chars: MAX_RESULT_BYTES,
        risk: "session".into(),
    }
}

pub(super) fn list_definition() -> ToolDefinition {
    ToolDefinition {
        name: "list_values".into(),
        description: "List your saved names and their output references. Use next_after for more."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "prefix": {"type": "string", "maxLength": MAX_VALUE_NAME_BYTES},
                "after": name_schema(),
                "limit": {"type": "integer", "minimum": 1, "maximum": MAX_LIST_VALUES}
            },
            "additionalProperties": false
        }),
        read_only: true,
        max_result_chars: MAX_RESULT_BYTES,
        risk: "read".into(),
    }
}

pub(super) fn handoff_schema() -> Value {
    json!({
        "type": "array",
        "description": "Exact names or available whole-output references to share with the recipient.",
        "maxItems": MAX_INPUT_VALUES,
        "items": {"oneOf": [name_schema(), {
            "type": "object",
            "properties": {"id": id_schema(), "offset": {"type": "integer", "const": 0}, "bytes": {"type": "integer", "minimum": 0}},
            "required": ["id", "offset", "bytes"],
            "additionalProperties": false
        }]}
    })
}

pub(crate) async fn save(
    session: &Session,
    agent: &str,
    args: &Value,
) -> Result<ToolResultEnvelope> {
    let (name, source, replace) = save_request(args)?;
    let value = session
        .save_named_value(agent, name, source, replace)
        .await?;
    let data = serde_json::to_value(value)?;
    Ok(ToolResultEnvelope::ok(
        "value saved",
        serde_json::to_string(&data)?,
        data,
    ))
}

pub(crate) fn list(session: &Session, agent: &str, args: &Value) -> Result<ToolResultEnvelope> {
    let request = ListRequest::deserialize(args)?;
    if request.prefix.len() > MAX_VALUE_NAME_BYTES || request.after.len() > MAX_VALUE_NAME_BYTES {
        return Err(Error::InvalidArgument(
            "list_values cursor or prefix exceeds the value name limit".into(),
        ));
    }
    let limit = request.limit.unwrap_or(MAX_LIST_VALUES);
    if !(1..=MAX_LIST_VALUES).contains(&limit) {
        return Err(Error::InvalidArgument(format!(
            "list_values limit must be 1-{MAX_LIST_VALUES}"
        )));
    }
    let mut values = session
        .named_values(agent)?
        .into_iter()
        .filter(|value| value.name > request.after && value.name.starts_with(&request.prefix));
    let page = values.by_ref().take(limit).collect::<Vec<_>>();
    let next_after = values
        .next()
        .and_then(|_| page.last().map(|value| &value.name));
    let data = json!({"values": page, "next_after": next_after});
    Ok(ToolResultEnvelope::ok(
        "named values",
        serde_json::to_string(&data)?,
        data,
    ))
}

pub(crate) fn prepare_save(session: &Session, agent: &str, turn: u64, args: &Value) -> Result<()> {
    let (_, source, _) = save_request(args)?;
    if let NamedValueSource::Output(id) = source {
        session.pin_output_selectors(agent, turn, &[OutputSelector::Id(&id)])?;
    }
    Ok(())
}

pub(super) fn prepare_handoff(
    session: &Session,
    agent: &str,
    turn: u64,
    args: &mut Value,
) -> Result<()> {
    let Some(values) = args.get("values") else {
        return Ok(());
    };
    let values = Vec::<SelectedValue>::deserialize(values)?;
    if values.len() > MAX_INPUT_VALUES {
        return Err(Error::InvalidArgument(format!(
            "send_message can share at most {MAX_INPUT_VALUES} outputs"
        )));
    }
    let selectors = values
        .iter()
        .map(|value| match value {
            SelectedValue::Name(name) => OutputSelector::Name(name),
            SelectedValue::Reference(reference) => OutputSelector::Id(&reference.id),
        })
        .collect::<Vec<_>>();
    let references = session.pin_output_selectors(agent, turn, &selectors)?;
    for (input, actual) in values.iter().zip(&references) {
        if let SelectedValue::Reference(expected) = input {
            if expected != actual {
                return Err(Error::InvalidArgument(format!(
                    "send_message reference '{}' must match the entire captured extent",
                    actual.id
                )));
            }
        }
    }
    args["values"] = serde_json::to_value(references)?;
    Ok(())
}

pub(super) fn handoff_values(args: &Value) -> Result<Vec<OutputReference>> {
    args.get("values")
        .map(Vec::<OutputReference>::deserialize)
        .transpose()
        .map(|values| values.unwrap_or_default())
        .map_err(Into::into)
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ListRequest {
    after: String,
    prefix: String,
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SelectedValue {
    Name(String),
    Reference(OutputReference),
}

fn save_request(args: &Value) -> Result<(&str, NamedValueSource, bool)> {
    let object = args
        .as_object()
        .ok_or_else(|| invalid_save("arguments must be an object"))?;
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "name" | "output_id" | "value" | "replace"))
    {
        return Err(invalid_save("unknown argument"));
    }
    let name = args["name"]
        .as_str()
        .ok_or_else(|| invalid_save("name must be a string"))?;
    let replace = match object.get("replace") {
        Some(value) => value
            .as_bool()
            .ok_or_else(|| invalid_save("replace must be a boolean"))?,
        None => false,
    };
    let source = match (object.get("output_id"), object.get("value")) {
        (Some(id), None) => NamedValueSource::Output(
            id.as_str()
                .ok_or_else(|| invalid_save("output_id must be a string"))?
                .into(),
        ),
        (None, Some(value)) => literal_value(value)?,
        _ => return Err(invalid_save("provide exactly one of output_id or value")),
    };
    Ok((name, source, replace))
}

fn literal_value(value: &Value) -> Result<NamedValueSource> {
    if let Some(text) = value.as_str() {
        if text.len() <= MAX_LITERAL_BYTES {
            return Ok(NamedValueSource::Text(text.into()));
        }
    } else if serde_json::to_vec(value)?.len() <= MAX_LITERAL_BYTES {
        return Ok(NamedValueSource::Json(value.clone()));
    }
    Err(invalid_save(
        "literal value exceeds 16384 bytes; save an existing output_id",
    ))
}

fn name_schema() -> Value {
    json!({"type": "string", "minLength": 1, "maxLength": MAX_VALUE_NAME_BYTES, "pattern": "^[A-Za-z][A-Za-z0-9_-]*$"})
}

fn id_schema() -> Value {
    json!({"type": "string", "minLength": 1, "maxLength": 128, "pattern": "^[A-Za-z0-9_-]+$"})
}

fn invalid_save(message: &str) -> Error {
    Error::InvalidArgument(format!("save_value: {message}"))
}
