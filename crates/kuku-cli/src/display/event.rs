use kuku::event::{EventPayload, StoredEvent};

pub fn render_event_brief(event: &StoredEvent, verbose: u8) -> String {
    let mut line = format!("evt:{} | {}", event.id, event.payload.type_name());
    let details = event_details(&event.payload, verbose > 0);
    if !details.is_empty() {
        line.push_str(" | ");
        line.push_str(&details);
    }
    if verbose >= 2 {
        if let EventPayload::ContextPrelude { messages, .. } = &event.payload {
            line.push('\n');
            line.push_str(&render_prelude(messages));
        }
    }
    line
}

fn render_prelude(messages: &[kuku::event::types::ContextMessage]) -> String {
    let mut out = String::new();
    out.push_str("    -- prelude -------------------------\n");
    for msg in messages {
        for line in msg.content.lines() {
            out.push_str("    ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn event_details(payload: &EventPayload, verbose: bool) -> String {
    match payload {
        EventPayload::UserInput { text, .. } => text.chars().take(60).collect(),
        EventPayload::ContextSources {
            request_id,
            project_instruction_sources,
            memory_sources,
            ..
        } => {
            if verbose {
                format!(
                    "req={request_id}  project={}  memory={}",
                    project_instruction_sources.len(),
                    memory_sources.len()
                )
            } else {
                format!(
                    "project={}  memory={}",
                    project_instruction_sources.len(),
                    memory_sources.len()
                )
            }
        }
        EventPayload::ModelResponse { text, .. } => {
            let preview: String = text.chars().take(60).collect();
            preview
        }
        EventPayload::ContextSkills {
            registry,
            bootstrap_loaded,
            ..
        } => {
            if verbose {
                format!(
                    "skills={}  bootstrap_loaded={}  hash={}",
                    registry.len(),
                    bootstrap_loaded.len(),
                    registry.hash()
                )
            } else {
                format!(
                    "skills={}  bootstrap_loaded={}",
                    registry.len(),
                    bootstrap_loaded.len()
                )
            }
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
        EventPayload::PermissionRequested {
            tool,
            risk,
            candidate,
            source,
            ..
        } => {
            if verbose {
                format!("request  {tool}  {risk}  {candidate}  source={source}")
            } else {
                format!("request  {tool}  {risk}  {candidate}")
            }
        }
        EventPayload::PermissionAllow {
            tool,
            scope,
            matcher,
            ..
        } => {
            format!("allow  {tool}  {scope}  {matcher}")
        }
        EventPayload::PermissionDeny {
            tool,
            reason,
            source,
            ..
        } => {
            if verbose {
                format!("deny  {tool}  {source}  {reason}")
            } else {
                format!("deny  {tool}  {reason}")
            }
        }
        EventPayload::Handoff {
            summary,
            keep_turns,
            ..
        } => {
            let preview: String = summary.chars().take(60).collect();
            if verbose {
                format!("handoff  keep_turns={keep_turns}  {preview}")
            } else {
                format!("handoff  {preview}")
            }
        }
        _ => String::new(),
    }
}

pub fn derive_final_output(events: &[StoredEvent]) -> Option<String> {
    let filtered = kuku::context::revert::filter_rolled_back_events(events);
    let final_turn = filtered
        .iter()
        .rev()
        .find_map(|event| match &event.payload {
            EventPayload::TurnEnd { turn, .. } => Some(*turn),
            _ => None,
        })?;

    filtered
        .iter()
        .rev()
        .find_map(|event| match &event.payload {
            EventPayload::ModelResponse { turn, text, .. } if *turn == final_turn => {
                Some(text.clone())
            }
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::{derive_final_output, render_event_brief};
    use kuku::event::{EventPayload, StoredEvent};

    #[test]
    fn derive_final_output_uses_last_model_response_before_turn_end() {
        let events = vec![
            StoredEvent {
                id: 1,
                payload: EventPayload::TurnStart {
                    turn: 1,
                    ts: "t0".to_string(),
                },
            },
            StoredEvent {
                id: 2,
                payload: EventPayload::ModelResponse {
                    turn: 1,
                    ts: "t1".to_string(),
                    request_id: "req_1".to_string(),
                    text: "tool phase".to_string(),
                    thinking: None,
                    input_tokens_total: Some(5),
                },
            },
            StoredEvent {
                id: 2,
                payload: EventPayload::ToolCall {
                    turn: 1,
                    ts: "t2".to_string(),
                    tool_call_id: "tool_1".to_string(),
                    request_id: "req_1".to_string(),
                    index: 0,
                    tool: "read_file".to_string(),
                    args: serde_json::json!({"path": "README.md"}),
                },
            },
            StoredEvent {
                id: 3,
                payload: EventPayload::ModelResponse {
                    turn: 1,
                    ts: "t3".to_string(),
                    request_id: "req_2".to_string(),
                    text: "final answer".to_string(),
                    thinking: None,
                    input_tokens_total: Some(7),
                },
            },
            StoredEvent {
                id: 4,
                payload: EventPayload::TurnEnd {
                    turn: 1,
                    ts: "t4".to_string(),
                },
            },
        ];

        assert_eq!(
            derive_final_output(&events).as_deref(),
            Some("final answer")
        );
    }

    #[test]
    fn render_event_brief_supports_fact_only_permission_and_handoff_events() {
        let allow = StoredEvent {
            id: 1,
            payload: EventPayload::PermissionAllow {
                turn: 1,
                ts: "t".to_string(),
                tool_call_id: "tool_1".to_string(),
                tool: "run_command".to_string(),
                scope: "session".to_string(),
                matcher: "run_command(cargo test *)".to_string(),
                source: "host".to_string(),
            },
        };
        let handoff = StoredEvent {
            id: 2,
            payload: EventPayload::Handoff {
                turn: 2,
                ts: "t".to_string(),
                request_id: "req_2".to_string(),
                summary: "carry forward".to_string(),
                keep_turns: 2,
            },
        };

        assert!(render_event_brief(&allow, 1).contains("allow  run_command  session"));
        assert!(render_event_brief(&handoff, 1).contains("keep_turns=2"));
    }

    #[test]
    fn derive_final_output_skips_rolled_back_answers() {
        let events = vec![
            StoredEvent {
                id: 1,
                payload: EventPayload::ModelResponse {
                    turn: 1,
                    ts: "t1".to_string(),
                    request_id: "req_1".to_string(),
                    text: "keep me".to_string(),
                    thinking: None,
                    input_tokens_total: Some(5),
                },
            },
            StoredEvent {
                id: 3,
                payload: EventPayload::TurnEnd {
                    turn: 1,
                    ts: "t2".to_string(),
                },
            },
            StoredEvent {
                id: 4,
                payload: EventPayload::TurnStart {
                    turn: 2,
                    ts: "t2.5".to_string(),
                },
            },
            StoredEvent {
                id: 5,
                payload: EventPayload::ModelResponse {
                    turn: 2,
                    ts: "t3".to_string(),
                    request_id: "req_2".to_string(),
                    text: "rolled back answer".to_string(),
                    thinking: None,
                    input_tokens_total: Some(7),
                },
            },
            StoredEvent {
                id: 6,
                payload: EventPayload::TurnEnd {
                    turn: 2,
                    ts: "t4".to_string(),
                },
            },
            StoredEvent {
                id: 7,
                payload: EventPayload::TurnRollback {
                    turn: 3,
                    ts: "t5".to_string(),
                    target_turn: 2,
                    scope: kuku::event::RollbackScope::ConversationOnly,
                },
            },
        ];

        assert_eq!(derive_final_output(&events).as_deref(), Some("keep me"));
    }
}
