use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use wreq::header::{HeaderName, HeaderValue};

use crate::{Error, Result};

/// Extra HTTP headers and top-level JSON fields for model requests.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RequestFields {
    #[serde(
        default = "super::types::default_true",
        skip_serializing_if = "is_true"
    )]
    pub inherit: bool,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub body: BTreeMap<String, Value>,
}

impl Default for RequestFields {
    fn default() -> Self {
        Self {
            inherit: true,
            headers: BTreeMap::new(),
            body: BTreeMap::new(),
        }
    }
}

impl fmt::Debug for RequestFields {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RequestFields")
            .field("inherit", &self.inherit)
            .field("headers", &self.headers.keys().collect::<Vec<_>>())
            .field("body", &self.body.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl RequestFields {
    pub fn validate(&self) -> Result<()> {
        let mut header_names = std::collections::BTreeSet::new();
        for (name, value) in &self.headers {
            let header = HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
                Error::ConfigLoad("request.headers contains an invalid HTTP header name".into())
            })?;
            if !header_names.insert(header.as_str().to_owned()) {
                return Err(Error::ConfigLoad(format!(
                    "request.headers contains duplicate header '{header}'"
                )));
            }
            if matches!(
                header.as_str(),
                "authorization"
                    | "x-api-key"
                    | "api-key"
                    | "proxy-authorization"
                    | "host"
                    | "content-type"
                    | "content-length"
                    | "transfer-encoding"
                    | "connection"
                    | "upgrade"
            ) {
                return Err(Error::ConfigLoad(format!(
                    "request header '{header}' is managed by the adapter"
                )));
            }
            HeaderValue::from_str(value).map_err(|_| {
                Error::ConfigLoad(format!("request header '{header}' has an invalid value"))
            })?;
            validate_variable(value, true)?;
        }
        for (name, value) in &self.body {
            if matches!(
                name.as_str(),
                "model"
                    | "input"
                    | "messages"
                    | "system"
                    | "instructions"
                    | "tools"
                    | "tool_choice"
                    | "stream"
                    | "stream_options"
                    | "store"
                    | "previous_response_id"
                    | "include"
                    | "max_output_tokens"
                    | "max_completion_tokens"
                    | "max_tokens"
                    | "reasoning"
                    | "reasoning_effort"
                    | "thinking"
                    | "output_config"
                    | "service_tier"
            ) {
                return Err(Error::ConfigLoad(format!(
                    "request body field '{name}' is managed by the adapter"
                )));
            }
            validate_body_variables(value)?;
        }
        Ok(())
    }
}

fn is_true(value: &bool) -> bool {
    *value
}

fn validate_body_variables(value: &Value) -> Result<()> {
    match value {
        Value::String(value) => validate_variable(value, false),
        Value::Array(values) => values.iter().try_for_each(validate_body_variables),
        Value::Object(values) => values.values().try_for_each(validate_body_variables),
        _ => Ok(()),
    }
}

fn validate_variable(value: &str, header: bool) -> Result<()> {
    if value.starts_with("{{")
        && value.ends_with("}}")
        && !matches!(value, "{{session_id}}" | "{{prompt_cache_key}}")
        && !(header && value == "{{request_id}}")
    {
        return Err(Error::ConfigLoad(
            "unsupported variable in request configuration".into(),
        ));
    }
    Ok(())
}
