use kuku_client::{HistoryEntry, HistoryKind, HistoryPage};
use serde_json::json;

use super::{HistoryRequest, Key, ReplySource, Transcript, HISTORY_PAGE};

fn response(id: u64, body: String) -> HistoryEntry {
    HistoryEntry {
        id,
        agent: "main".into(),
        turn: Some(1),
        kind: HistoryKind::Assistant,
        text: Some(body),
        turn_summary: None,
        details: [("turn_number".into(), json!(1))].into_iter().collect(),
    }
}

fn page(first: u64, before: u64) -> HistoryPage {
    HistoryPage {
        entries: (first..before)
            .map(|id| response(id, format!("Answer {id}")))
            .collect(),
        next_before: (first > 1).then_some(first),
    }
}

fn rows(transcript: &mut Transcript) -> Vec<String> {
    transcript
        .project(44, 10)
        .lines
        .iter()
        .map(ToString::to_string)
        .collect()
}

#[test]
fn frozen_history_survives_older_page_arrival_and_many_recent_commits() {
    let mut transcript = Transcript::default();
    transcript.replace_recent(page(51, 70));
    let older = transcript.scroll(-200, 44, 10);
    assert_eq!(Some(HistoryRequest::Older(51)), older);
    let anchored = rows(&mut transcript);
    transcript.apply_page(older.unwrap(), page(3, 51));
    for id in 70..240 {
        transcript.apply(response(id, format!("Answer {id}")), Some(100));
    }
    assert_eq!(anchored, rows(&mut transcript));
    transcript.show_latest();
    assert!(rows(&mut transcript)
        .iter()
        .any(|row| row.contains("Answer 239")));
}

#[test]
fn rewind_invalidation_discards_reading_window_and_saved_anchor() {
    let mut transcript = Transcript::default();
    transcript.replace_recent(page(51, 91));
    assert!(transcript.scroll(-200, 44, 10).is_some());
    assert!(transcript.is_reading());

    transcript.invalidate_after_rewind();

    assert!(!transcript.is_reading());
    assert_eq!(None, transcript.restore_request());
    assert!(rows(&mut transcript).is_empty());
}

#[test]
fn rewind_picker_uses_display_turn_number_and_keeps_event_identity() {
    let mut transcript = Transcript::default();
    transcript.replace_recent(HistoryPage {
        entries: vec![HistoryEntry {
            id: 99,
            agent: "main".into(),
            turn: Some(23),
            kind: HistoryKind::User,
            text: Some("selected prompt".into()),
            turn_summary: None,
            details: [("turn_number".into(), json!(2))].into_iter().collect(),
        }],
        next_before: None,
    });

    assert_eq!(
        vec![(99, 2, "selected prompt".into())],
        transcript.rewind_messages("main")
    );
}

#[test]
fn evicted_view_restores_its_saved_anchor_from_durable_history() {
    let mut transcript = Transcript::default();
    transcript.replace_recent(page(51, 91));
    transcript.scroll(-36, 44, 10);
    let anchored = rows(&mut transcript);
    transcript.suspend();
    let request = transcript.restore_request().unwrap();
    let before = request.before();
    transcript.replace_recent(page(101, 141));
    transcript.apply_page(request, page(before - HISTORY_PAGE as u64, before));
    assert_eq!(anchored, rows(&mut transcript));
}

#[test]
fn durable_response_replaces_its_live_preview() {
    let mut transcript = Transcript::default();
    transcript.live(
        7,
        Some(1),
        Some(1),
        &json!({"type":"text","content":"temporary preview"}),
    );
    assert!(rows(&mut transcript)
        .iter()
        .any(|row| row.contains("temporary preview")));
    transcript.apply(response(8, "settled response".into()), Some(7));
    let rendered = rows(&mut transcript).join("\n");
    assert!(!rendered.contains("temporary preview"));
    assert!(rendered.contains("settled response"));
}

