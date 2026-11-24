use super::{
    ContextCursor, MAX_CACHED_FILE_SNAPSHOTS, MAX_SELECTION_BYTES_FACTOR, project_source,
    read_agent_events, read_frozen_catalog,
};
use crate::context::{CanonicalMessage, MessageBlock, ToolResult};
use crate::conversation::address::ConversationAddress;
use crate::event::{EventPayload, EventStore, StoredEvent};
use serde_json::json;
use sha2::{Digest, Sha256};

fn assert_cursor_accounting(cursor: &ContextCursor) {
    let messages = cursor
        .admitted
        .iter()
        .map(|value| &value.message)
        .collect::<Vec<_>>();
    let expected = messages
        .iter()
        .map(|message| serde_json::to_vec(message).unwrap().len())
        .sum::<usize>()
        + cursor
            .tail
            .iter()
            .chain(&cursor.pending_admissions)
            .map(|event| serde_json::to_vec(event).unwrap().len())
            .sum::<usize>();
    assert_eq!(expected, cursor.retained_bytes());
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&messages).unwrap())
        ),
        cursor.history_prefix_hash(messages.len())
    );
}

fn rendered_provider_body(
    history: Vec<CanonicalMessage>,
    prelude_messages: Vec<CanonicalMessage>,
    handoff_summary: Option<String>,
) -> serde_json::Value {
    let catalog = crate::prompt::builtin_prompt_catalog();
    let request = crate::provider::types::ProviderRequest {
        assembly: crate::context::ContextAssembly {
            system_prompt: String::new(),
            prelude_messages,
            history,
            tools: Vec::new(),
            prompt_asset_sources: Vec::new(),
            project_instruction_sources: Vec::new(),
            memory_sources: Vec::new(),
            runtime_context: None,
            handoff_summary,
        },
        catalog: &catalog,
        current_input: crate::provider::types::CanonicalPromptInput { parts: Vec::new() },
        model: "fixture".into(),
        max_output_tokens: Some(128),
        temperature: None,
        stream: true,
        think_level: crate::config::ThinkLevel::Off,
        thinking: Default::default(),
    };
    crate::provider::openai_compat::render_body(&request)
}

#[test]
fn tool_result_projection_keeps_the_wire_view_in_every_protocol() {
    let catalog = crate::prompt::builtin_prompt_catalog();
    let render = |events: &[StoredEvent]| {
        let (_, history) =
            super::super::replay::rebuild_history_for_provider(events, &ConversationAddress::MAIN);
        let request = crate::provider::types::ProviderRequest {
            assembly: crate::context::ContextAssembly {
                system_prompt: String::new(),
                prelude_messages: Vec::new(),
                history,
                tools: Vec::new(),
                prompt_asset_sources: Vec::new(),
                project_instruction_sources: Vec::new(),
                memory_sources: Vec::new(),
                runtime_context: None,
                handoff_summary: None,
            },
            catalog: &catalog,
            current_input: crate::provider::types::CanonicalPromptInput { parts: Vec::new() },
            model: "fixture".into(),
            max_output_tokens: Some(128),
            temperature: None,
            stream: true,
            think_level: crate::config::ThinkLevel::Off,
            thinking: Default::default(),
        };
        [
            crate::provider::openai_compat::render_body(&request),
            crate::provider::anthropic::render_body(&request),
            crate::provider::openai_responses::render_body(&request),
        ]
    };
    for status in ["ok", "error", "cancelled"] {
        for model_content in ["", "EXACT_MODEL_VIEW"] {
            let events = vec![
                StoredEvent {
                    id: 1,
                    payload: EventPayload::ToolCall {
                        turn: 1,
                        ts: String::new(),
                        conversation: Some("main".into()),
                        tool_call_id: "tool".into(),
                        request_id: "req_1".into(),
                        index: 0,
                        tool: "fixture".into(),
                        args: json!({}),
                    },
                },
                StoredEvent {
                    id: 2,
                    payload: EventPayload::ToolResult {
                        turn: 1,
                        ts: String::new(),
                        conversation: Some("main".into()),
                        tool_call_id: "tool".into(),
                        status: status.into(),
                        summary: "DIAGNOSTIC_SUMMARY".into(),
                        model_content: model_content.into(),
                        truncated: true,
                        files_read: vec!["read".into()],
                        files_changed: vec!["changed".into()],
                        commands_run: vec!["command".into()],
                        memory_changed: Some(json!({"diagnostic":true})),
                        structured: Some(json!({"diagnostic":"x".repeat(65_536)})),
                    },
                },
            ];
            let projected = events
                .iter()
                .cloned()
                .filter_map(|event| project_source(event, "main", Some(1)))
                .collect::<Vec<_>>();
            assert!(matches!(
                &projected[1].payload,
                EventPayload::ToolResult {
                    structured: None,
                    ..
                }
            ));
            let original = render(&events);
            assert_eq!(original, render(&projected));
            assert_eq!(
                model_content,
                original[0]["messages"].as_array().unwrap().last().unwrap()["content"]
            );
            let messages = original[1]["messages"].as_array().unwrap();
            assert_eq!(
                model_content,
                messages.last().unwrap()["content"][0]["content"]
            );
            assert_eq!(
                status != "ok",
                messages.last().unwrap()["content"][0]["is_error"]
            );
            assert_eq!(
                model_content,
                original[2]["input"].as_array().unwrap().last().unwrap()["output"]
            );
        }
    }
}

