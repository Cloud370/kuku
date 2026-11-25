use serde_json::json;

use crate::transcript::{HISTORY_PAGE, MAX_RECORDS};

use super::{
    HistoryPage, HistoryRequest, Journal, Transcript, TranscriptView, collapse_auto_group, line,
    text,
};

#[test]
fn latest_activity_opens_only_the_readable_group_and_respects_manual_choices() {
    let mut journal = Journal::default();
    for path in ["old-first.txt", "old-second.txt", "old-third.txt"] {
        journal.read(7, "main", path);
    }
    let mut transcript = Journal(journal.0.clone()).load();
    let first = text(&transcript.project(100, 60));
    assert!(first.contains("Read old-first.txt"), "{first}");
    assert!(
        !first.contains("contents for old-first.txt") && !first.contains("Requested arguments")
    );
    let boundary = journal.0.len();
    journal.push(
        7,
        "main",
        json!({"type":"assistant","text":"Now check the other files."}),
    );
    journal.read(7, "main", "fresh-first.txt");
    journal.read(7, "main", "fresh-second.txt");
    journal.push(7, "main", json!({"type":"tool_call","tool":"read_file","tool_call_id":"pending","args":{"path":"fresh-third.txt"}}));
    for entry in journal.0[boundary..].iter().cloned() {
        transcript.apply(entry, None);
    }
    let view = transcript.project(100, 60);
    let second = text(&view);
    assert!(!second.contains("Read old-first.txt"), "{second}");
    assert!(second.contains("Read fresh-first.txt"), "{second}");
    let current = transcript
        .detail_items
        .visible
        .iter()
        .find(|item| item.key.kind == super::DetailKind::Group && item.open)
        .expect("one open group")
        .key
        .clone();
    assert!(transcript.toggle_detail(&current));
    transcript.live(
        49,
        Some(7),
        None,
        &json!({"type":"tool_start","tool":"read_file","id":"pending"}),
    );
    transcript.project(100, 60);
    assert!(
        transcript
            .detail_items
            .visible
            .iter()
            .filter(|item| item.key.kind == super::DetailKind::Group)
            .all(|item| !item.open)
    );

    let boundary = journal.0.len();
    journal.push(7, "main", json!({"type":"tool_result","tool":"read_file","tool_call_id":"pending","status":"ok","summary":"Read third","model_content":"Do not expose raw result","structured":{"kind":"file_content","path":"fresh-third.txt","start_line":1,"line_count":1,"total_lines":1}}));
    transcript.apply(journal.0[boundary].clone(), Some(49));
    let settled = transcript.project(100, 60);
    assert!(
        transcript
            .detail_items
            .visible
            .iter()
            .filter(|item| item.key.kind == super::DetailKind::Group)
            .all(|item| !item.open),
        "{}",
        text(&settled)
    );
    assert!(!text(&settled).contains("Do not expose raw result"));

    let old = transcript
        .detail_at(line(&settled, "Read 3 files"))
        .unwrap()
        .clone();
    assert!(transcript.toggle_detail(&old));
    let boundary = journal.0.len();
    journal.push(
        7,
        "main",
        json!({"type":"assistant","text":"Third batch next."}),
    );
    for path in ["last-first.txt", "last-second.txt", "last-third.txt"] {
        journal.read(7, "main", path);
    }
    for entry in journal.0[boundary..].iter().cloned() {
        transcript.apply(entry, None);
    }
    let third = text(&transcript.project(100, 80));
    assert!(
        third.contains("Read old-first.txt") && third.contains("Read last-first.txt"),
        "{third}"
    );
    assert!(!third.contains("Read fresh-first.txt"), "{third}");
    assert_eq!(
        2,
        transcript
            .detail_items
            .visible
            .iter()
            .filter(|item| item.key.kind == super::DetailKind::Group && item.open)
            .count()
    );

    // An already-readable card takes over without opening its raw details.
    journal.push(7, "main", json!({"type":"job_finished","job_id":"review-job","status":"completed","summary":"Background report ready"}));
    transcript.apply(journal.0.last().unwrap().clone(), None);
    let job = text(&transcript.project(100, 80));
    assert!(
        job.contains("Background report ready") && job.contains("Read old-first.txt"),
        "{job}"
    );
    assert!(!job.contains("Read last-first.txt"), "{job}");
}