#[test]
fn interrupted_reply_replaces_preview_and_preserves_previous_copy_target() {
    let previous = response(2, "Last complete reply".into());
    let interruption: HistoryEntry = serde_json::from_value(json!({
        "id":9,"agent":"main","turn":7,"turn_number":2,"execution_id":8,
        "type":"status","status":"interrupted","text":"Interrupted **body**.","truncated":true,
    }))
    .unwrap();
    let mut transcript = Transcript::default();
    transcript.apply(previous.clone(), None);
    transcript.live(
        8,
        Some(7),
        Some(2),
        &json!({"type":"text","content":"Interrupted **body**."}),
    );
    transcript.apply(interruption.clone(), None);

    let rendered = rows(&mut transcript).join("\n");
    assert!(rendered
        .lines()
        .collect::<Vec<_>>()
        .windows(2)
        .any(|pair| pair[0].starts_with("Turn 2  ─") && pair[1] == "Interrupted"));
    assert_eq!(1, rendered.matches("Interrupted body.").count());
    assert!(rendered.contains("Display limit reached"));
    assert_eq!(
        Some(ReplySource::Local("Last complete reply")),
        transcript.last_reply()
    );
    transcript.replace_recent(HistoryPage {
        entries: vec![previous, interruption],
        next_before: None,
    });
    assert_eq!(rendered, rows(&mut transcript).join("\n"));
}

#[test]
fn cancellation_without_visible_reply_still_renders_a_turn_marker() {
    let mut transcript = Transcript::default();
    let entry: HistoryEntry = serde_json::from_value(json!({
        "id":9,"agent":"main","turn":7,"turn_number":1,"execution_id":8,
        "type":"status","status":"interrupted"
    }))
    .unwrap();
    transcript.apply(entry, None);
    let rendered = rows(&mut transcript).join("\n");
    assert!(rendered
        .lines()
        .collect::<Vec<_>>()
        .windows(2)
        .any(|pair| pair[0].starts_with("Turn 1  ─") && pair[1] == "Interrupted"));
    assert!(transcript.last_reply().is_none());
}

#[test]
fn roles_and_file_details_are_readable_without_styles_at_narrow_widths() {
    let mut transcript = Transcript::default();
    let mut question = response(1, "看一下 e\u{301} 👩‍💻".into());
    question.kind = HistoryKind::User;
    transcript.apply(question, None);
    transcript.apply(response(2, "我来检查".into()), None);
    transcript.apply(
        serde_json::from_value(json!({
            "id": 3, "agent": "main", "turn": 1, "type": "tool_call",
            "tool": "read_file", "tool_call_id": "read", "args": {"path": "中文.txt"}
        }))
        .unwrap(),
        None,
    );
    transcript.apply(
        serde_json::from_value(json!({
            "id": 4, "agent": "main", "turn": 1, "type": "tool_result",
            "tool_call_id": "read", "status": "error", "summary": "Unable to read",
            "model_content": "文件丢失"
        }))
        .unwrap(),
        None,
    );

    for width in [17, 72] {
        let view = transcript.project(width, 32);
        let plain: Vec<_> = view.lines.iter().map(ToString::to_string).collect();
        assert_eq!(
            if width == 17 {
                "│ You / Turn 1"
            } else {
                "│  You   /   Turn 1"
            },
            plain[0]
        );
        let (body, inset) = if width == 17 { (1, 2) } else { (2, 3) };
        assert_eq!(
            "看一下 e\u{301} 👩‍💻",
            plain[body].trim_start_matches('│').trim_start()
        );
        assert_eq!(inset, view.row_insets[body]);
        assert!(plain
            .windows(2)
            .any(|pair| pair == ["Assistant", "我来检查"]));
        assert!(plain
            .iter()
            .any(|row| row.trim_start().starts_with("Read file")));
        assert!(plain.iter().any(|row| row.contains("error")));
        assert!(plain.iter().any(|row| row.trim_start() == "中文.txt"));
        assert!(plain.iter().any(|row| row.trim_start() == "文件丢失"));
        assert!(view.lines.iter().all(|line| line.width() <= width));
    }

    let compact: Vec<_> = transcript
        .project(8, 40)
        .lines
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(compact[0].starts_with("Turn 1"));
    assert_eq!("You", compact[1]);
    assert!(compact.iter().any(|row| row == "AI"));
    assert!(compact.iter().any(|row| row == "文件丢失"));
}