#[test]
fn rewind_prunes_persisted_admissions_by_canonical_replay_provenance_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.jsonl");
    let mut writer = EventStore::open(&path).unwrap();
    writer
        .append(EventPayload::TurnStarted {
            turn: 1,
            ts: "t".into(),
            conversation: "main".into(),
        })
        .unwrap();
    let catalog = crate::prompt::builtin_prompt_catalog();
    let segment = writer
        .append(EventPayload::Unknown(json!({
            "kind":"runtime.context_segment", "agent":"main", "signature":"fixture",
            "configuration":1, "handoff":0, "system_prompt":"system", "prelude":[],
            "tools":[], "catalog":catalog, "connection":"", "protocol":"", "model":"",
            "endpoint":"", "behavior":"", "identity_slot":1, "workspace":""
        })))
        .unwrap()
        .id;
    writer
        .append(EventPayload::Unknown(json!({
            "kind":"runtime.step_prepared", "agent":"main", "segment":segment,
            "context_head":0, "source_head":0
        })))
        .unwrap();
    writer
        .append(EventPayload::ModelResponse {
            conversation: Some("main".into()),
            turn: 1,
            ts: "t".into(),
            request_id: "first".into(),
            text: "earlier assistant".into(),
            thinking: None,
            stop_reason: None,
            input_tokens_total: None,
            output_tokens_total: None,
        })
        .unwrap();
    let tool_call = writer
        .append(EventPayload::ToolCall {
            conversation: Some("main".into()),
            turn: 1,
            ts: "t".into(),
            tool_call_id: "tool-1".into(),
            request_id: "first".into(),
            index: 0,
            tool: "file_read".into(),
            args: json!({}),
        })
        .unwrap()
        .id;
    let tool_result = writer
        .append(EventPayload::ToolResult {
            conversation: Some("main".into()),
            turn: 1,
            ts: "t".into(),
            tool_call_id: "tool-1".into(),
            status: "ok".into(),
            summary: "earlier tool result".into(),
            model_content: "earlier tool result".into(),
            truncated: false,
            files_read: Vec::new(),
            files_changed: Vec::new(),
            commands_run: Vec::new(),
            memory_changed: None,
            structured: None,
        })
        .unwrap()
        .id;
    let steering = writer
        .append(EventPayload::MessageUser {
            conversation: "main".into(),
            turn: 1,
            ts: "t".into(),
            text: "steering input".into(),
            from: None,
            job_delivery: false,
        })
        .unwrap()
        .id;
    let later_answer = writer
        .append(EventPayload::ModelResponse {
            conversation: Some("main".into()),
            turn: 1,
            ts: "t".into(),
            request_id: "second".into(),
            text: "later answer".into(),
            thinking: None,
            stop_reason: None,
            input_tokens_total: None,
            output_tokens_total: None,
        })
        .unwrap()
        .id;

    let mut cursor = ContextCursor::default();
    cursor.refresh(&path, "main", 1024 * 1024).unwrap();
    let admissions = cursor.history(&ConversationAddress::MAIN);
    assert_eq!(4, admissions.len());

    let source_head = later_answer;
    for (ordinal, admitted) in admissions.iter().enumerate() {
        writer
            .append(EventPayload::Unknown(json!({
                "kind":"runtime.context_admitted", "agent":"main", "segment":segment,
                "ordinal":ordinal, "turn":admitted.turn, "origin":admitted.origin,
                "source_head":source_head, "admitted_at":admitted.admitted_at,
                "message":admitted.message.clone()
            })))
            .unwrap();
    }
    cursor.refresh(&path, "main", 1024 * 1024).unwrap();
    assert_eq!(admissions.len(), cursor.pending_admissions.len());
    assert_cursor_accounting(&cursor);
    writer
        .append(EventPayload::Unknown(json!({
            "kind":"runtime.step_prepared", "agent":"main", "segment":segment,
            "context_head":admissions.len(), "source_head":source_head
        })))
        .unwrap();
    cursor.refresh(&path, "main", 1024 * 1024).unwrap();
    assert_eq!(4, cursor.admitted.len());
    assert_cursor_accounting(&cursor);

    writer
        .append(EventPayload::Unknown(json!({
            "kind":"runtime.operation",
            "record":{"operation":"message_rewind","address":"main","target_event_id":steering}
        })))
        .unwrap();
    cursor.refresh(&path, "main", 1024 * 1024).unwrap();
    drop(writer);

    let assert_provider_body = |cursor: &ContextCursor| {
        assert_cursor_accounting(cursor);
        let history = cursor.history(&ConversationAddress::MAIN);
        let body = serde_json::to_string(&rendered_provider_body(
            history
                .iter()
                .map(|admitted| admitted.message.clone())
                .collect(),
            cursor
                .segment
                .as_ref()
                .map(|segment| segment.prelude.clone())
                .unwrap_or_default(),
            None,
        ))
        .unwrap();
        assert_eq!(2, history.len());
        assert_eq!(
            vec![tool_call, tool_result],
            history
                .iter()
                .map(|item| item.admitted_at)
                .collect::<Vec<_>>()
        );
        assert!(body.contains("earlier assistant"));
        assert!(body.contains("earlier tool result"));
        assert!(!body.contains("steering input"));
        assert!(!body.contains("later answer"));
    };
    assert_provider_body(&cursor);
    assert_eq!(
        vec![tool_call, tool_result, steering, later_answer],
        admissions
            .iter()
            .map(|item| item.admitted_at)
            .collect::<Vec<_>>()
    );

    let mut reopened = ContextCursor::default();
    reopened.refresh(&path, "main", 1024 * 1024).unwrap();
    assert_provider_body(&reopened);
}

