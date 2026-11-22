use super::{line, text, HistoryPage, Journal, Transcript, TranscriptView};
use serde_json::json;

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
            .filter(|row| row.starts_with("│ Read "))
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
    let screen = transcript.project(80, 22);
    assert!(text(&screen).contains("8 file changes"));
    assert!(text(&screen).contains("Edited   chapter-1.md"));
    assert!(text(&screen).contains("Write denied"));
    let file = transcript
        .detail_at(line(&screen, "Edited   chapter-1.md"))
        .unwrap()
        .clone();
    assert!(transcript.toggle_detail(&file));
    let opened = text(&transcript.project(80, 22));
    assert!(opened.contains("-10 │ before") && opened.contains("+10 │ after"));
    assert!(opened.contains("Edited   chapter-1.md"));
    assert!(opened.contains("Write denied"));
    assert!(transcript.toggle_detail(&file));
    assert_eq!(text(&screen), text(&transcript.project(80, 22)));
}
