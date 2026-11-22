use serde_json::{json, Map, Value};

use crate::session::runtime::OutputSelector;
use crate::tool::builtin::run_command::output::search::{
    OutputSearch, DEFAULT_MATCHES, MAX_LITERAL_BYTES, MAX_MATCHES,
};
use crate::tool::builtin::run_command::output::validate_output_id;
use crate::tool::{ToolDefinition, ToolResultEnvelope};
use crate::{Error, OutputRead, OutputStream, Result, Session, MAX_VALUE_NAME_BYTES};

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
        description: "Read output by exact ID, including bytes already captured from a running command, or by your exact value name. Select a byte offset and bounded scan limit, with an optional literal search. Continue with next_offset. When has_more and complete are both false, you have reached the current end of a growing capture.".into(),
        input_schema: json!({
            "type":"object",
            "properties":{
                "id":{"type":"string","minLength":1,"maxLength":128,"pattern":"^[A-Za-z0-9_-]+$"},
                "name":{"type":"string","minLength":1,"maxLength":MAX_VALUE_NAME_BYTES,"pattern":"^[A-Za-z][A-Za-z0-9_-]*$"},
                "offset":{"type":"integer","minimum":0,"description":"Starting raw byte offset across both output streams; defaults to 0."},
                "limit":{"type":"integer","minimum":1,"default":bounds.default,"description":format!("Raw byte budget capped at {}. Continue with next_offset.", bounds.scan)},
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

pub(crate) fn execute_search(
    session: &Session,
    agent: &str,
    args: &Value,
    max_json_result_bytes: usize,
    cancelled: impl Fn() -> bool,
) -> Result<ToolResultEnvelope> {
    let (selector, mut request) = search_request(args)?;
    request.limit = request
        .limit
        .min((max_json_result_bytes.saturating_sub(512) / 96).max(1));
    let page = match session.search_selected_output(agent, selector, request, cancelled) {
        Ok(page) => page,
        Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::Interrupted => {
            return Ok(ToolResultEnvelope::cancelled("output search cancelled"));
        }
        Err(error) => return Err(error),
    };
    let summary = format!("searched retained output {}", page.output_id);
    let value = serde_json::to_value(&page)?;
    let content = serde_json::to_string(&value)?;
    if content.len() > max_json_result_bytes {
        return Err(Error::InvalidArgument(
            "search_output page exceeds the JSON result bound".into(),
        ));
    }
    Ok(ToolResultEnvelope::ok(summary, content, value))
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
    let pinned = pin_name(session, agent, turn, selector)?;
    apply_pinned_name(args, pinned);
    Ok(())
}

pub(crate) fn prepare_search(
    session: &Session,
    agent: &str,
    turn: u64,
    args: &mut Value,
) -> Result<()> {
    let (selector, _) = search_request(args)?;
    let pinned = pin_name(session, agent, turn, selector)?;
    apply_pinned_name(args, pinned);
    Ok(())
}

fn pin_name(
    session: &Session,
    agent: &str,
    turn: u64,
    selector: OutputSelector<'_>,
) -> Result<Option<String>> {
    let id = session.pin_readable_output(agent, turn, selector)?;
    Ok(matches!(selector, OutputSelector::Name(_)).then_some(id))
}

fn apply_pinned_name(args: &mut Value, pinned: Option<String>) {
    if let Some(id) = pinned {
        let object = args.as_object_mut().expect("validated output arguments");
        object.remove("name");
        object.insert("id".into(), id.into());
    }
}

fn selection(object: &Map<String, Value>) -> Result<(OutputSelector<'_>, u64)> {
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
    Ok((selector, offset))
}

fn search_request(args: &Value) -> Result<(OutputSelector<'_>, OutputSearch<'_>)> {
    let invalid_search = |message| Error::InvalidArgument(format!("search_output: {message}"));
    let object = args
        .as_object()
        .ok_or_else(|| invalid_search("arguments must be an object"))?;
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "id" | "name" | "offset" | "limit" | "text" | "stream"
        )
    }) {
        return Err(invalid_search("unknown argument"));
    }
    let (selector, offset) = selection(object)?;
    let text = args["text"]
        .as_str()
        .filter(|text| (1..=MAX_LITERAL_BYTES).contains(&text.len()))
        .ok_or_else(|| invalid_search("text must contain 1–4096 UTF-8 bytes"))?;
    let limit = match object.get("limit") {
        Some(value) => value
            .as_u64()
            .filter(|limit| (1..=MAX_MATCHES as u64).contains(limit))
            .ok_or_else(|| invalid_search("limit must be an integer between 1 and 128 matches"))?
            as usize,
        None => DEFAULT_MATCHES,
    };
    let stream = match object.get("stream") {
        None => None,
        Some(value) => Some(match value.as_str() {
            Some("stdout") => OutputStream::Stdout,
            Some("stderr") => OutputStream::Stderr,
            _ => return Err(invalid_search("stream must be stdout or stderr")),
        }),
    };
    Ok((
        selector,
        OutputSearch {
            text,
            stream,
            offset,
            limit,
        },
    ))
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
    let (selector, offset) = selection(object)?;
    let limit = match object.get("limit") {
        Some(value) => value
            .as_u64()
            .filter(|value| *value > 0)
            .ok_or_else(|| invalid("limit must be a positive integer"))?
            .min(bounds.scan as u64) as usize,
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
        let limit = &definition.input_schema["properties"]["limit"];
        assert_eq!(1, limit["minimum"]);
        assert_eq!(DEFAULT_READ_BYTES, limit["default"]);
        assert!(limit.get("maximum").is_none());
        assert!(limit["description"].as_str().unwrap().contains("8192"));
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
    fn malformed_output_requests_are_rejected() {
        for args in [
            json!(null),
            json!({}),
            json!({"id":1}),
            json!({"id":"out","path":"outside"}),
            json!({"id":"out","offset":-1}),
            json!({"id":"out","offset":1.5}),
            json!({"id":"out","offset":null}),
            json!({"id":"out","limit":0}),
            json!({"id":"out","limit":-1}),
            json!({"id":"out","limit":1.5}),
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
        let limit = &definition.input_schema["properties"]["limit"];
        assert_eq!(DEFAULT_LOCAL_READ_BYTES, limit["default"]);
        assert!(limit.get("maximum").is_none());
        assert!(limit["description"].as_str().unwrap().contains("80000"));
        let args = json!({"name":"baseline"});
        let (selected, read) = request(&args, LOCAL_BOUNDS).unwrap();
        assert!(matches!(selected, OutputSelector::Name("baseline")));
        assert_eq!(DEFAULT_LOCAL_READ_BYTES, read.limit);
        let args = json!({"id":"captured","offset":17,"limit":u64::MAX,"search":"marker"});
        let (_, model) = request(&args, MODEL_BOUNDS).unwrap();
        let (_, local) = request(&args, LOCAL_BOUNDS).unwrap();
        assert_eq!(MAX_READ_BYTES, model.limit);
        assert_eq!(MAX_LOCAL_READ_BYTES, local.limit);
        assert_eq!(17, model.offset);
        assert_eq!(Some("marker"), model.search.as_deref());
    }
}
