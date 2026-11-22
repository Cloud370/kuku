use serde_json::json;

use crate::transcript::{HISTORY_PAGE, MAX_RECORDS};

use super::{line, text, HistoryPage, HistoryRequest, Journal, Transcript, TranscriptView};

#[test]
fn reloaded_file_results_group_even_when_the_calls_are_on_an_older_page() {
    let mut journal = Journal::default();
    for path in ["first.txt", "second.txt", "third.txt"] {
        journal.read(7, "main", path);
    }
    journal
        .0
        .retain(|entry| entry.kind == kuku_client::HistoryKind::ToolResult);
    let mut transcript = journal.load();
    let screen = transcript.project(80, 20);
    let group = transcript
        .detail_at(line(&screen, "Read 3 files"))
        .unwrap()
        .clone();
    assert!(transcript.toggle_detail(&group));
    let open = text(&transcript.project(80, 20));
    for path in ["first.txt", "second.txt", "third.txt"] {
        assert!(open.contains(&format!("Read {path}")), "{open}");
    }
}

#[test]
fn loading_previous_call_records_keeps_completed_read_order_and_expansion() {
    let mut journal = Journal::default();
    for path in ["first.txt", "second.txt", "third.txt"] {
        journal.read(7, "main", path);
    }
    let mut calls = Vec::new();
    let mut results = Vec::new();
    for mut entry in journal.0 {
        if entry.kind == kuku_client::HistoryKind::ToolCall {
            entry.id = calls.len() as u64 + 1;
            calls.push(entry);
        } else {
            entry.id = results.len() as u64 + 4;
            results.push(entry);
        }
    }
    let newest_call = calls.pop().unwrap();
    let mut transcript = Transcript::default();
    transcript.replace_recent(HistoryPage {
        entries: std::iter::once(newest_call).chain(results).collect(),
        next_before: Some(3),
        rewind_groups: Vec::new(),
    });
    let initial = transcript.project(90, 20);
    let key = transcript
        .detail_at(line(&initial, "Read 3 files"))
        .unwrap()
        .clone();
    assert!(transcript.toggle_detail(&key));
    let reads = |view: &TranscriptView| {
        view.lines
            .iter()
            .map(ToString::to_string)
            .filter(|row| super::card_text(row).starts_with("Read ") && row.contains(".txt"))
            .collect::<Vec<_>>()
    };
    let first = reads(&transcript.project(90, 20));
    assert_eq!(3, first.len());
    for (row, path) in first.iter().zip(["first.txt", "second.txt", "third.txt"]) {
        assert!(row.contains(path), "{first:?}");
    }
    let request = transcript.scroll(-8, 90, 20).unwrap();
    transcript.apply_page(
        request,
        HistoryPage {
            entries: calls,
            next_before: None,
            rewind_groups: Vec::new(),
        },
    );
    assert_eq!(first, reads(&transcript.project(90, 20)));
    transcript.show_latest();
    assert_eq!(first, reads(&transcript.project(90, 20)));
}

#[test]
fn repeated_file_changes_group_with_clickable_file_rows_and_keep_failures_visible() {
    let mut journal = Journal::default();
    for index in 1..=8 {
        let path = format!("chapter-{index}.md");
        journal.tool(
            "edit_file",
            json!({"path":path}),
            json!({"status":"ok","structured":{"kind":"file_edit","path":path,"change":{
                "added_lines":1,"removed_lines":1,"preview":"-before\n+after","preview_line_numbers":[10,10],
            }}}),
        );
    }
    journal.tool(
        "write_file",
        json!({"path":"missing.md"}),
        json!({
            "status":"error", "summary":"Could not write missing.md", "model_content":"Write denied"
        }),
    );
    let mut transcript = journal.load();
    let screen = transcript.project(80, 40);
    let compact = text(&screen);
    assert!(compact.contains("8 file changes") && compact.contains("Write denied"));
    for index in 1..=8 {
        assert!(compact.contains(&format!("Edited   chapter-{index}.md (+1 -1)")));
    }
    assert_eq!(8, compact.matches("10 - before").count());
    assert_eq!(8, compact.matches("10 + after").count());
    let file = transcript
        .detail_at(line(&screen, "Edited   chapter-1.md"))
        .unwrap()
        .clone();
    assert!(transcript.toggle_detail(&file));
    let opened = text(&transcript.project(80, 40));
    assert!(opened.contains("10 - before") && opened.contains("10 + after"));
    assert!(opened.contains("Edited   chapter-1.md"));
    assert!(opened.contains("Write denied"));
    assert!(transcript.toggle_detail(&file));
    assert_eq!(compact, text(&transcript.project(80, 40)));
}