#[test]
fn toggling_tool_previews_preserves_the_reading_anchor_through_settlement() {
    let mut transcript = Transcript::default();
    transcript.apply(response(1, "Earlier reply".into()), None);
    transcript.apply(
        serde_json::from_value(json!({
            "id": 2, "agent": "main", "turn": 1, "type": "tool_call",
            "tool": "run_command", "tool_call_id": "cmd", "args": {"command": "check-workspace", "brief":"Check workspace"}
        }))
        .unwrap(),
        None,
    );
    assert!(rows(&mut transcript)
        .iter()
        .any(|row| row.contains("requested")));
    assert!(rows(&mut transcript)
        .iter()
        .any(|row| row.contains("Check workspace")));

    transcript.live(
        9,
        Some(1),
        Some(1),
        &json!({
            "type": "tool_start", "id": "cmd", "tool": "run_command", "summary": "Checking"
        }),
    );
    transcript.scroll(-50, 60, 5);
    let anchor = transcript.reading.as_ref().unwrap().anchor.clone();
    assert!(!transcript.toggle_details());
    transcript.live(
        9,
        Some(1),
        Some(1),
        &json!({
            "type": "tool_output", "id": "cmd", "event": {"stdout": "working output"}
        }),
    );
    let collapsed = rows(&mut transcript).join("\n");
    assert!(collapsed.contains("running"));
    assert!(!collapsed.contains("working output"));
    assert!(transcript.toggle_details());
    assert!(rows(&mut transcript)
        .iter()
        .any(|row| row.contains("working output")));

    transcript.live(
        9,
        Some(1),
        Some(1),
        &json!({
            "type": "tool_end", "id": "cmd", "status": "error", "summary": "Live failure",
            "model_content": "live diagnostic"
        }),
    );
    assert!(rows(&mut transcript)
        .iter()
        .any(|row| row.contains("live diagnostic")));
    transcript.apply(serde_json::from_value(json!({
        "id": 3, "agent": "main", "turn": 1, "type": "tool_result",
        "tool_call_id": "cmd", "status": "ok", "summary": "Command completed",
        "model_content": "durable diagnostic", "structured": {"exit_code": 7, "output_id": "out"}
    })).unwrap(), Some(9));
    transcript.live(
        9,
        Some(1),
        Some(1),
        &json!({
            "type": "tool_output", "id": "cmd", "event": {"stderr": "late duplicate"}
        }),
    );
    let settled = transcript.project(60, 20);
    let plain: Vec<_> = settled.lines.iter().map(ToString::to_string).collect();
    assert_eq!(&anchor, &transcript.reading.as_ref().unwrap().anchor);
    assert!(plain[0].starts_with("Turn 1  ─"));
    assert_eq!("Assistant", plain[1]);
    assert_eq!(
        1,
        plain
            .iter()
            .filter(|row| row.starts_with("╭─ Check workspace"))
            .count()
    );
    assert!(plain.iter().any(|row| row.contains("failed · exit 7")));
    assert!(plain
        .iter()
        .any(|row| row.trim_matches('│').trim() == "durable diagnostic"));
    assert!(plain
        .iter()
        .any(|row| row.trim_matches('│').trim() == "Saved output · /tools"));
    assert!(!plain
        .iter()
        .any(|row| row.contains("live diagnostic") || row.contains("late duplicate")));
    let tools = transcript.tools();
    assert_eq!(Some("out"), tools[0].output_id.as_deref());
    assert_eq!("check-workspace", tools[0].invocation);
}

#[test]
fn history_page_join_only_marks_genuine_turn_boundaries_and_preserves_the_anchor() {
    let mut transcript = Transcript::default();
    let mut first = response(15, "First visible model step".into());
    first.turn = Some(6);
    let mut same_turn = response(19, "Another model step".into());
    same_turn.turn = Some(6);
    let mut later = response(24, "Different turn".into());
    later.turn = Some(23);
    later.details.insert("turn_number".into(), json!(2));
    transcript.replace_recent(HistoryPage {
        entries: vec![first, same_turn, later],
        next_before: Some(15),
    });
    let request = transcript.scroll(-100, 44, 10).expect("older history");
    let anchored = rows(&mut transcript);
    assert_eq!("Assistant", anchored[0]);
    assert_eq!(
        1,
        anchored
            .iter()
            .filter(|row| row.starts_with("Turn 2  ─"))
            .count()
    );
    assert!(!anchored
        .iter()
        .any(|row| matches!(row.as_str(), "Turn 6" | "Turn 23")));

    let mut first_message = response(8, "Inspect the file".into());
    first_message.kind = HistoryKind::User;
    first_message.turn = Some(6);
    let mut older_step = response(12, "Earlier model step".into());
    older_step.turn = Some(6);
    transcript.apply_page(
        request,
        HistoryPage {
            entries: vec![first_message, older_step],
            next_before: None,
        },
    );
    assert_eq!(anchored, rows(&mut transcript));

    transcript.scroll(-100, 72, 24);
    let all: Vec<_> = transcript
        .project(72, 24)
        .lines
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        vec!["1", "2"],
        all.iter()
            .filter_map(|row| row.strip_prefix("│  You   /   Turn ").or_else(|| row
                .strip_prefix("Turn ")
                .and_then(|tail| tail.split_once("  ─"))
                .map(|(number, _)| number)))
            .collect::<Vec<_>>()
    );
    assert_eq!("│  Inspect the file", all[2]);
}

