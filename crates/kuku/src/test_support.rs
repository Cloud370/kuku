use serde_json::Value;

pub fn anthropic_sse_response(msg: Value) -> String {
    let id = msg
        .get("id")
        .cloned()
        .unwrap_or(Value::String("msg_1".into()));
    let model = msg
        .get("model")
        .cloned()
        .unwrap_or(Value::String("test-model".into()));
    let stop_reason = msg
        .get("stop_reason")
        .and_then(Value::as_str)
        .unwrap_or("end_turn");
    let usage = msg
        .get("usage")
        .cloned()
        .unwrap_or(serde_json::json!({"input_tokens": 0, "output_tokens": 0}));
    let content = msg
        .get("content")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut sse = String::new();

    sse.push_str(&format!(
        "event: message_start\ndata: {}\n\n",
        serde_json::json!({"type":"message_start","message":{"id":id,"model":model,"content":[],"usage":usage}})
    ));

    for (i, block) in content.iter().enumerate() {
        let btype = block.get("type").and_then(Value::as_str).unwrap_or("text");
        if btype == "text" {
            let text = block.get("text").and_then(Value::as_str).unwrap_or("");
            sse.push_str(&format!(
                "event: content_block_start\ndata: {}\n\n",
                serde_json::json!({"type":"content_block_start","index":i,"content_block":{"type":"text","text":""}})
            ));
            if !text.is_empty() {
                sse.push_str(&format!(
                    "event: content_block_delta\ndata: {}\n\n",
                    serde_json::json!({"type":"content_block_delta","index":i,"delta":{"type":"text_delta","text":text}})
                ));
            }
            sse.push_str(&format!(
                "event: content_block_stop\ndata: {}\n\n",
                serde_json::json!({"type":"content_block_stop","index":i})
            ));
        } else if btype == "tool_use" {
            let tc_id = block.get("id").and_then(Value::as_str).unwrap_or("tc_1");
            let name = block.get("name").and_then(Value::as_str).unwrap_or("");
            let input = block.get("input").cloned().unwrap_or(serde_json::json!({}));
            sse.push_str(&format!(
                "event: content_block_start\ndata: {}\n\n",
                serde_json::json!({"type":"content_block_start","index":i,"content_block":{"type":"tool_use","id":tc_id,"name":name,"input":{}}})
            ));
            let args_str = serde_json::to_string(&input).unwrap_or_default();
            if !args_str.is_empty() && args_str != "{}" {
                sse.push_str(&format!(
                    "event: content_block_delta\ndata: {}\n\n",
                    serde_json::json!({"type":"content_block_delta","index":i,"delta":{"type":"input_json_delta","partial_json":args_str}})
                ));
            }
            sse.push_str(&format!(
                "event: content_block_stop\ndata: {}\n\n",
                serde_json::json!({"type":"content_block_stop","index":i})
            ));
        }
    }

    sse.push_str(&format!(
        "event: message_delta\ndata: {}\n\n",
        serde_json::json!({"type":"message_delta","delta":{"stop_reason":stop_reason},"usage":{"output_tokens":usage.get("output_tokens").and_then(Value::as_u64).unwrap_or(0)}})
    ));

    sse.push_str("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n");

    sse
}