#[test]
fn reopened_cursor_drops_rewound_handoff_summary_and_frozen_prelude() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.jsonl");
    let mut writer = EventStore::open(&path).unwrap();
    writer
        .append(EventPayload::TurnStarted {
            turn: 1,
            ts: "t".into(),
            conversation: "main".into(),
        })
        .unwrap();
    let catalog = crate::prompt::builtin_prompt_catalog();
    let prelude =
        CanonicalMessage::assistant(vec![MessageBlock::Text("removed handoff prelude".into())]);
    let segment = writer
        .append(EventPayload::Unknown(json!({
            "kind":"runtime.context_segment", "agent":"main", "signature":"old",
            "configuration":1, "handoff":2, "system_prompt":"system", "prelude":[prelude],
            "tools":[], "catalog":catalog, "connection":"", "protocol":"", "model":"",
            "endpoint":"", "behavior":"", "identity_slot":1, "workspace":""
        })))
        .unwrap()
        .id;
    writer
        .append(EventPayload::Unknown(json!({
            "kind":"runtime.step_prepared", "agent":"main", "segment":segment,
            "context_head":0, "source_head":0
        })))
        .unwrap();
    writer
        .append(EventPayload::Handoff {
            turn: 1,
            ts: "t".into(),
            request_id: "handoff".into(),
            summary: "removed handoff summary".into(),
            keep_turns: 1,
        })
        .unwrap();
    writer
        .append(EventPayload::Unknown(json!({
            "kind":"runtime.operation",
            "record":{"operation":"message_rewind","address":"main","target_event_id":1}
        })))
        .unwrap();

    let mut reopened = ContextCursor::default();
    reopened.refresh(&path, "main", 1024 * 1024).unwrap();
    let events = EventStore::replay(&path).unwrap();
    let (handoff, history) =
        super::super::replay::rebuild_history_for_provider(&events, &ConversationAddress::MAIN);

    assert!(reopened.segment.is_none());
    assert!(reopened.handoff.is_none());
    let handoff = handoff.or_else(|| {
        reopened
            .handoff
            .as_ref()
            .and_then(|event| match &event.payload {
                EventPayload::Handoff { summary, .. } => Some(summary.clone()),
                _ => None,
            })
    });
    let body = serde_json::to_string(&rendered_provider_body(
        history,
        reopened
            .segment
            .as_ref()
            .map(|segment| segment.prelude.clone())
            .unwrap_or_default(),
        handoff,
    ))
    .unwrap();
    assert!(!body.contains("removed handoff"));
}