#[test]
fn every_direct_and_script_write_shows_a_bounded_preview_by_default() {
    let mut journal = Journal::default();
    for index in 1..=6 {
        let path = format!("chapter-{index}.txt");
        let facts = json!({
            "kind": "file_write", "path": path, "canonical_path": format!("/workspace/{path}"),
            "commit_state": "committed", "content_hash_after": "recorded", "line_count": 5,
            "raw_text_after": format!("First {index}\n\nSecond\nThird\nLast {index}\n"),
        });
        if index <= 4 {
            journal.tool(
                "write_file",
                json!({"path":path}),
                json!({"status":"ok","structured":facts}),
            );
        } else {
            journal.tool(
                "run_js",
                json!({"title":"Write chapter"}),
                json!({"status":"ok","structured":{
                    "kind":"workspace_script_result","actions":[{
                        "id":"write","tool":"write_file","status":"ok",
                        "summary":"Wrote chapter","truncated":false,"result":facts
                    }],"value":null,"timed_out":false,"cleanup_incomplete":false,"not_dispatched":0
                }}),
            );
        }
    }
    let mut transcript = journal.load();
    let view = transcript.project(90, 55);
    let compact = text(&view);
    for index in 1..=6 {
        assert!(
            compact.contains(&format!("Wrote    chapter-{index}.txt (5 lines)")),
            "{compact}"
        );
        let row = &view.lines[line(&view, &format!("First {index}"))];
        assert!(row.style.bg.is_none());
        assert!(row
            .spans
            .iter()
            .all(|span| span.style.bg == crate::theme::transcript_background().bg));
    }
    assert!(!compact.contains("Third"), "{compact}");
    let target = transcript
        .detail_at(line(&view, "Wrote    chapter-6.txt"))
        .unwrap()
        .clone();
    assert!(transcript.toggle_detail(&target));
    let expanded = text(&transcript.project(90, 55));
    assert!(expanded.contains("Last 6"), "{expanded}");
    assert!(!expanded.contains("Last 5"), "{expanded}");
}

#[test]
fn every_script_edit_remains_visible_with_group_and_action_preview_limits() {
    let mut journal = Journal::default();
    for (ordinal, count) in [(0, 15), (15, 1), (16, 1), (17, 1)] {
        let actions: Vec<_> = (ordinal..ordinal + count)
            .map(|index| json!({
                "id": index.to_string(), "tool": "edit_file", "status": "ok",
                "summary": "Edited example.md", "truncated": false,
                "result": {
                    "kind": "file_edit", "path": "example.md", "canonical_path": "/workspace/example.md",
                    "commit_state": "committed", "content_hash_after": "recorded",
                    "change": {
                        "added_lines": 1, "removed_lines": 1,
                        "preview": format!("-before {index}\n+after {index}"),
                        "preview_line_numbers": [index + 10, index + 10]
                    }
                }
            }))
            .collect();
        journal.tool(
            "run_js",
            json!({"title": format!("Script {ordinal}")}),
            json!({
                "status": "ok",
                "structured": {
                    "kind": "workspace_script_result", "value": null, "actions": actions,
                    "timed_out": false, "cleanup_incomplete": false, "not_dispatched": 0
                }
            }),
        );
    }
    let mut transcript = journal.load();
    let compact = transcript.project(90, 80);
    let key = transcript
        .detail_at(line(&compact, "Script 0"))
        .unwrap()
        .clone();
    let check = |view: TranscriptView| {
        let rendered = text(&view);
        assert_eq!(
            18,
            rendered.matches("Edited   example.md").count(),
            "{rendered}"
        );
        for index in 0..18 {
            assert!(rendered.contains(&format!("{} - before {index}", index + 10)));
            assert!(rendered.contains(&format!("{} + after {index}", index + 10)));
        }
    };
    check(compact);
    assert!(transcript.toggle_detail(&key));
    check(transcript.project(90, 80));
}

#[test]
fn line_and_wheel_scrolling_cross_the_closing_rows_of_a_collapsed_group() {
    let mut journal = Journal::default();
    for path in ["first.txt", "second.txt", "third.txt", "fourth.txt"] {
        journal.read(7, "main", path);
    }
    journal.push(
        7,
        "main",
        serde_json::json!({"type":"assistant","text":"Read complete."}),
    );
    for increment in [1, 3] {
        let mut transcript = Journal(journal.0.clone()).load();
        transcript.scroll(-100, 80, 3);
        for _ in 0..12 {
            if !transcript.is_reading() {
                break;
            }
            let before = transcript.reading_anchor().cloned();
            transcript.scroll(increment, 80, 3);
            assert_ne!(before.as_ref(), transcript.reading_anchor());
        }
        assert!(!transcript.is_reading(), "scroll by {increment}");
        assert!(text(&transcript.project(80, 3)).contains("Read complete."));
    }
}

#[test]
fn loading_older_history_keeps_a_visible_batch_and_the_card_after_it() {
    let mut journal = Journal::default();
    for index in 0..HISTORY_PAGE {
        journal.push(
            7,
            "main",
            json!({"type":"assistant","text":format!("Older {index}")}),
        );
    }
    let recent = journal.0.len();
    journal.push(7, "main", json!({"type":"user","text":"Review files."}));
    let count = MAX_RECORDS - HISTORY_PAGE / 2;
    for index in 0..count {
        journal.read(7, "main", &format!("sample-{index}.txt"));
    }
    journal.push(
        7,
        "main",
        json!({"type":"user","text":"Show me the findings."}),
    );
    let latest = journal.0.split_off(recent);
    let cursor = latest[0].id;
    let mut transcript = Transcript::default();
    transcript.replace_recent(HistoryPage {
        entries: latest,
        next_before: Some(cursor),
        rewind_groups: Vec::new(),
    });
    let (width, height) = (90, 14);
    let request = transcript.scroll(-1, width, height).unwrap();
    assert_eq!(HistoryRequest::Older(cursor), request);
    let before = text(&transcript.project(width, height));
    assert!(
        before.contains(&format!("Read {count} files")) && before.contains("Show me the findings."),
        "{before}"
    );
    transcript.apply_page(
        request,
        HistoryPage {
            entries: journal.0,
            next_before: None,
            rewind_groups: Vec::new(),
        },
    );
    assert_eq!(before, text(&transcript.project(width, height)));
}