#[test]
fn oversized_latest_activity_uses_the_compact_preview_until_explicitly_opened() {
    use crate::transcript::window::MAX_AUTO_GROUP_MEMBERS;

    let mut journal = Journal::default();
    for index in 0..MAX_AUTO_GROUP_MEMBERS {
        journal.read(7, "main", &format!("bounded-{index}.txt"));
    }
    let mut transcript = Journal(journal.0.clone()).load();
    assert!(text(&transcript.project(100, 60)).contains("Read bounded-0.txt"));
    let boundary = journal.0.len();
    journal.read(7, "main", "one-more.txt");
    for entry in journal.0[boundary..].iter().cloned() {
        transcript.apply(entry, None);
    }
    let collapsed = transcript.project(100, 60);
    assert!(!text(&collapsed).contains("Read bounded-0.txt"));
    assert!(collapsed.lines.len() <= 9);
    let group = transcript
        .detail_items
        .visible
        .iter()
        .find(|item| item.key.kind == super::DetailKind::Group && !item.open)
        .unwrap()
        .key
        .clone();
    assert!(transcript.toggle_detail(&group));
    assert!(text(&transcript.project(100, 60)).contains("Read bounded-0.txt"));
    let boundary = journal.0.len();
    journal.read(7, "main", "another.txt");
    for entry in journal.0[boundary..].iter().cloned() {
        transcript.apply(entry, None);
    }
    assert!(text(&transcript.project(100, 60)).contains("Read bounded-0.txt"));
}

#[test]
fn oversized_mixed_activity_keeps_the_newest_result_and_priority_previews() {
    let mut journal = Journal::default();
    for index in 0..4 {
        journal.read(7, "main", &format!("reference-{index}.txt"));
    }
    journal.tool(
        "write_file",
        json!({"path":"changed.txt"}),
        json!({
            "status":"ok","structured":{"kind":"file_write","path":"changed.txt",
            "line_count":1,"raw_text_after":"Saved line"}
        }),
    );
    journal.tool(
        "run_command",
        json!({"program":"failed-check"}),
        json!({
            "status":"error","model_content":"Important failure","structured":{"exit_code":1}
        }),
    );
    for program in ["first-check", "second-check", "newest-check"] {
        journal.tool(
            "run_command",
            json!({"program":program}),
            json!({
                "status":"ok","model_content":"Raw command result","structured":{"exit_code":0}
            }),
        );
    }
    let mut transcript = Journal(journal.0.clone()).load();
    let commands = transcript.project(100, 60);
    let rendered = text(&commands);
    assert!(
        rendered.contains("changed.txt") && rendered.contains("Important failure"),
        "{rendered}"
    );
    assert!(
        commands.lines.iter().any(|row| {
            let row = row.to_string();
            row.contains("newest-check") && row.contains("ok · exit 0")
        }),
        "{rendered}"
    );
    assert!(!rendered.contains("Read reference-0.txt") && !rendered.contains("Raw command result"));
    assert!(
        transcript
            .detail_items
            .visible
            .iter()
            .any(|item| item.key.kind == super::DetailKind::Group && !item.open)
    );

    let boundary = journal.0.len();
    journal.read(7, "main", "latest-read.txt");
    for entry in journal.0[boundary..].iter().cloned() {
        transcript.apply(entry, None);
    }
    let current = transcript.project(100, 60);
    let rendered = text(&current);
    assert!(
        rendered.contains("Read latest-read.txt")
            && rendered.contains("Important failure")
            && rendered.contains("changed.txt"),
        "{rendered}"
    );
    assert!(!rendered.contains("contents for latest-read.txt"));
    assert!(!rendered.contains("Read reference-0.txt"));
    transcript.scroll(-100, 100, 60);
    assert_eq!(rendered, text(&transcript.project(100, 60)));
}