#[test]
fn reopened_cursor_preserves_handoff_summary_before_rewind_target() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.jsonl");
    let mut writer = EventStore::open(&path).unwrap();
    writer
        .append(EventPayload::TurnStarted {
            turn: 1,
            ts: "t".into(),
            conversation: "main".into(),
        })
        .unwrap();
    let catalog = crate::prompt::builtin_prompt_catalog();
    let frozen_summary = CanonicalMessage::assistant(vec![MessageBlock::Text(
        "surviving turn-one summary".into(),
    )]);
    let segment = writer
        .append(EventPayload::Unknown(json!({
            "kind":"runtime.context_segment", "agent":"main", "signature":"old",
            "configuration":1, "handoff":4, "system_prompt":"system", "prelude":[frozen_summary],
            "tools":[], "catalog":catalog, "connection":"", "protocol":"", "model":"",
            "endpoint":"", "behavior":"", "identity_slot":1, "workspace":""
        })))
        .unwrap()
        .id;
    writer
        .append(EventPayload::Unknown(json!({
            "kind":"runtime.step_prepared", "agent":"main", "segment":segment,
            "context_head":0, "source_head":0
        })))
        .unwrap();
    let handoff = writer
        .append(EventPayload::Handoff {
            turn: 1,
            ts: "t".into(),
            request_id: "handoff".into(),
            summary: "surviving handoff summary".into(),
            keep_turns: 1,
        })
        .unwrap();
    assert_eq!(4, handoff.id);
    writer
        .append(EventPayload::TurnStarted {
            turn: 2,
            ts: "t".into(),
            conversation: "main".into(),
        })
        .unwrap();
    let target = writer
        .append(EventPayload::MessageUser {
            conversation: "main".into(),
            turn: 2,
            ts: "t".into(),
            text: "rewound second-turn input".into(),
            from: None,
            job_delivery: false,
        })
        .unwrap()
        .id;
    writer
        .append(EventPayload::ModelResponse {
            conversation: Some("main".into()),
            turn: 2,
            ts: "t".into(),
            request_id: "later".into(),
            text: "rewound second-turn answer".into(),
            thinking: None,
            stop_reason: None,
            input_tokens_total: None,
            output_tokens_total: None,
        })
        .unwrap();
    writer
        .append(EventPayload::Unknown(json!({
            "kind":"runtime.operation",
            "record":{"operation":"message_rewind","address":"main","target_event_id":target}
        })))
        .unwrap();
    drop(writer);

    let mut reopened = ContextCursor::default();
    reopened.refresh(&path, "main", 1024 * 1024).unwrap();
    let handoff_summary = reopened
        .handoff
        .as_ref()
        .and_then(|event| match &event.payload {
            EventPayload::Handoff { summary, .. } => Some(summary.clone()),
            _ => None,
        });
    let history = reopened.history(&ConversationAddress::MAIN);
    let body = serde_json::to_string(&rendered_provider_body(
        history
            .iter()
            .map(|admitted| admitted.message.clone())
            .collect(),
        reopened
            .segment
            .as_ref()
            .map(|segment| segment.prelude.clone())
            .unwrap_or_default(),
        handoff_summary,
    ))
    .unwrap();

    assert_eq!(Some(4), reopened.handoff.as_ref().map(|event| event.id));
    assert_eq!(
        Some(segment),
        reopened.segment.as_ref().map(|segment| segment.id)
    );
    assert!(body.contains("surviving turn-one summary"));
    assert!(body.contains("surviving handoff summary"));
    assert!(!body.contains("rewound second-turn input"));
    assert!(!body.contains("rewound second-turn answer"));
}

