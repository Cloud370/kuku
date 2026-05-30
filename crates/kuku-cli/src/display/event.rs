use kuku::event::{EventPayload, StoredEvent};

pub fn render_event_brief(event: &StoredEvent, verbose: u8) -> String {
    let mut line = format!("evt:{} | {}", event.id, event.payload.type_name());
    let details = event_details(&event.payload, verbose > 0);
    if !details.is_empty() {
        line.push_str(" | ");
        line.push_str(&details);
    }
    if verbose >= 2 {
        if let EventPayload::ModelRequest {
            context: Some(ctx), ..
        } = &event.payload
        {
            line.push('\n');
            line.push_str(&render_context(ctx));
        }
    }
    line
}

fn render_context(ctx: &kuku::event::types::RequestContext) -> String {
    let mut out = String::new();
    out.push_str("    -- context -------------------------\n");

    out.push_str("    [system]\n");
    for line in ctx.system.lines() {
        out.push_str("    ");
        out.push_str(line);
        out.push('\n');
    }

    if let Some(prelude) = &ctx.prelude {
        out.push_str("\n    [prelude]\n");
        for msg in prelude {
            for line in msg.content.lines() {
                out.push_str("    ");
                out.push_str(line);
                out.push('\n');
            }
        }
    }

    if !ctx.notices.is_empty() {
        out.push_str("\n    [notices]\n");
        for msg in &ctx.notices {
            for line in msg.content.lines() {
                out.push_str("    ");
                out.push_str(line);
                out.push('\n');
            }
        }
    }

    out
}

fn event_details(payload: &EventPayload, verbose: bool) -> String {
    match payload {
        EventPayload::UserInput { text, .. } => text.chars().take(60).collect(),
        EventPayload::ModelResponse {
            text, stop_reason, ..
        } => {
            let preview: String = text.chars().take(60).collect();
            format!("{preview}  stop={stop_reason}")
        }
        EventPayload::ToolCall {
            tool,
            args,
            tool_call_id,
            ..
        } => {
            let path_or_cmd = args
                .get("path")
                .or_else(|| args.get("command"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if verbose {
                format!("{tool} {path_or_cmd}  id={tool_call_id}")
            } else {
                format!("{tool} {path_or_cmd}")
            }
        }
        EventPayload::ToolResult {
            tool_call_id,
            status,
            summary,
            ..
        } => {
            if verbose {
                format!("{status}  {summary}  id={tool_call_id}")
            } else {
                format!("{status}  {summary}")
            }
        }
        EventPayload::PermissionDecision { decision, rule, .. } => {
            format!("{decision}  {rule}")
        }
        EventPayload::Handoff {
            summary,
            kept_turns,
            ..
        } => {
            let preview: String = summary.chars().take(60).collect();
            if verbose {
                format!("handoff  kept_turns={kept_turns}  {preview}")
            } else {
                format!("handoff  {preview}")
            }
        }
        _ => String::new(),
    }
}

pub fn derive_final_output(events: &[StoredEvent]) -> Option<String> {
    events.iter().rev().find_map(|event| match &event.payload {
        EventPayload::ModelResponse {
            stop_reason, text, ..
        } if stop_reason == "end_turn" => Some(text.clone()),
        _ => None,
    })
}
