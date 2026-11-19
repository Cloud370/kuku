use kuku_client::{HistoryEntry, HistoryKind, HistoryPage};
use serde_json::json;

use super::{HistoryRequest, Key, Record, Transcript, MAX_RECORDS};

fn retry_entry(
    id: u64,
    turn: u64,
    request_id: &str,
    message: &str,
    stop_reason: &str,
) -> HistoryEntry {
    HistoryEntry {
        id,
        agent: "main".into(),
        turn: Some(turn),
        kind: HistoryKind::Retry,
        text: None,
        turn_summary: None,
        details: [(
            "retry".into(),
            json!({
                "source":"runtime.model_retry",
                "phase":"stopped",
                "attempt":2,
                "max_attempts":3,
                "request_id":request_id,
                "failed_kind":"transport",
                "status":502,
                "will_retry":false,
                "stop_reason":stop_reason,
                "message":message
            }),
        )]
        .into_iter()
        .collect(),
    }
}

fn error_entry(id: u64, turn: u64, request_id: &str, message: &str) -> HistoryEntry {
    HistoryEntry {
        id,
        agent: "main".into(),
        turn: Some(turn),
        kind: HistoryKind::Error,
        text: Some(message.into()),
        turn_summary: None,
        details: [
            ("code".into(), json!("transport")),
            ("request_id".into(), json!(request_id)),
        ]
        .into_iter()
        .collect(),
    }
}

fn interruption_entry(
    id: u64,
    turn: u64,
    execution_id: Option<u64>,
    message: &str,
) -> HistoryEntry {
    let mut entry = json!({
        "id":id,
        "agent":"main",
        "turn":turn,
        "type":"status",
        "status":"interrupted",
        "text":message
    });
    if let Some(execution_id) = execution_id {
        entry["execution_id"] = json!(execution_id);
    }
    serde_json::from_value(entry).expect("interruption entry")
}

fn status_entry(id: u64) -> HistoryEntry {
    HistoryEntry {
        id,
        agent: "main".into(),
        turn: Some(2),
        kind: HistoryKind::Status,
        text: Some("Quiet status".into()),
        turn_summary: None,
        details: Default::default(),
    }
}

fn rendered(transcript: &mut Transcript) -> String {
    rendered_at_height(transcript, 20)
}