#[test]
fn reading_freezes_the_automatic_group_even_when_newer_history_arrives() {
    let mut journal = Journal::default();
    journal.push(
        7,
        "main",
        json!({"type":"user","text":"Inspect the files."}),
    );
    for path in ["first.txt", "second.txt", "third.txt"] {
        journal.read(7, "main", path);
    }
    let mut transcript = Journal(journal.0.clone()).load();
    transcript.project(90, 6);
    transcript.scroll(-100, 90, 6);
    // Reading from the top keeps the original group open rather than changing
    // its layout at the moment the reader leaves follow mode.
    let before = text(&transcript.project(90, 6));
    let anchor = transcript.reading_anchor().cloned();
    let boundary = journal.0.len();
    journal.push(
        7,
        "main",
        json!({"type":"assistant","text":"Another inspection."}),
    );
    for path in ["new-first.txt", "new-second.txt", "new-third.txt"] {
        journal.read(7, "main", path);
    }
    let newer = journal.0[boundary..].to_vec();
    for entry in newer.iter().cloned() {
        transcript.apply(entry, None);
    }
    assert_eq!(before, text(&transcript.project(90, 6)));
    assert_eq!(anchor.as_ref(), transcript.reading_anchor());
    let cursor = journal.0[boundary - 1].id;
    transcript.reading.as_mut().unwrap().newer = Some(cursor);
    transcript.apply_forward_page(
        HistoryRequest::Newer(cursor),
        kuku_client::ForwardHistoryPage {
            page: HistoryPage {
                entries: newer,
                rewind_groups: Vec::new(),
                next_before: None,
            },
            next_after: None,
        },
    );
    assert_eq!(before, text(&transcript.project(90, 6)));
    assert_eq!(anchor.as_ref(), transcript.reading_anchor());
    let resized = text(&transcript.project(90, 60));
    assert!(
        resized.contains("Read first.txt") && !resized.contains("Read new-first.txt"),
        "{resized}"
    );
    assert_eq!(anchor.as_ref(), transcript.reading_anchor());
    transcript.show_latest();
    let latest = text(&transcript.project(90, 60));
    assert!(
        !latest.contains("Read first.txt") && latest.contains("Read new-first.txt"),
        "{latest}"
    );
}

#[test]
fn inspecting_an_auto_open_member_pins_its_group_until_the_user_folds_it() {
    let mut journal = Journal::default();
    for path in [
        "inspect-first.txt",
        "inspect-second.txt",
        "inspect-third.txt",
    ] {
        journal.read(7, "main", path);
    }
    let mut transcript = Journal(journal.0.clone()).load();
    let auto = transcript.project(100, 60);
    let group = transcript
        .detail_at(line(&auto, "Read 3 files"))
        .unwrap()
        .clone();
    let member = transcript
        .detail_at(line(&auto, "Read inspect-first.txt"))
        .unwrap()
        .clone();
    assert!(transcript.toggle_detail(&member));
    assert!(text(&transcript.project(100, 60)).contains("contents for inspect-first.txt"));

    let boundary = journal.0.len();
    journal.push(
        7,
        "main",
        json!({"type":"assistant","text":"Another batch next."}),
    );
    for path in ["later-first.txt", "later-second.txt", "later-third.txt"] {
        journal.read(7, "main", path);
    }
    for entry in journal.0[boundary..].iter().cloned() {
        transcript.apply(entry, None);
    }
    let both = text(&transcript.project(100, 60));
    assert!(
        both.contains("contents for inspect-first.txt") && both.contains("Read later-first.txt"),
        "{both}"
    );
    assert!(transcript.toggle_detail(&group));
    let folded = text(&transcript.project(100, 60));
    assert!(
        !folded.contains("contents for inspect-first.txt")
            && folded.contains("Read later-first.txt"),
        "{folded}"
    );
    // Choosing the existing hidden inspection reopens its parent without
    // closing the inspection, so explicit group folds still take precedence.
    assert!(transcript.toggle_detail(&member));
    assert!(text(&transcript.project(100, 60)).contains("contents for inspect-first.txt"));
}