#[test]
fn frozen_assets_are_selected_by_agent_across_bounded_pages() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.jsonl");
    let mut writer = EventStore::open(&path).unwrap();
    let mut original = crate::prompt::builtin_prompt_catalog();
    original.system.text = "original main system".into();
    let prepared = writer
        .append(EventPayload::Unknown(
            json!({"kind":"runtime.context_segment","agent":"main","catalog":original}),
        ))
        .unwrap();
    writer
        .append(EventPayload::Unknown(
            json!({"kind":"runtime.step_prepared","agent":"main","segment":prepared.id}),
        ))
        .unwrap();
    for index in 0..300 {
        writer.append(EventPayload::Unknown(json!({"kind":"fixture.other_agent_output","agent":"reviewer","index":index,"output":"x".repeat(4096)}))).unwrap();
    }
    let mut unrelated = original.clone();
    unrelated.system.text = "other agent system".into();
    let prepared = writer
        .append(EventPayload::Unknown(
            json!({"kind":"runtime.context_segment","agent":"reviewer","catalog":unrelated}),
        ))
        .unwrap();
    writer
        .append(EventPayload::Unknown(
            json!({"kind":"runtime.step_prepared","agent":"reviewer","segment":prepared.id}),
        ))
        .unwrap();
    writer.append(EventPayload::Unknown(json!({"kind":"runtime.context_segment","agent":"main","catalog":"unprepared invalid catalog"}))).unwrap();
    assert_eq!(Some(original), read_frozen_catalog(&path, "main").unwrap());
    assert_eq!(None, read_frozen_catalog(&path, "missing").unwrap());
}

fn segment(agent: &str, system: &str) -> EventPayload {
    EventPayload::Unknown(
        json!({"kind":"runtime.context_segment","agent":agent,"signature":"fixture","configuration":0,"handoff":0,
        "system_prompt":system,"prelude":[],"tools":[],"catalog":crate::prompt::builtin_prompt_catalog()}),
    )
}

#[test]
fn prepared_boundary_requires_the_matching_complete_admission_batch() {
    for (head, source_head, succeeds) in [(2, 1, true), (1, 1, false), (2, 2, false)] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("events.jsonl");
        let mut writer = EventStore::open(&path).unwrap();
        let segment = writer.append(segment("main", "system")).unwrap();
        for ordinal in 0..2 {
            writer.append(EventPayload::Unknown(json!({"kind":"runtime.context_admitted", "agent":"main", "segment":segment.id,
                "ordinal":ordinal, "turn":1, "source_head":1, "message":CanonicalMessage::user_text(format!("input-{ordinal}"))}))).unwrap();
        }
        writer
            .append(EventPayload::Unknown(
                json!({"kind":"runtime.step_prepared", "agent":"main", "segment":segment.id,
            "context_head":head, "source_head":source_head}),
            ))
            .unwrap();
        let mut cursor = ContextCursor::default();
        assert_eq!(succeeds, cursor.refresh(&path, "main", 4096).is_ok());
        if succeeds {
            assert_eq!(2, cursor.admitted.len());
        }
    }
}