fn rendered_at_height(transcript: &mut Transcript, height: usize) -> String {
    transcript
        .project(120, height)
        .lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn consecutive_retry_history_entries_share_one_heading_and_one_line_each() {
    let mut transcript = Transcript::default();
    for (id, phase, attempt) in [(8, "waiting", 2), (9, "started", 2), (10, "stopped", 2)] {
        transcript.apply(
            HistoryEntry {
                id,
                agent: "main".into(),
                turn: Some(2),
                kind: HistoryKind::Retry,
                text: None,
                turn_summary: None,
                details: [(
                    "retry".into(),
                    json!({
                        "source":"runtime.model_retry",
                        "phase":phase,
                        "attempt":attempt,
                        "max_attempts":3,
                        "failed_kind":"transport",
                        "will_retry":phase!="stopped",
                        "stop_reason":if phase=="stopped" {"attempt_limit"} else {"unknown"}
                    }),
                )]
                .into_iter()
                .collect(),
            },
            None,
        );
    }

    let lines = transcript
        .project(120, 20)
        .lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let text = lines.join("\n");
    assert_eq!(1, text.matches("Retry history").count());
    let heading = lines
        .iter()
        .position(|line| line.contains("Retry history"))
        .unwrap();
    let events = lines
        .iter()
        .skip(heading + 1)
        .take_while(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    assert_eq!(3, events.len(), "each retry decision should occupy one row");
    let scheduled = text.find("Retry scheduled").unwrap();
    let started = text.find("Retry request started").unwrap();
    let stopped = text.find("attempt limit reached").unwrap();
    assert!(scheduled < started && started < stopped);
}

#[test]
fn correlated_retry_message_equal_to_diagnostic_has_no_fallback_hint() {
    let mut transcript = Transcript::default();
    transcript.apply(
        retry_entry(8, 2, "req_1", "Fixture connection closed", "attempt_limit"),
        None,
    );
    transcript.apply(
        error_entry(9, 2, "req_1", "Fixture connection closed"),
        None,
    );

    let text = rendered(&mut transcript);
    assert_eq!(1, text.matches("Fixture connection closed").count());
    assert_eq!(1, text.matches("HTTP 502").count());
    assert_eq!(1, text.matches("attempt limit reached").count());
    assert!(!text.contains("Wait/check service/network; then Continue."));
}

#[test]
fn correlated_retry_without_message_keeps_diagnostic_and_code_guidance() {
    let mut transcript = Transcript::default();
    transcript.apply(retry_entry(8, 2, "req_1", "", "attempt_limit"), None);
    transcript.apply(error_entry(9, 2, "req_1", "Connection reset by peer"), None);

    let text = rendered(&mut transcript);
    assert_eq!(1, text.matches("Connection reset by peer").count());
    assert_eq!(1, text.matches("HTTP 502").count());
    assert_eq!(1, text.matches("Transport error").count());
    assert_eq!(
        1,
        text.matches("Wait/check service/network; then Continue.")
            .count()
    );
}

#[test]
fn correlated_retry_keeps_distinct_diagnostic_and_user_safe_explanation_once() {
    let mut transcript = Transcript::default();
    transcript.apply(
        retry_entry(
            8,
            2,
            "req_1",
            "Check the provider network, then Continue.",
            "attempt_limit",
        ),
        None,
    );
    transcript.apply(error_entry(9, 2, "req_1", "Connection reset by peer"), None);

    let text = rendered(&mut transcript);
    assert_eq!(1, text.matches("Connection reset by peer").count());
    assert_eq!(
        1,
        text.matches("Check the provider network, then Continue.")
            .count()
    );
    assert_eq!(1, text.matches("HTTP 502").count());
    assert!(!text.contains("Wait/check service/network; then Continue."));
}

#[test]
fn matching_error_text_without_matching_request_id_is_not_deduplicated() {
    let mut transcript = Transcript::default();
    transcript.apply(
        retry_entry(
            8,
            2,
            "req_retry",
            "Fixture connection closed",
            "attempt_limit",
        ),
        None,
    );
    transcript.apply(
        error_entry(9, 2, "req_error", "Fixture connection closed"),
        None,
    );

    let text = rendered(&mut transcript);
    assert_eq!(2, text.matches("Fixture connection closed").count());
}

#[test]
fn retry_error_correlation_requires_same_agent_and_turn() {
    let retry_message = "Check the provider network, then Continue.";
    let different_turn = error_entry(9, 3, "req_1", "Connection reset by peer");
    let mut different_agent = error_entry(9, 2, "req_1", "Connection reset by peer");
    different_agent.agent = "worker".into();

    for error in [different_turn, different_agent] {
        let mut transcript = Transcript::default();
        transcript.apply(
            retry_entry(8, 2, "req_1", retry_message, "attempt_limit"),
            None,
        );
        transcript.apply(error, None);

        let text = rendered(&mut transcript);
        assert!(text.contains(retry_message), "{text}");
        assert!(
            text.contains("Wait/check service/network; then Continue."),
            "{text}"
        );
    }
}

#[test]
fn retry_detail_and_error_fallback_restore_when_counterpart_leaves_window() {
    let retry_message = "Check the provider network, then Continue.";
    let mut transcript = Transcript::default();
    transcript.apply(
        retry_entry(8, 2, "req_1", retry_message, "attempt_limit"),
        None,
    );
    transcript.apply(error_entry(9, 2, "req_1", "Connection reset by peer"), None);

    let correlated = rendered(&mut transcript);
    assert_eq!(1, correlated.matches(retry_message).count());
    assert!(!correlated.contains("Wait/check service/network; then Continue."));
    transcript.recent.remove(&Key::Event(9));

    let retry_only = rendered(&mut transcript);
    assert_eq!(1, retry_only.matches(retry_message).count());
    assert!(!retry_only.contains("Connection reset by peer"));

    let mut transcript = Transcript::default();
    transcript.apply(
        retry_entry(8, 2, "req_1", retry_message, "attempt_limit"),
        None,
    );
    transcript.apply(error_entry(9, 2, "req_1", "Connection reset by peer"), None);
    transcript.recent.remove(&Key::Event(8));

    let error_only = rendered(&mut transcript);
    assert!(!error_only.contains(retry_message));
    assert!(error_only.contains("Connection reset by peer"));
    assert!(error_only.contains("Wait/check service/network; then Continue."));
}

#[test]
fn retry_detail_restores_when_paging_trims_correlated_error() {
    let retry_message = "Check the provider network, then Continue.";
    let mut transcript = Transcript::default();
    transcript.apply(
        retry_entry(8, 2, "req_1", retry_message, "attempt_limit"),
        None,
    );
    for offset in 0..MAX_RECORDS - 1 {
        transcript.apply(status_entry(9 + offset as u64), None);
    }
    transcript.recent.insert(
        Record::from_entry(error_entry(200, 2, "req_1", "Connection reset by peer"))
            .expect("error record"),
    );

    let correlated = rendered_at_height(&mut transcript, 512);
    assert_eq!(1, correlated.matches(retry_message).count());
    assert!(transcript.recent.trim_back().is_some());

    let paged = rendered_at_height(&mut transcript, 512);
    assert_eq!(1, paged.matches(retry_message).count());
    assert!(!paged.contains("Connection reset by peer"));
}

#[test]
fn partial_reply_precedes_correlated_error_and_durable_interruption() {
    let partial = "Partial reply survives failure.";
    for status_text in [partial, ""] {
        let mut transcript = Transcript::default();
        transcript.live(
            12,
            Some(7),
            Some(2),
            &json!({
                "type":"reply_preview",
                "phase":"streaming",
                "content":partial
            }),
        );
        transcript.apply(
            retry_entry(9, 7, "req_1", "Fixture connection closed", "stream_started"),
            None,
        );
        transcript.apply(
            error_entry(10, 7, "req_1", "Fixture connection closed"),
            None,
        );
        let interruption: HistoryEntry = serde_json::from_value(json!({
            "id":11,
            "agent":"main",
            "turn":7,
            "turn_number":2,
            "execution_id":12,
            "type":"status",
            "status":"interrupted",
            "text":status_text
        }))
        .unwrap();
        transcript.apply(interruption, None);
        transcript.clear_preview();

        let text = rendered(&mut transcript);
        assert_eq!(1, text.matches(partial).count());
        assert!(
            text.find("Partial reply").unwrap() < text.find("Transport error").unwrap(),
            "{text}"
        );
    }
}

#[test]
fn live_partial_precedes_error_and_interruption_notice_without_execution_id() {
    let mut transcript = Transcript::default();
    transcript.live(
        12,
        Some(7),
        Some(2),
        &json!({
            "type":"reply_preview",
            "phase":"streaming",
            "content":"Partial reply survives failure."
        }),
    );
    transcript.apply(
        retry_entry(9, 7, "req_1", "Fixture connection closed", "stream_started"),
        None,
    );
    transcript.apply(
        error_entry(10, 7, "req_1", "Fixture connection closed"),
        None,
    );
    let interruption = interruption_entry(11, 7, None, "transport");
    transcript.apply(interruption, None);

    let text = rendered(&mut transcript);
    let partial = text.find("Partial reply").unwrap();
    assert!(
        partial < text.find("Transport error · Reply interrupted").unwrap(),
        "{text}"
    );
    assert!(!text.contains("Execution interrupted"), "{text}");
    assert!(
        !text.lines().any(|line| line.trim() == "transport"),
        "{text}"
    );
}

#[test]
fn correlated_provider_interruption_renders_one_error_block_after_page_load() {
    let retry_message = "Check the provider network, then Continue.";
    for stop_reason in [
        "stream_started",
        "attempt_limit",
        "non_retryable",
        "request_budget",
        "disabled",
        "future_reason",
    ] {
        for message in [retry_message, ""] {
            let mut transcript = Transcript::default();
            transcript.apply_page(
                HistoryRequest::Recent,
                HistoryPage {
                    entries: vec![
                        retry_entry(8, 2, "req_1", message, stop_reason),
                        error_entry(9, 2, "req_1", "Fixture connection closed"),
                        interruption_entry(10, 2, None, "transport"),
                    ],
                    next_before: None,
                },
            );

            let text = rendered(&mut transcript);
            assert!(
                text.contains("Transport error · Reply interrupted"),
                "{text}"
            );
            assert_eq!(1, text.matches("Fixture connection closed").count());
            assert!(
                text.contains("Diagnostic: Fixture connection closed"),
                "{text}"
            );
            assert!(text.to_lowercase().contains("no automatic retry"), "{text}");
            if stop_reason == "stream_started" {
                assert!(text.contains("Reply already started"), "{text}");
            }
            if message.is_empty() {
                assert!(
                    text.contains("Next step: Wait/check service/network; then Continue."),
                    "{text}"
                );
                assert_eq!(0, text.matches(retry_message).count());
            } else {
                assert_eq!(1, text.matches(retry_message).count());
                assert!(
                    !text.contains("Next step: Check the provider network, then Continue."),
                    "{text}"
                );
                assert!(
                    !text.contains("Wait/check service/network; then Continue."),
                    "{text}"
                );
            }
            assert!(!text.contains("Execution interrupted"), "{text}");
            assert!(
                !text.lines().any(|line| line.trim() == "transport"),
                "{text}"
            );
        }
    }
}

#[test]
fn legacy_model_error_and_provider_interruption_are_correlated_on_page_load() {
    let mut transcript = Transcript::default();
    transcript.apply_page(
        HistoryRequest::Recent,
        HistoryPage {
            entries: vec![
                error_entry(9, 2, "req_1", "Fixture connection closed"),
                interruption_entry(10, 2, None, "transport"),
            ],
            next_before: None,
        },
    );

    let text = rendered(&mut transcript);
    assert!(
        text.contains("Transport error · Reply interrupted"),
        "{text}"
    );
    assert_eq!(1, text.matches("Fixture connection closed").count());
    assert!(!text.contains("Execution interrupted"), "{text}");
    assert!(
        !text.lines().any(|line| line.trim() == "transport"),
        "{text}"
    );
}

#[test]
fn explicit_stop_and_different_execution_interruption_remain_separate() {
    for (execution_id, message) in [(Some(12), "User pressed Esc"), (None, "transport")] {
        let mut transcript = Transcript::default();
        transcript.apply(
            retry_entry(
                8,
                2,
                "req_1",
                "Check network, then Continue.",
                "stream_started",
            ),
            Some(12),
        );
        transcript.apply(
            error_entry(9, 2, "req_1", "Fixture connection closed"),
            Some(12),
        );
        let interruption = interruption_entry(10, 2, execution_id, message);
        let current_execution = if execution_id.is_some() {
            Some(12)
        } else {
            Some(13)
        };
        transcript.apply(interruption, current_execution);

        let text = rendered(&mut transcript);
        assert!(
            !text.contains("Transport error · Reply interrupted"),
            "{text}"
        );
        assert!(text.contains("Transport error"), "{text}");
        assert!(text.contains(message), "{text}");
    }
}