#[test]
fn a_group_fold_remains_explicit_after_its_first_member_is_evicted_and_reloaded() {
    let mut journal = Journal::default();
    for index in 0..3 {
        journal.read(7, "main", &format!("read-{index}.txt"));
    }
    let mut transcript = Journal(journal.0.clone()).load();
    let original = transcript.project(100, 60);
    let initial = transcript
        .detail_at(line(&original, "Read 3 files"))
        .unwrap()
        .clone();
    assert!(transcript.toggle_detail(&initial));
    for index in 3..MAX_RECORDS + 64 {
        let boundary = journal.0.len();
        journal.read(7, "main", &format!("read-{index}.txt"));
        for entry in journal.0[boundary..].iter().cloned() {
            transcript.apply(entry, None);
        }
    }
    assert!(!transcript.recent.contains(&initial.record));
    assert_eq!(MAX_RECORDS, transcript.recent.records.len());
    assert_eq!(
        Some(false),
        transcript
            .detail_items
            .group_choice(&transcript.recent.records)
            .map(|choice| choice.open)
    );
    transcript.project(100, 60);
    assert!(
        transcript
            .detail_items
            .visible
            .iter()
            .any(|item| item.key.kind == super::DetailKind::Group && !item.open)
    );
    // A reconnect loads a narrower latest page. Move the alias onto the
    // surviving suffix rather than losing the manual fold at that boundary.
    let entries = journal.0[journal.0.len() - 6..].to_vec();
    transcript.replace_recent(HistoryPage {
        next_before: Some(entries[0].id),
        entries,
        rewind_groups: Vec::new(),
    });
    transcript.project(100, 60);
    let group = transcript
        .detail_items
        .visible
        .iter()
        .find(|item| item.key.kind == super::DetailKind::Group && !item.open)
        .expect("reloaded suffix preserves the manual fold")
        .key
        .clone();
    assert!(transcript.toggle_detail(&group));
    transcript.project(100, 60);
    assert!(
        transcript
            .detail_items
            .visible
            .iter()
            .any(|item| item.key.kind == super::DetailKind::Group && item.open)
    );
    assert!(
        transcript
            .detail_items
            .groups
            .iter()
            .map(|choice| choice.keys.len())
            .sum::<usize>()
            <= MAX_RECORDS
    );
}

#[test]
fn paging_a_long_group_keeps_its_frozen_auto_expansion_after_trimming_the_tail() {
    let mut journal = Journal::default();
    for index in 0..MAX_RECORDS * 2 + 5 {
        journal.read(7, "main", &format!("long-{index}.txt"));
    }
    let recent = journal.0.split_off(MAX_RECORDS * 4);
    let middle = journal.0.split_off(MAX_RECORDS * 2);
    let cursor = recent[0].id;
    let older_cursor = middle[0].id;
    let mut transcript = Transcript::default();
    transcript.replace_recent(HistoryPage {
        entries: recent,
        next_before: Some(cursor),
        rewind_groups: Vec::new(),
    });
    assert_eq!(
        Some(HistoryRequest::Older(cursor)),
        transcript.scroll(-1000, 100, 5)
    );
    transcript.apply_page(
        HistoryRequest::Older(cursor),
        HistoryPage {
            entries: middle,
            next_before: Some(older_cursor),
            rewind_groups: Vec::new(),
        },
    );
    assert_eq!(
        Some(HistoryRequest::Older(older_cursor)),
        transcript.scroll(-1000, 100, 5)
    );
    transcript.scroll(2, 100, 5);
    let before = text(&transcript.project(100, 5));
    let anchor = transcript.reading_anchor().cloned();
    let activity = transcript
        .reading
        .as_ref()
        .unwrap()
        .window
        .latest_activity()
        .unwrap()
        .clone();
    transcript.apply_page(
        HistoryRequest::Older(older_cursor),
        HistoryPage {
            entries: journal.0,
            next_before: None,
            rewind_groups: Vec::new(),
        },
    );
    let window = &transcript.reading.as_ref().unwrap().window;
    assert!(!window.contains(&activity));
    assert!(
        window
            .latest_activity()
            .is_some_and(|key| window.contains(key))
    );
    assert_eq!(before, text(&transcript.project(100, 5)));
    assert_eq!(anchor.as_ref(), transcript.reading_anchor());
}

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
    collapse_auto_group(&mut transcript);
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
    collapse_auto_group(&mut transcript);
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
        assert!(
            row.spans
                .iter()
                .all(|span| span.style.bg == crate::theme::transcript_background().bg)
        );
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
fn loading_an_earlier_batch_prefix_keeps_the_reader_at_the_visible_calls() {
    let mut journal = Journal::default();
    let edit = |journal: &mut Journal, path: &str| {
        journal.tool(
            "edit_file",
            json!({"path":path}),
            json!({"status":"ok","structured":{"kind":"file_edit","path":path,"change":{
                "added_lines":1,"removed_lines":1,"preview":"-before\n+after","preview_line_numbers":[10,10],
            }}}),
        );
    };
    for index in 0..20 {
        edit(&mut journal, &format!("earlier-{index}.txt"));
    }
    let boundary = journal.0.len();
    journal.read(7, "main", "notes.txt");
    edit(&mut journal, "currently-visible.txt");
    journal.push(8, "main", json!({"type":"user","text":"Show the results."}));
    let recent = journal.0.split_off(boundary);
    let before = recent[0].id;
    let mut transcript = Transcript::default();
    transcript.replace_recent(HistoryPage {
        entries: recent,
        next_before: Some(before),
        rewind_groups: Vec::new(),
    });

    let (width, height) = (90, 12);
    let request = transcript.scroll(-1, width, height).unwrap();
    let visible = transcript.project(width, height);
    let edit_row = line(&visible, "currently-visible.txt");
    assert!(edit_row <= 2, "{}", text(&visible));
    transcript.apply_page(
        request,
        HistoryPage {
            entries: journal.0,
            next_before: None,
            rewind_groups: Vec::new(),
        },
    );
    let joined = transcript.project(width, height);
    assert_eq!(
        0,
        line(&joined, "currently-visible.txt"),
        "{}",
        text(&joined)
    );
    assert!(text(&joined).contains("Show the results."));

    transcript.scroll(-3, width, height);
    assert!(text(&transcript.project(width, height)).contains("earlier-19.txt"));
}

