use serde_json::{Map, Value};

use super::ModelStopReason;

pub(super) fn string_field(object: &Map<String, Value>, key: &str) -> Option<String> {
    object.get(key)?.as_str().map(ToOwned::to_owned)
}

pub(super) fn optional_string_field(object: &Map<String, Value>, key: &str) -> Option<String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

pub(super) fn optional_json_field(object: &Map<String, Value>, key: &str) -> Option<Value> {
    object.get(key).cloned().filter(|value| !value.is_null())
}

pub(super) fn u64_field(object: &Map<String, Value>, key: &str) -> Option<u64> {
    object.get(key)?.as_u64()
}

pub(super) fn usize_field(object: &Map<String, Value>, key: &str) -> Option<usize> {
    usize::try_from(object.get(key)?.as_u64()?).ok()
}

pub(super) fn u32_field(object: &Map<String, Value>, key: &str) -> Option<u32> {
    u32::try_from(object.get(key)?.as_u64()?).ok()
}

pub(super) fn u8_field(object: &Map<String, Value>, key: &str) -> Option<u8> {
    u8::try_from(object.get(key)?.as_u64()?).ok()
}

pub(super) fn optional_u64_field(object: &Map<String, Value>, key: &str) -> Option<u64> {
    object.get(key).and_then(Value::as_u64)
}

pub(super) fn optional_stop_reason_field(
    object: &Map<String, Value>,
    key: &str,
) -> Option<ModelStopReason> {
    match object.get(key) {
        None => None,
        Some(Value::String(reason)) => {
            ModelStopReason::from_wire(reason).or(Some(ModelStopReason::InvalidResponse))
        }
        Some(_) => Some(ModelStopReason::InvalidResponse),
    }
}

pub(super) fn bool_field(object: &Map<String, Value>, key: &str) -> Option<bool> {
    object.get(key)?.as_bool()
}

pub(super) fn vec_string_field(object: &Map<String, Value>, key: &str) -> Vec<String> {
    object
        .get(key)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}
