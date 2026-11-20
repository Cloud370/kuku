use serde_json::{json, Value};

use crate::session::runtime::OutputSelector;
use crate::tool::builtin::run_command::output::validate_output_id;
use crate::tool::{ToolDefinition, ToolResultEnvelope};
use crate::{Error, OutputRead, Result, Session, MAX_VALUE_NAME_BYTES};

const DEFAULT_READ_BYTES: usize = 4096;
const MAX_READ_BYTES: usize = 8192;
const MAX_SEARCH_BYTES: usize = 4096;
const MAX_RESULT_BYTES: usize = 16_384;
const DEFAULT_LOCAL_READ_BYTES: usize = 65_536;
const MAX_LOCAL_READ_BYTES: usize = 80_000;
const DEFAULT_LOCAL_RESULT_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy)]
struct PageBounds {
    default: usize,
    scan: usize,
    result: usize,
}

const MODEL_BOUNDS: PageBounds = PageBounds {
    default: DEFAULT_READ_BYTES,
    scan: MAX_READ_BYTES,
    result: MAX_RESULT_BYTES,
};

const LOCAL_BOUNDS: PageBounds = PageBounds {
    default: DEFAULT_LOCAL_READ_BYTES,
    scan: MAX_LOCAL_READ_BYTES,
    result: DEFAULT_LOCAL_RESULT_BYTES,
};

pub(super) fn definition() -> ToolDefinition {
    definition_with_bounds(MODEL_BOUNDS)
}

pub(crate) fn definition_local() -> ToolDefinition {
    definition_with_bounds(LOCAL_BOUNDS)
}

fn definition_with_bounds(bounds: PageBounds) -> ToolDefinition {
    ToolDefinition {
        name: "read_output".into(),
        description: "Read retained output by its exact ID or your exact value name. Select a byte offset and bounded scan limit, with an optional literal search. Returns JSON with next_offset and capture status.".into(),
        input_schema: json!({
            "type":"object",
            "properties":{
                "id":{"type":"string","minLength":1,"maxLength":128,"pattern":"^[A-Za-z0-9_-]+$"},
                "name":{"type":"string","minLength":1,"maxLength":MAX_VALUE_NAME_BYTES,"pattern":"^[A-Za-z][A-Za-z0-9_-]*$"},
                "offset":{"type":"integer","minimum":0,"description":"Starting raw byte offset across both output streams; defaults to 0."},
                "limit":{"type":"integer","minimum":1,"maximum":bounds.scan,"description":format!("Maximum raw bytes to scan. Defaults to {}. A page can be smaller to fit the JSON result bound.", bounds.default)},
                "search":{"type":"string","minLength":1,"maxLength":MAX_SEARCH_BYTES,"description":"Literal filter of at most 4096 UTF-8 bytes, no longer than limit. Continue with next_offset even when no matches are returned."}
            },
            "oneOf":[{"required":["id"]},{"required":["name"]}],
            "additionalProperties":false
        }),
        read_only: true,
        max_result_chars: bounds.result,
        risk: "read".into(),
    }
}

pub(crate) fn execute(session: &Session, agent: &str, args: &Value) -> Result<ToolResultEnvelope> {
    execute_with_bounds(session, agent, args, MODEL_BOUNDS)
}

pub(crate) fn execute_local(
    session: &Session,
    agent: &str,
    args: &Value,
    max_json_result_bytes: usize,
) -> Result<ToolResultEnvelope> {
    execute_with_bounds(
        session,
        agent,
        args,
        PageBounds {
            result: max_json_result_bytes,
            ..LOCAL_BOUNDS
        },
    )
}