#[test]
fn repeated_workspace_diagnostics_keep_single_preview_and_individual_calls() {
    let mut journal = Journal::default();
    let fail = serde_json::json!({
        "status": "error", "summary": "Result limit reached",
        "model_content": "Result limit reached"
    });
    for name in ["First script", "Second script"] {
        journal.read(7, "main", "source.txt");
        journal.tool("run_js", serde_json::json!({"title":name}), fail.clone());
    }
    let mut transcript = journal.load();
    let view = transcript.project(82, 32);
    let collapsed = text(&view);
    assert_eq!(1, collapsed.matches("2 file reads").count());
    for label in ["2 file reads", "Result limit reached"] {
        let row = &view.lines[line(&view, label)];
        assert!(row.style.bg.is_none());
        assert!(row
            .spans
            .iter()
            .all(|span| span.style.bg == crate::theme::transcript_background().bg));
    }
    assert_eq!(1, collapsed.matches("Result limit reached").count());
    assert!(collapsed.contains("error ×2"));
    let title = line(&view, "Script   First script");
    let detail = transcript.detail_at(title).unwrap().clone();
    assert!(transcript.toggle_in_view(&detail, 82, 32));
    let inspected = transcript.project(82, 32);
    assert_eq!(view.lines[title], inspected.lines[title]);
    let group = transcript
        .detail_at(line(&inspected, "2 file reads"))
        .unwrap()
        .clone();
    assert!(transcript.toggle_detail(&group));
    let inspected = transcript.project(82, 32);
    let rendered = text(&inspected);
    assert!(rendered.contains("First script") && rendered.contains("Second script"));
    let first = transcript
        .detail_at(line(&inspected, "First script"))
        .unwrap();
    let second = transcript
        .detail_at(line(&inspected, "Second script"))
        .unwrap();
    assert_ne!(first, second);
}

#[test]
fn a_page_that_settles_the_top_row_keeps_reading_after_the_changed_group() {
    let mut journal = Journal::default();
    for (id, path) in [("a", "first.txt"), ("b", "second.txt")] {
        journal.push(
            7,
            "main",
            serde_json::json!({
                "type":"tool_call", "tool":"read_file", "tool_call_id":id, "args":{"path":path}
            }),
        );
    }
    for (id, path) in [("a", "first.txt"), ("b", "second.txt")] {
        journal.push(7, "main", serde_json::json!({
            "type":"tool_result", "tool_call_id":id, "status":"ok",
            "structured":{"kind":"file_content","path":path,"start_line":1,"line_count":1,"total_lines":1}
        }));
    }
    journal.push(
        7,
        "main",
        serde_json::json!({"type":"assistant", "text":"The files are ready."}),
    );
    let mut page_entries = journal.0;
    let missing: Vec<_> = page_entries.drain(2..4).collect();
    let mut transcript = Journal(page_entries).load();
    let width = 80;
    let height = 3;
    transcript.scroll(-100, width, height);
    transcript.scroll(2, width, height);
    let viewport = text(&transcript.project(width, height));
    assert!(viewport
        .lines()
        .next()
        .unwrap()
        .contains("result not loaded ×2"));
    let request = HistoryRequest::Newer {
        first: 3,
        before: 5,
    };
    transcript
        .reading
        .as_mut()
        .unwrap()
        .newer
        .push_back(request);
    transcript.apply_page(
        request,
        HistoryPage {
            entries: missing,
            rewind_groups: Vec::new(),
            next_before: None,
        },
    );
    let after = text(&transcript.project(width, height));
    assert!(!after.contains("Read 2 files"), "{after}");
    assert!(after.contains("Assistant"), "{after}");
}

#[test]
fn repeated_global_toggles_follow_the_group_state_before_the_next_frame() {
    let mut journal = Journal::default();
    journal.read(7, "main", "first.txt");
    journal.read(7, "main", "second.txt");
    journal.read(7, "main", "third.txt");
    let mut transcript = journal.load();
    transcript.project(80, 20);
    assert!(transcript.toggle_details());
    assert!(!transcript.toggle_details());
    let collapsed = text(&transcript.project(80, 20));
    assert!(collapsed.contains("second.txt") && collapsed.contains("third.txt"));
    assert!(!collapsed.contains("first.txt"));
    assert!(transcript.toggle_details());
    assert!(text(&transcript.project(80, 20)).contains("Read first.txt"));
}
