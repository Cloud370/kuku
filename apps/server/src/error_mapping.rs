pub fn error_response(err: &kuku::Error) -> (String, String) {
    (err.code().to_string(), err.to_string())
}

pub fn error_envelope(err: &kuku::Error) -> serde_json::Value {
    serde_json::json!({
        "ok": false,
        "code": err.code(),
        "message": err.to_string(),
    })
}

pub fn ndjson_error(err: &kuku::Error) -> String {
    let value = serde_json::json!({
        "ok": false,
        "type": "error",
        "code": err.code(),
        "message": err.to_string(),
    });
    format!("{}\n", value)
}