#[test]
fn live_and_committed_turns_use_display_numbers_and_unknown_remains_generic() {
    let mut transcript = Transcript::default();
    let mut first = response(8, "First reply".into());
    first.turn = Some(6);
    transcript.apply(first, None);
    transcript.live(
        22,
        Some(23),
        Some(2),
        &json!({"type":"text","content":"Second reply"}),
    );
    let live = rows(&mut transcript);
    assert_eq!(
        1,
        live.iter()
            .filter(|row| row.starts_with("Turn 1  ─"))
            .count()
    );
    assert_eq!(
        1,
        live.iter()
            .filter(|row| row.starts_with("Turn 2  ─"))
            .count()
    );

    let mut settled = response(29, "Second reply".into());
    settled.turn = Some(23);
    settled.details.insert("turn_number".into(), json!(2));
    transcript.apply(settled, Some(22));
    let completed = rows(&mut transcript);
    assert_eq!(live, completed);

    let mut unknown = response(30, "Unnumbered reply".into());
    unknown.turn = Some(26);
    unknown.details.clear();
    transcript.replace_recent(HistoryPage {
        entries: vec![unknown],
        next_before: None,
    });
    assert!(rows(&mut transcript)[0].starts_with("Turn  ─"));
}

#[test]
fn a_thinking_only_commit_retires_the_preview_without_emitting_an_empty_assistant() {
    let mut transcript = Transcript::default();
    transcript.apply(response(1, "Previous reply".into()), None);
    transcript.live(
        7,
        Some(1),
        Some(1),
        &json!({"type":"text","content":"  \n "}),
    );
    assert_eq!(
        1,
        rows(&mut transcript)
            .iter()
            .filter(|row| row.as_str() == "Assistant")
            .count()
    );
    transcript.live(
        7,
        Some(1),
        Some(1),
        &json!({"type":"text","content":"temporary answer"}),
    );
    assert!(rows(&mut transcript)
        .iter()
        .any(|row| row.contains("temporary answer")));
    transcript.apply(serde_json::from_value(json!({
        "id": 2, "agent": "main", "turn": 1, "type": "assistant", "text": "", "thinking": "Consult the tool"
    })).unwrap(), Some(7));
    let view = rows(&mut transcript);
    assert_eq!(
        1,
        view.iter()
            .filter(|row| row.as_str() == "Assistant")
            .count()
    );
    assert!(!view.iter().any(|row| row.contains("temporary answer")));
    assert_eq!(
        Some(ReplySource::Local("Previous reply")),
        transcript.last_reply()
    );
}

#[test]
fn assistant_markdown_keeps_the_model_sections_as_prose_and_raw_copy() {
    let mut transcript = Transcript::default();
    let reply = "## Checks\n\n**Tests failed.**\n\n| Check | Result |\n|---|---|\n| Build | 失败 |\n\n## Suggestions\n\n1. Inspect the log.\n\n_Should I proceed?_";
    transcript.apply(response(1, reply.into()), None);
    let rendered = transcript
        .project(72, 24)
        .lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("Checks\n\nTests failed."));
    let narrow = transcript.project(24, 24);
    assert!(narrow.lines.iter().all(|line| line.width() <= 24));
    assert!(narrow
        .lines
        .iter()
        .any(|line| line.to_string().contains("失败")));
    assert!(rendered.contains("Suggestions\n\n1. Inspect the log."));
    assert!(rendered.contains("Should I proceed?"));
    assert!(!rendered.contains("Permission") && !rendered.contains("F2"));
    assert_eq!(Some(ReplySource::Local(reply)), transcript.last_reply());
}