#[test]
fn active_cursor_selects_one_prepared_segment_and_incrementally_reads_new_facts() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.jsonl");
    let mut writer = EventStore::open(&path).unwrap();
    writer
        .append(EventPayload::TurnStarted {
            ts: "ts".into(),
            conversation: "reviewer".into(),
            turn: 1,
        })
        .unwrap();
    let input = writer
        .append(EventPayload::MessageUser {
            ts: "ts".into(),
            conversation: "reviewer".into(),
            turn: 1,
            text: "input".into(),
            from: None,
            job_delivery: false,
        })
        .unwrap();
    let first = writer
        .append(segment("reviewer", "committed system"))
        .unwrap();
    writer.append(EventPayload::Unknown(json!({"kind":"runtime.context_admitted","agent":"reviewer","segment":first.id,"ordinal":0,"turn":1,"origin":"input","message":CanonicalMessage::user_text("input")}))).unwrap();
    writer.append(EventPayload::Unknown(json!({"kind":"runtime.step_prepared","agent":"reviewer","segment":first.id,"context_head":1,"source_head":input.id}))).unwrap();
    writer
        .append(EventPayload::TurnStarted {
            ts: "ts".into(),
            conversation: "noisy".into(),
            turn: 2,
        })
        .unwrap();
    for _ in 0..40 {
        writer
            .append(EventPayload::ModelResponse {
                ts: "ts".into(),
                conversation: Some("noisy".into()),
                turn: 2,
                request_id: "noise".into(),
                text: "x".repeat(64 * 1024),
                thinking: None,
                stop_reason: Some(crate::event::ModelStopReason::EndTurn),
                input_tokens_total: None,
                output_tokens_total: None,
            })
            .unwrap();
    }
    writer
        .append(EventPayload::ModelResponse {
            ts: "ts".into(),
            conversation: Some("reviewer".into()),
            turn: 1,
            request_id: "reply".into(),
            text: "done".into(),
            thinking: None,
            stop_reason: Some(crate::event::ModelStopReason::EndTurn),
            input_tokens_total: Some(100),
            output_tokens_total: Some(1),
        })
        .unwrap();
    let orphan = writer
        .append(segment("reviewer", "unprepared replacement"))
        .unwrap();
    writer.append(EventPayload::Unknown(json!({"kind":"runtime.context_admitted","agent":"reviewer","segment":orphan.id,"ordinal":0,"turn":1,"origin":"input","message":CanonicalMessage::user_text("unprepared view")}))).unwrap();
    let mut cursor = ContextCursor::default();
    cursor.refresh(&path, "reviewer", 16 * 1024).unwrap();
    assert_eq!(
        Some(first.id),
        cursor.segment.as_ref().map(|segment| segment.id)
    );
    assert!(cursor.retained_bytes() < 4096);
    let conversation = ConversationAddress::parse("reviewer").unwrap();
    let history = cursor.history(&conversation);
    assert_eq!(2, history.len());
    assert_eq!(
        CanonicalMessage::assistant(vec![MessageBlock::Text("done".into())]),
        history[1].message
    );
    let read = cursor.events_read;
    writer
        .append(EventPayload::MessageAssistant {
            ts: "ts".into(),
            conversation: "reviewer".into(),
            turn: 1,
            message_id: "reply".into(),
            text: "done".into(),
        })
        .unwrap();
    writer
        .append(EventPayload::MessageAssistant {
            ts: "ts".into(),
            conversation: "reviewer".into(),
            turn: 1,
            message_id: "independent".into(),
            text: "independent notice".into(),
        })
        .unwrap();
    cursor.refresh(&path, "reviewer", 16 * 1024).unwrap();
    assert_eq!(2, cursor.events_read - read);
    assert_eq!(3, cursor.history(&conversation).len());
    assert_cursor_accounting(&cursor);
    let read = cursor.events_read;
    cursor.refresh(&path, "reviewer", 16 * 1024).unwrap();
    assert_eq!(read, cursor.events_read);
}

