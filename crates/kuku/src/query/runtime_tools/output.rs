use serde_json::{json, Value};

use crate::tool::{ToolDefinition, ToolResultEnvelope};
use crate::{Error, OutputRead, Result, Session};

const DEFAULT_READ_BYTES: usize = 4096;
const MAX_READ_BYTES: usize = 8192;
const MAX_SEARCH_BYTES: usize = 4096;
const MAX_RESULT_BYTES: usize = 16_384;

pub(super) fn definition() -> ToolDefinition {
    ToolDefinition {
        name: "read_output".into(),
        description: "Read retained command output by its Session output ID. Select a byte offset and bounded scan limit, with an optional literal search. Returns JSON with next_offset and capture status.".into(),
        input_schema: json!({
            "type":"object",
            "properties":{
                "id":{"type":"string","minLength":1,"maxLength":128,"pattern":"^[A-Za-z0-9_-]+$"},
                "offset":{"type":"integer","minimum":0,"description":"Starting raw byte offset across both output streams; defaults to 0."},
                "limit":{"type":"integer","minimum":1,"maximum":MAX_READ_BYTES,"description":"Maximum raw bytes to scan; defaults to 4096. A page can be smaller to fit the JSON result bound."},
                "search":{"type":"string","minLength":1,"maxLength":MAX_SEARCH_BYTES,"description":"Literal filter of at most 4096 UTF-8 bytes, no longer than limit. Continue with next_offset even when no matches are returned."}
            },
            "required":["id"],
            "additionalProperties":false
        }),
        read_only: true,
        max_result_chars: MAX_RESULT_BYTES,
        risk: "read".into(),
    }
}

pub(super) fn execute(session: &Session, args: &Value) -> Result<ToolResultEnvelope> {
    let (id, mut request) = request(args)?;
    loop {
        let chunk = session.read_output(id, &request)?;
        let value = serde_json::to_value(&chunk)?;
        let content = serde_json::to_string(&value)?;
        if content.len() <= MAX_RESULT_BYTES {
            return Ok(ToolResultEnvelope::ok(
                format!(
                    "output bytes {}..{} of {}",
                    chunk.offset, chunk.next_offset, chunk.total_bytes
                ),
                content,
                value,
            ));
        }
        let minimum = request.search.as_ref().map_or(1, String::len);
        if request.limit <= minimum {
            return Err(Error::InvalidArgument(
                "read_output page exceeds the JSON result bound; use a shorter search literal"
                    .into(),
            ));
        }
        request.limit = (request.limit / 2).max(minimum);
    }
}

fn request(args: &Value) -> Result<(&str, OutputRead)> {
    let object = args
        .as_object()
        .ok_or_else(|| invalid("arguments must be an object"))?;
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "id" | "offset" | "limit" | "search"))
    {
        return Err(invalid("unknown argument"));
    }
    let id = args["id"]
        .as_str()
        .ok_or_else(|| invalid("id must be a string"))?;
    let offset = match object.get("offset") {
        Some(value) => value
            .as_u64()
            .ok_or_else(|| invalid("offset must be a nonnegative integer"))?,
        None => 0,
    };
    let limit = match object.get("limit") {
        Some(value) => value
            .as_u64()
            .filter(|value| (1..=MAX_READ_BYTES as u64).contains(value))
            .ok_or_else(|| invalid("limit must be an integer between 1 and 8192 bytes"))?
            as usize,
        None => DEFAULT_READ_BYTES,
    };
    let search = match object.get("search") {
        Some(value) => {
            let search = value
                .as_str()
                .filter(|search| {
                    !search.is_empty() && search.len() <= MAX_SEARCH_BYTES && search.len() <= limit
                })
                .ok_or_else(|| invalid("search must contain 1-4096 UTF-8 bytes and fit limit"))?;
            Some(search.to_string())
        }
        None => None,
    };
    Ok((
        id,
        OutputRead {
            offset,
            limit,
            search,
        },
    ))
}

fn invalid(message: &str) -> Error {
    Error::InvalidArgument(format!("read_output: {message}"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{definition, request, DEFAULT_READ_BYTES, MAX_READ_BYTES, MAX_RESULT_BYTES};

    #[test]
    fn output_definition_is_read_only_with_a_bounded_result() {
        let definition = definition();
        assert_eq!("read_output", definition.name);
        assert!(definition.read_only);
        assert_eq!("read", definition.risk);
        assert_eq!(MAX_RESULT_BYTES, definition.max_result_chars);
        assert_eq!(json!(["id"]), definition.input_schema["required"]);
        assert_eq!(
            MAX_READ_BYTES,
            definition.input_schema["properties"]["limit"]["maximum"]
        );
    }

    #[test]
    fn output_request_preserves_explicit_arguments_and_defaults() {
        let args = json!({"id":"output_1"});
        let (id, read) = request(&args).unwrap();
        assert_eq!("output_1", id);
        assert_eq!(0, read.offset);
        assert_eq!(DEFAULT_READ_BYTES, read.limit);
        assert!(read.search.is_none());
        let args = json!({"id":"output_1","offset":17,"limit":1024,"search":"literal.*标记"});
        let (_, read) = request(&args).unwrap();
        assert_eq!(17, read.offset);
        assert_eq!(1024, read.limit);
        assert_eq!(Some("literal.*标记"), read.search.as_deref());
    }

    #[test]
    fn malformed_or_unbounded_output_requests_are_rejected() {
        for args in [
            json!(null),
            json!({}),
            json!({"id":1}),
            json!({"id":"out","path":"outside"}),
            json!({"id":"out","offset":-1}),
            json!({"id":"out","offset":1.5}),
            json!({"id":"out","offset":null}),
            json!({"id":"out","limit":0}),
            json!({"id":"out","limit":MAX_READ_BYTES+1}),
            json!({"id":"out","limit":"12"}),
            json!({"id":"out","search":""}),
            json!({"id":"out","search":null}),
            json!({"id":"out","limit":3,"search":"标记"}),
            json!({"id":"out","limit":MAX_READ_BYTES,"search":"x".repeat(4097)}),
        ] {
            assert!(request(&args).is_err(), "unexpected valid request: {args}");
        }
    }
}