#[test]
fn source_keeps_nested_fences_and_both_renderings_are_cached_until_the_reply_changes() {
    let mut transcript = Transcript::default();
    let chunk = "````markdown\n# Document\n\n```text\nsafe\u{001b}[31m\n```\n";
    transcript.live(7, Some(1), Some(1), &json!({"type":"text","content":chunk}));
    let preview = rows(&mut transcript);
    let cached_preview = transcript.recent.records[0]
        .rendered
        .preview
        .as_ref()
        .unwrap()
        .1
        .as_ptr();

    assert!(transcript.toggle_source());
    let raw = rows(&mut transcript).join("\n");
    assert!(raw.contains("````markdown\n# Document\n\n```text\nsafe␛[31m\n```"));
    assert!(!raw.contains('\u{001b}'));
    let cache = &transcript.recent.records[0].rendered;
    let cached_source = cache.source.as_ref().unwrap().1.as_ptr();
    assert_eq!(cached_preview, cache.preview.as_ref().unwrap().1.as_ptr());

    assert!(!transcript.toggle_source());
    assert_eq!(preview, rows(&mut transcript));
    let cache = &transcript.recent.records[0].rendered;
    assert_eq!(cached_preview, cache.preview.as_ref().unwrap().1.as_ptr());
    assert_eq!(cached_source, cache.source.as_ref().unwrap().1.as_ptr());

    assert!(transcript.toggle_source());
    transcript.live(
        7,
        Some(1),
        Some(1),
        &json!({"type":"text","content":"````\n"}),
    );
    assert!(transcript.recent.records[0].rendered.preview.is_none());
    assert!(rows(&mut transcript).iter().any(|row| row == "````"));

    let committed = format!("{chunk}````\n");
    transcript.apply(response(9, committed.clone()), Some(7));
    assert!(rows(&mut transcript).iter().any(|row| row == "````"));
    assert_eq!(
        Some(ReplySource::Local(&committed)),
        transcript.last_reply()
    );
}

#[test]
fn toggling_markdown_source_keeps_the_reading_anchor_and_round_trips_the_view() {
    let mut transcript = Transcript::default();
    transcript.apply(response(1, "Earlier answer".into()), None);
    let body = (0..30)
        .map(|index| format!("## Section {index}\n\n- Item {index}"))
        .collect::<Vec<_>>()
        .join("\n\n");
    transcript.apply(response(2, body), None);
    transcript.apply(response(3, "Latest answer".into()), None);
    transcript.scroll(-20, 44, 10);
    let anchor = transcript.reading.as_ref().unwrap().anchor.clone();
    assert_eq!(Key::Event(2), anchor.key);
    let preview = rows(&mut transcript);

    assert!(transcript.toggle_source());
    let source = transcript.project(44, 10);
    assert!(!source.following);
    assert!(source
        .lines
        .iter()
        .any(|row| row.to_string().starts_with("## Section")));
    assert_eq!(&anchor, &transcript.reading.as_ref().unwrap().anchor);

    assert!(!transcript.toggle_source());
    assert_eq!(preview, rows(&mut transcript));
}

#[test]
fn streaming_tables_code_and_unicode_use_the_committed_markdown_layout() {
    let mut transcript = Transcript::default();
    let chunks = [
        "## 检查\n\n| 文件 |",
        " 状态 |\n|---|---|\n| 中文 | 完成 |\n\n```rust\n",
        "let text = \"中 e\u{301} 👩‍💻\";\n",
        "```\n",
    ];
    let width = 25;
    for chunk in chunks {
        transcript.live(
            4,
            Some(1),
            Some(1),
            &json!({"type": "text", "content": chunk}),
        );
        let view = transcript.project(width, 30);
        assert!(view.lines.iter().all(|line| line.width() <= width));
    }
    let live: Vec<_> = transcript
        .project(width, 30)
        .lines
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(live.iter().any(|row| row.contains("完成")));
    assert!(live.iter().any(|row| row.contains("e\u{301} 👩‍💻")));
    let source = chunks.concat();
    transcript.apply(response(8, source.clone()), Some(4));
    let committed: Vec<_> = transcript
        .project(width, 30)
        .lines
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(live, committed);
    assert_eq!(Some(ReplySource::Local(&source)), transcript.last_reply());
}

#[test]
fn latest_reply_keeps_its_history_identity_after_the_visible_window_evicts_it() {
    let mut transcript = Transcript::default();
    transcript.apply(response(9, "Retained source".into()), None);
    for offset in 0..super::MAX_RECORDS as u64 {
        let mut entry = response(10 + offset, "Tool result".into());
        entry.kind = HistoryKind::ToolResult;
        transcript.apply(entry, None);
    }
    assert_eq!(Some(ReplySource::History(9)), transcript.last_reply());
    transcript.suspend();
    transcript.replace_recent(page(1, 4));
    assert_eq!(Some(ReplySource::History(9)), transcript.last_reply());
}