fn execute_with_bounds(
    session: &Session,
    agent: &str,
    args: &Value,
    bounds: PageBounds,
) -> Result<ToolResultEnvelope> {
    let (mut selector, mut request) = request(args, bounds)?;
    let resolved;
    if let OutputSelector::Name(name) = selector {
        resolved = session.named_value(agent, name)?.reference.id;
        selector = OutputSelector::Id(&resolved);
    }
    loop {
        let chunk = session.read_selected_output(agent, selector, &request)?;
        let value = serde_json::to_value(&chunk)?;
        let content = serde_json::to_string(&value)?;
        if content.len() <= bounds.result {
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

pub(crate) fn prepare(session: &Session, agent: &str, turn: u64, args: &mut Value) -> Result<()> {
    prepare_with_bounds(session, agent, turn, args, MODEL_BOUNDS)
}

pub(crate) fn prepare_local(
    session: &Session,
    agent: &str,
    turn: u64,
    args: &mut Value,
) -> Result<()> {
    prepare_with_bounds(session, agent, turn, args, LOCAL_BOUNDS)
}

fn prepare_with_bounds(
    session: &Session,
    agent: &str,
    turn: u64,
    args: &mut Value,
    bounds: PageBounds,
) -> Result<()> {
    let (selector, _) = request(args, bounds)?;
    let references = session.pin_output_selectors(agent, turn, &[selector])?;
    if matches!(selector, OutputSelector::Name(_)) {
        let object = args.as_object_mut().expect("validated output arguments");
        object.remove("name");
        object.insert("id".into(), references[0].id.clone().into());
    }
    Ok(())
}

fn request(args: &Value, bounds: PageBounds) -> Result<(OutputSelector<'_>, OutputRead)> {
    let object = args
        .as_object()
        .ok_or_else(|| invalid("arguments must be an object"))?;
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "id" | "name" | "offset" | "limit" | "search"))
    {
        return Err(invalid("unknown argument"));
    }
    let selector = match (object.get("id"), object.get("name")) {
        (Some(id), None) => {
            let id = id.as_str().ok_or_else(|| invalid("id must be a string"))?;
            validate_output_id(id).map_err(|error| invalid(&error.to_string()))?;
            OutputSelector::Id(id)
        }
        (None, Some(name)) => OutputSelector::Name(
            name.as_str()
                .ok_or_else(|| invalid("name must be a string"))?,
        ),
        _ => return Err(invalid("provide exactly one of id or name")),
    };
    let offset = match object.get("offset") {
        Some(value) => value
            .as_u64()
            .ok_or_else(|| invalid("offset must be a nonnegative integer"))?,
        None => 0,
    };
    let limit = match object.get("limit") {
        Some(value) => value
            .as_u64()
            .filter(|value| (1..=bounds.scan as u64).contains(value))
            .ok_or_else(|| {
                invalid(&format!(
                    "limit must be an integer between 1 and {} bytes",
                    bounds.scan
                ))
            })? as usize,
        None => bounds.default,
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
        selector,
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

    use super::{
        definition, definition_local, request, DEFAULT_LOCAL_READ_BYTES, DEFAULT_READ_BYTES,
        LOCAL_BOUNDS, MAX_LOCAL_READ_BYTES, MAX_READ_BYTES, MAX_RESULT_BYTES, MODEL_BOUNDS,
    };
    use crate::session::runtime::OutputSelector;

    #[test]
    fn output_definition_is_read_only_with_a_bounded_result() {
        let definition = definition();
        assert_eq!("read_output", definition.name);
        assert!(definition.read_only);
        assert_eq!("read", definition.risk);
        assert_eq!(MAX_RESULT_BYTES, definition.max_result_chars);
        assert_eq!(
            json!([{"required":["id"]},{"required":["name"]}]),
            definition.input_schema["oneOf"]
        );
        assert_eq!(
            MAX_READ_BYTES,
            definition.input_schema["properties"]["limit"]["maximum"]
        );
    }

    #[test]
    fn output_request_preserves_explicit_arguments_and_defaults() {
        let args = json!({"id":"output_1"});
        let (id, read) = request(&args, MODEL_BOUNDS).unwrap();
        assert!(matches!(id, OutputSelector::Id("output_1")));
        assert_eq!(0, read.offset);
        assert_eq!(DEFAULT_READ_BYTES, read.limit);
        assert!(read.search.is_none());
        let args = json!({"id":"output_1","offset":17,"limit":1024,"search":"literal.*标记"});
        let (_, read) = request(&args, MODEL_BOUNDS).unwrap();
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
            assert!(
                request(&args, MODEL_BOUNDS).is_err(),
                "unexpected valid request: {args}"
            );
        }
    }

    #[test]
    fn local_output_reader_has_a_larger_page_than_the_model_reader() {
        let definition = definition_local();
        assert_eq!(
            MAX_LOCAL_READ_BYTES,
            definition.input_schema["properties"]["limit"]["maximum"]
        );
        let args = json!({"name":"baseline"});
        let (selected, read) = request(&args, LOCAL_BOUNDS).unwrap();
        assert!(matches!(selected, OutputSelector::Name("baseline")));
        assert_eq!(DEFAULT_LOCAL_READ_BYTES, read.limit);
        let args = json!({"id":"captured","limit":MAX_LOCAL_READ_BYTES});
        assert!(request(&args, MODEL_BOUNDS).is_err());
        assert!(request(&args, LOCAL_BOUNDS).is_ok());
    }
}