#[test]
fn loading_earlier_actions_keeps_a_folded_read_batch_visible() {
    let mut journal = Journal::default();
    journal.tool(
        "run_command",
        json!({"program":"check"}),
        json!({"status":"error","summary":"check failed"}),
    );
    let boundary = journal.0.len();
    for index in 0..4 {
        journal.read(7, "main", &format!("reference-{index}.txt"));
    }
    journal.push(8, "main", json!({"type":"user","text":"Show the results."}));
    let recent = journal.0.split_off(boundary);
    let mut transcript = Transcript::default();
    transcript.replace_recent(HistoryPage {
        next_before: Some(recent[0].id),
        entries: recent,
        rewind_groups: Vec::new(),
    });

    let (width, height) = (90, 14);
    let request = transcript.scroll(-1, width, height).unwrap();
    assert!(text(&transcript.project(width, height)).contains("Read 4 files"));
    transcript.apply_page(
        request,
        HistoryPage {
            entries: journal.0,
            next_before: None,
            rewind_groups: Vec::new(),
        },
    );
    let joined = text(&transcript.project(width, height));
    assert!(
        joined.lines().next().unwrap().contains("4 file reads"),
        "{joined}"
    );
    assert!(joined.contains("Show the results."), "{joined}");
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
    collapse_auto_group(&mut transcript);
    let view = transcript.project(82, 32);
    let collapsed = text(&view);
    assert_eq!(1, collapsed.matches("2 file reads").count());
    for label in ["2 file reads", "Result limit reached"] {
        let row = &view.lines[line(&view, label)];
        assert!(row.style.bg.is_none());
        assert!(
            row.spans
                .iter()
                .all(|span| span.style.bg == crate::theme::transcript_background().bg)
        );
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
    assert!(
        viewport
            .lines()
            .next()
            .unwrap()
            .contains("result not loaded ×2")
    );
    let request = HistoryRequest::Newer(2);
    transcript.reading.as_mut().unwrap().newer = Some(2);
    transcript.apply_forward_page(
        request,
        kuku_client::ForwardHistoryPage {
            page: HistoryPage {
                entries: missing,
                rewind_groups: Vec::new(),
                next_before: None,
            },
            next_after: None,
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
    assert!(!transcript.toggle_details());
    assert!(transcript.toggle_details());
    assert!(!transcript.toggle_details());
    let collapsed = text(&transcript.project(80, 20));
    assert!(collapsed.contains("second.txt") && collapsed.contains("third.txt"));
    assert!(!collapsed.contains("first.txt"));
    assert!(transcript.toggle_details());
    assert!(text(&transcript.project(80, 20)).contains("Read first.txt"));
}