#[test]
fn an_orphan_segment_does_not_hide_a_later_prepared_head_of_the_active_segment() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.jsonl");
    let mut writer = EventStore::open(&path).unwrap();
    writer
        .append(EventPayload::TurnStarted {
            ts: "ts".into(),
            conversation: "main".into(),
            turn: 1,
        })
        .unwrap();
    let first = writer.append(segment("main", "prepared system")).unwrap();
    writer.append(EventPayload::Unknown(json!({"kind":"runtime.context_admitted","agent":"main","segment":first.id,"ordinal":0,"turn":1,"origin":"input","message":CanonicalMessage::user_text("first input")}))).unwrap();
    writer.append(EventPayload::Unknown(json!({"kind":"runtime.step_prepared","agent":"main","segment":first.id,"context_head":1,"source_head":first.id}))).unwrap();
    let reply = writer
        .append(EventPayload::ModelResponse {
            ts: "ts".into(),
            conversation: Some("main".into()),
            turn: 1,
            request_id: "reply".into(),
            text: "accepted response".into(),
            thinking: None,
            stop_reason: Some(crate::event::ModelStopReason::EndTurn),
            input_tokens_total: Some(100),
            output_tokens_total: Some(1),
        })
        .unwrap();
    let orphan = writer.append(segment("main", "unprepared system")).unwrap();
    writer.append(EventPayload::Unknown(json!({"kind":"runtime.context_admitted","agent":"main","segment":orphan.id,"ordinal":0,"turn":1,"origin":"input","message":CanonicalMessage::user_text("orphan input")}))).unwrap();
    writer.append(EventPayload::Unknown(json!({"kind":"runtime.context_admitted","agent":"main","segment":first.id,"ordinal":1,"turn":1,"origin":"response","message":CanonicalMessage::assistant(vec![MessageBlock::Text("accepted response".into())])}))).unwrap();
    writer.append(EventPayload::Unknown(json!({"kind":"runtime.step_prepared","agent":"main","segment":first.id,"context_head":2,"source_head":reply.id}))).unwrap();
    let mut cursor = ContextCursor::default();
    cursor.refresh(&path, "main", 16 * 1024).unwrap();
    assert_eq!(
        Some(first.id),
        cursor.segment.as_ref().map(|segment| segment.id)
    );
    assert_eq!(2, cursor.admitted.len());
    assert_eq!(
        CanonicalMessage::assistant(vec![MessageBlock::Text("accepted response".into())]),
        cursor.history(&ConversationAddress::MAIN)[1].message
    );
}
fn append_read(writer: &mut EventStore, turn: u64, file: u64, cached: bool) {
    writer
        .append(EventPayload::TurnStarted {
            ts: "ts".into(),
            conversation: "main".into(),
            turn,
        })
        .unwrap();
    writer.append(EventPayload::ToolResult { ts: "ts".into(), conversation: Some("main".into()), turn, tool_call_id: format!("read-{turn}"), status: "ok".into(), summary: "read".into(), model_content: "model view".into(), truncated: false,
        files_read: Vec::new(), files_changed: Vec::new(), commands_run: Vec::new(), memory_changed: None,
        structured: Some(json!({"kind":"file_content","canonical_path":format!("/fixture/{file}.txt"),"start_line":1,"raw_text":"x".repeat(4096),"cached":cached})) }).unwrap();
}

#[test]
fn startup_keeps_request_counters_and_pending_arguments_without_response_bodies() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.jsonl");
    let mut writer = EventStore::open(&path).unwrap();
    writer
        .append(EventPayload::TurnStarted {
            turn: 1,
            ts: String::new(),
            conversation: "reviewer".into(),
        })
        .unwrap();
    writer
        .append(EventPayload::ModelResponse {
            conversation: None,
            turn: 1,
            ts: String::new(),
            request_id: "req_3".into(),
            text: "x".repeat(65_536),
            thinking: Some("y".repeat(65_536)),
            stop_reason: Some(crate::event::ModelStopReason::ToolUse),
            input_tokens_total: Some(100),
            output_tokens_total: Some(100),
        })
        .unwrap();
    writer
        .append(EventPayload::ToolCall {
            turn: 1,
            ts: String::new(),
            conversation: None,
            tool_call_id: "pending".into(),
            request_id: "req_3".into(),
            index: 0,
            tool: "run_command".into(),
            args: json!({"program":"echo","args":["exact"],"cwd":".","timeout":5}),
        })
        .unwrap();
    writer
        .append(EventPayload::TurnStarted {
            turn: 2,
            ts: String::new(),
            conversation: "main".into(),
        })
        .unwrap();
    writer
        .append(EventPayload::ModelResponse {
            conversation: Some("main".into()),
            turn: 2,
            ts: String::new(),
            request_id: "req_9".into(),
            text: "z".repeat(65_536),
            thinking: None,
            stop_reason: None,
            input_tokens_total: None,
            output_tokens_total: None,
        })
        .unwrap();
    let events = read_agent_events(&path, "reviewer", 1024).unwrap();
    assert_eq!(3, events.len());
    assert!(events.iter().any(|event| matches!(&event.payload,
        EventPayload::ModelResponse { conversation: Some(agent), request_id, text, thinking, .. }
        if agent == "reviewer" && request_id == "req_3" && text.is_empty() && thinking.is_none()
    )));
    assert!(events.iter().any(|event| matches!(&event.payload,
        EventPayload::ToolCall { conversation: Some(agent), args, .. }
        if agent == "reviewer" && args["program"] == "echo" && args["args"] == json!(["exact"])
    )));
}

