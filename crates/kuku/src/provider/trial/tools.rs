use serde_json::{Value, json};

use super::ProviderKind;

const NAME: &str = "kuku_setup_check";
const PROMPT: &str = "Call kuku_setup_check with no arguments. After the tool returns, reply with its result exactly.";

pub(super) fn set_challenge(kind: ProviderKind, body: &mut Value) {
    let parameters = json!({"type":"object", "properties":{}, "additionalProperties":false});
    body["tools"] = match kind {
        ProviderKind::OpenAiResponses => {
            json!([{"type":"function", "name":NAME, "parameters":parameters}])
        }
        ProviderKind::OpenAiCompatible => {
            json!([{"type":"function", "function":{"name":NAME, "parameters":parameters}}])
        }
        ProviderKind::Anthropic => json!([{"name":NAME, "input_schema":parameters}]),
    };
    let message = json!([{"role":"user", "content":PROMPT}]);
    body[input_field(kind)] = message;
}

pub(super) fn continue_call(
    kind: ProviderKind,
    body: &mut Value,
    response: &Value,
    answer: &str,
) -> bool {
    let Some(input) = body[input_field(kind)].as_array_mut() else {
        return false;
    };
    match kind {
        ProviderKind::OpenAiResponses => {
            let Some(output) = response["output"].as_array() else {
                return false;
            };
            let Some(call) = output
                .iter()
                .find(|item| item["type"] == "function_call" && item["name"] == NAME)
            else {
                return false;
            };
            let Some(id) = call["call_id"].as_str().filter(|id| !id.is_empty()) else {
                return false;
            };
            if !empty_arguments(&call["arguments"]) {
                return false;
            }
            input.extend(output.iter().cloned());
            input.push(json!({"type":"function_call_output", "call_id":id, "output":answer}));
        }
        ProviderKind::OpenAiCompatible => {
            let message = &response["choices"][0]["message"];
            let call = &message["tool_calls"][0];
            let Some(id) = call["id"].as_str().filter(|id| !id.is_empty()) else {
                return false;
            };
            if call["function"]["name"] != NAME || !empty_arguments(&call["function"]["arguments"])
            {
                return false;
            }
            let mut assistant = message.clone();
            assistant["role"] = json!("assistant");
            input.push(assistant);
            input.push(json!({"role":"tool", "tool_call_id":id, "content":answer}));
        }
        ProviderKind::Anthropic => {
            let Some(content) = response["content"].as_array() else {
                return false;
            };
            let Some(call) = content
                .iter()
                .find(|item| item["type"] == "tool_use" && item["name"] == NAME)
            else {
                return false;
            };
            let Some(id) = call["id"].as_str().filter(|id| !id.is_empty()) else {
                return false;
            };
            if !empty_arguments(&call["input"]) {
                return false;
            }
            input.push(json!({"role":"assistant", "content":content}));
            input.push(json!({"role":"user", "content":[{"type":"tool_result", "tool_use_id":id, "content":answer}]}));
        }
    }
    true
}

pub(super) fn replied_with(kind: ProviderKind, reply: &Value, answer: &str) -> bool {
    let matches = |value: &Value| value.as_str().is_some_and(|text| text.trim() == answer);
    match kind {
        ProviderKind::OpenAiCompatible => {
            let content = &reply["choices"][0]["message"]["content"];
            matches(content)
                || content
                    .as_array()
                    .is_some_and(|parts| parts.iter().any(|part| matches(&part["text"])))
        }
        ProviderKind::OpenAiResponses => reply["output"].as_array().is_some_and(|output| {
            output
                .iter()
                .filter_map(|item| item["content"].as_array())
                .flatten()
                .any(|part| matches(&part["text"]))
        }),
        ProviderKind::Anthropic => reply["content"]
            .as_array()
            .is_some_and(|parts| parts.iter().any(|part| matches(&part["text"]))),
    }
}

fn input_field(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::OpenAiResponses => "input",
        ProviderKind::OpenAiCompatible | ProviderKind::Anthropic => "messages",
    }
}

fn empty_arguments(value: &Value) -> bool {
    value.as_object().is_some_and(serde_json::Map::is_empty)
        || value
            .as_str()
            .and_then(|text| serde_json::from_str::<Value>(text).ok())
            .is_some_and(|args| args.as_object().is_some_and(serde_json::Map::is_empty))
}