#[test]
fn startup_read_snapshots_follow_both_byte_and_recent_access_limits() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.jsonl");
    let mut writer = EventStore::open(&path).unwrap();
    for file in 0..130 {
        append_read(&mut writer, file + 1, file, false);
    }
    let generous = read_agent_events(&path, "main", 1024 * 1024).unwrap();
    let snapshots = |events: &[StoredEvent]| {
        events
            .iter()
            .filter_map(|event| match &event.payload {
                EventPayload::ToolResult {
                    structured: Some(value),
                    ..
                } if value["kind"] == "file_content" => {
                    value["canonical_path"].as_str().map(str::to_string)
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(MAX_CACHED_FILE_SNAPSHOTS, snapshots(&generous).len());
    append_read(&mut writer, 131, 127, true);
    append_read(&mut writer, 132, 130, false);
    let bounded = read_agent_events(&path, "main", 4096).unwrap();
    assert!(serde_json::to_vec(&bounded).unwrap().len() <= 4096 * MAX_SELECTION_BYTES_FACTOR);
    let paths = snapshots(&bounded);
    assert!(paths.contains(&"/fixture/127.txt".into()));
    assert!(paths.contains(&"/fixture/130.txt".into()));
    assert!(!paths.contains(&"/fixture/128.txt".into()));
}

#[test]
fn startup_keeps_only_write_confirmations_in_the_prepared_context() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.jsonl");
    let mut writer = EventStore::open(&path).unwrap();
    append_read(&mut writer, 1, 0, false);
    let turn = 1;
    let mut source_head = 0;
    for file in ["forgotten", "retained", "unconfirmed"] {
        source_head = writer.append(EventPayload::ToolResult {
            ts: String::new(), conversation: Some("main".into()), turn, tool_call_id: file.into(),
            status: "ok".into(), summary: "written".into(), model_content: "confirmation".into(), truncated: false,
            files_read: vec![], files_changed: vec![], commands_run: vec![], memory_changed: None,
            structured: Some(json!({"kind":"file_write","canonical_path":file,"commit_state":"committed",
                "supplied_full_write":true,"content_hash_after":"written","raw_text_after":"x".repeat(16_384)})),
        }).unwrap().id;
    }
    let start_segment = |writer: &mut EventStore| {
        writer
            .append(EventPayload::Unknown(json!({
                "kind":"runtime.context_segment", "agent":"main"
            })))
            .unwrap()
            .id
    };
    let admit = |writer: &mut EventStore, segment: u64, files: &[&str]| {
        let blocks = files
            .iter()
            .map(|file| {
                MessageBlock::ToolResult(ToolResult {
                    tool_call_id: (*file).into(),
                    status: "ok".into(),
                    summary: "written".into(),
                    model_content: "confirmation".into(),
                    structured: None,
                    truncated: false,
                })
            })
            .collect();
        writer
            .append(EventPayload::Unknown(json!({
                "kind":"runtime.context_admitted", "agent":"main", "segment":segment,
                "ordinal":0, "turn":turn, "origin":"tool_results", "source_head":source_head,
                "message": CanonicalMessage::user(blocks),
            })))
            .unwrap();
    };
    let prepare = |writer: &mut EventStore, segment: u64| {
        writer
            .append(EventPayload::Unknown(json!({
                "kind":"runtime.step_prepared", "agent":"main", "segment":segment,
                "context_head":1, "source_head":source_head,
            })))
            .unwrap();
    };
    let paths = || -> Vec<String> {
        let events = read_agent_events(&path, "main", 4096).unwrap();
        assert!(events.iter().any(|event| matches!(&event.payload,
            EventPayload::ToolResult { structured: Some(value), .. } if value["kind"] == "file_content"
        )));
        events
            .into_iter()
            .filter_map(|event| {
                let EventPayload::ToolResult {
                    structured: Some(value),
                    ..
                } = event.payload
                else {
                    return None;
                };
                let (path, _) = super::super::replay::supplied_file_version(&value)?;
                assert!(value.get("raw_text_after").is_none());
                Some(path.to_owned())
            })
            .collect()
    };
    let segment = start_segment(&mut writer);
    admit(&mut writer, segment, &["forgotten", "retained"]);
    assert!(paths().is_empty());
    prepare(&mut writer, segment);
    assert_eq!(["forgotten", "retained"], paths().as_slice());

    let successor = start_segment(&mut writer);
    admit(&mut writer, successor, &["retained"]);
    assert_eq!(["forgotten", "retained"], paths().as_slice());
    prepare(&mut writer, successor);
    assert_eq!(["retained"], paths().as_slice());
}
