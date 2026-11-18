use kuku_client::{HistoryEntry, HistoryPage};
use serde_json::{json, Value};

use super::super::{AnchorLine, HistoryRequest, Transcript, TranscriptView};

#[derive(Default)]
struct Journal(Vec<HistoryEntry>);

impl Journal {
    fn push(&mut self, turn: u64, agent: &str, mut event: Value) {
        event["id"] = json!(self.0.len() as u64 + 1);
        event["turn"] = json!(turn);
        event["agent"] = json!(agent);
        self.0.push(serde_json::from_value(event).unwrap());
    }

    fn tool(&mut self, name: &str, args: Value, mut result: Value) {
        let id = format!("call-{}", self.0.len());
        self.push(
            7,
            "main",
            json!({"type":"tool_call", "tool":name, "tool_call_id":id, "args":args}),
        );
        result["type"] = json!("tool_result");
        result["tool_call_id"] = json!(id);
        self.push(7, "main", result);
    }

    fn read(&mut self, turn: u64, agent: &str, path: &str) {
        let id = format!("read-{}", self.0.len());
        self.push(turn, agent, json!({"type":"tool_call","tool":"read_file","tool_call_id":id,"args":{"path":path,"offset":2}}));
        self.push(turn, agent, json!({"type":"tool_result","tool_call_id":id,"status":"ok","summary":format!("read {path}"),"model_content":format!("2\tcontents for {path}"),"structured":{"kind":"file_content","path":path,"start_line":2,"line_count":1,"total_lines":3}}));
    }

    fn load(self) -> Transcript {
        let mut transcript = Transcript::default();
        transcript.replace_recent(HistoryPage {
            entries: self.0,
            next_before: None,
        });
        transcript
    }
}

fn text(view: &TranscriptView) -> String {
    view.lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn line(view: &TranscriptView, part: &str) -> usize {
    view.lines
        .iter()
        .position(|line| line.to_string().contains(part))
        .unwrap_or_else(|| panic!("missing {part:?}\n{}", text(view)))
}

#[test]
fn mixed_twenty_four_tools_compact_by_operation_and_remain_individually_inspectable() {
    let mut journal = Journal::default();
    for index in 1..=12 {
        journal.read(7, "main", &format!("samples/read-{index:02}.txt"));
    }
    for pattern in ["*.rs", "*.md", "*.toml"] {
        journal.tool(
            "find_files",
            json!({"pattern":pattern}),
            json!({"status":"ok","summary":"found files"}),
        );
    }
    for command in ["check-one", "check-two", "check-three"] {
        journal.tool("run_command", json!({"command":command}), json!({"status":"ok","summary":"exited","structured":{"exit_code":0,"command":command}}));
    }
    journal.tool("run_command", json!({"command":"check-failed"}), json!({"status":"ok","summary":"check failed","model_content":"actual diagnostic","structured":{"exit_code":23}}));
    journal.tool("run_command", json!({"command":"check-warning"}), json!({"status":"ok","summary":"captured stderr","model_content":"actual warning","structured":{"exit_code":0,"stderr_lines":1}}));
    journal.tool("read_file", json!({"path":"missing.txt"}), json!({"status":"error","summary":"File is missing","model_content":"No such file: missing.txt"}));
    journal.tool(
        "ask_user",
        json!({"question":"Continue?"}),
        json!({"status":"ok","summary":"session operation committed"}),
    );
    journal.push(
        7,
        "main",
        json!({"type":"permission","text":"Confirm the next operation"}),
    );
    journal.push(7, "main", json!({"type":"tool_call","tool":"run_command","tool_call_id":"active","args":{"command":"checking now"}}));
    journal.push(7, "main", json!({"type":"tool_call","tool":"read_file","tool_call_id":"requested","args":{"path":"queued.txt"}}));
    let mut transcript = journal.load();
    transcript.live(
        4,
        Some(7),
        None,
        &json!({"type":"tool_start","tool":"run_command","id":"active"}),
    );
    assert_eq!(24, transcript.tools().len());

    let compact = transcript.project(90, 64);
    let rendered = text(&compact);
    assert!(rendered.contains("Read-only activity · 15 calls · ok"));
    assert!(rendered.contains("12 file reads · 3 file searches"));
    assert_eq!(3, rendered.matches("Run command · ok · exit 0").count());
    assert!(rendered.contains("failed · exit 23"));
    for term in [
        "actual diagnostic",
        "ok with warnings",
        "actual warning",
        "No such file",
        "Permission",
        "Confirm the next operation",
        "running",
        "requested",
    ] {
        assert!(rendered.contains(term), "{term} missing\n{rendered}");
    }
    assert!(!rendered.contains("samples/read-12.txt"));
    assert!(transcript.recent.records[..12]
        .iter()
        .all(|record| record.rendered.preview.is_none() && record.rendered.source.is_none()));
    assert_eq!(Some("Ctrl+O show · /details"), compact.tool_hint.as_deref());
    let group = transcript
        .detail_at(line(&compact, "12 file reads · 3 file searches"))
        .unwrap()
        .clone();
    assert!(transcript.toggle_detail(&group));
    let expanded = transcript.project(90, 64);
    assert!(text(&expanded).contains("samples/read-12.txt"));
    assert!(!text(&expanded).contains("contents for samples/read-12.txt"));
    let (record, _, _) = transcript
        .visible_detail_choices()
        .into_iter()
        .find(|(_, _, summary)| summary.contains("samples/read-12.txt"))
        .unwrap();
    transcript.project(36, 2);
    assert!(transcript.toggle_detail(&record));
    let detail = text(&transcript.project(90, 64));
    assert!(detail.contains("Requested arguments") && detail.contains("\"offset\":2"));
    assert!(detail.contains("contents for samples/read-12.txt"));
    assert!(transcript.toggle_detail(&group));
    let compact_again = text(&transcript.project(90, 64));
    assert_eq!(rendered, compact_again);
}

#[test]
fn summaries_do_not_merge_across_prose_turns_addresses_or_missing_metadata() {
    let mut journal = Journal::default();
    journal.read(7, "main", "same.txt");
    journal.read(7, "main", "same.txt");
    journal.push(
        7,
        "main",
        json!({"type":"assistant","text":"I will check a different file."}),
    );
    journal.read(7, "main", "a.txt");
    journal.read(7, "reviewer", "b.txt");
    journal.read(9, "reviewer", "c.txt");
    journal.read(0, "main", "d.txt");
    journal.read(0, "main", "e.txt");
    let mut transcript = journal.load();
    let view = text(&transcript.project(90, 50));
    assert_eq!(1, view.matches("Read file ×2 · ok").count());
    assert!(!view.contains("Read 2 files"));
    for path in ["a.txt", "b.txt", "c.txt", "d.txt", "e.txt"] {
        assert!(view.contains(path), "{path} missing\n{view}");
    }
}

#[test]
fn built_in_actions_use_human_labels_and_extensions_keep_their_names() {
    let mut journal = Journal::default();
    for name in [
        "search_text",
        "search_text",
        "write_file",
        "edit_file",
        "vendor_lookup",
    ] {
        journal.tool(name, json!({"path":"source.txt"}), json!({"status":"ok"}));
    }
    let view = text(&journal.load().project(72, 32));
    for label in [
        "Search text ×2 · ok",
        "Write file · ok",
        "Edit file · ok",
        "vendor_lookup · ok",
    ] {
        assert!(view.contains(label), "{label} missing\n{view}");
    }
}

#[test]
fn missing_read_has_one_default_diagnostic_and_keeps_originals_in_its_details() {
    let mut journal = Journal::default();
    let path = "samples/missing.txt";
    let summary = format!("failed: path not found: {path}");
    let diagnostic = format!("path does not exist: {path}");
    journal.tool(
        "read_file",
        json!({"path":path}),
        json!({"status":"error","summary":summary,"model_content":diagnostic}),
    );
    let mut transcript = journal.load();
    let view = transcript.project(80, 20);
    let compact = text(&view);
    assert!(compact.contains("Read file · error") && compact.contains(&diagnostic));
    assert_eq!(1, compact.matches(path).count());
    assert!(!compact.contains(&summary));

    let key = transcript
        .detail_at(line(&view, "Read file · error"))
        .unwrap()
        .clone();
    assert!(transcript.toggle_detail(&key));
    let inspected = text(&transcript.project(80, 20));
    assert!(inspected.contains("Requested arguments · read_file"));
    assert!(inspected.contains(&summary) && inspected.contains(&diagnostic));
    let raw = &transcript.tools()[0];
    assert_eq!(diagnostic, raw.details);
    assert_eq!(summary, raw.summary);
}

#[test]
fn listing_polls_show_their_recorded_snapshot_and_preserve_the_original_result() {
    let mut journal = Journal::default();
    let sample = json!([
        {"address":"main","status":"running","config":{"preset":null},"permission":null,"question":null},
        {"address":"reviewer","status":"idle","last_output":"Original large reply heading\n\n```rust\nfn main() {}\n```", "pending_inputs":0},
    ]);
    let source = sample.to_string();
    for _ in 0..3 {
        journal.tool("list_agents", json!({}), json!({"status":"ok", "summary":"session operation committed", "structured":sample, "model_content":source}));
    }
    let waiting = json!([
        {"address":"main","status":"running"},
        {"address":"reviewer","status":"waiting","permission":{"id":"request"},"pending_inputs":1},
        {"address":"audit","status":"idle","last_error":"Stopped with an error"},
        {"address":"another","status":"idle"},
    ]);
    journal.tool("list_agents", json!({}), json!({"status":"ok", "summary":"session operation committed", "structured":waiting, "model_content":waiting.to_string()}));
    let mut split_page = Transcript::default();
    split_page.apply(journal.0[1].clone(), None);
    let without_call = text(&split_page.project(90, 10));
    assert!(without_call.contains("2 Agents at check"));
    assert!(!without_call.contains("Original large reply heading"));

    let mut transcript = journal.load();
    let view = transcript.project(108, 28);
    let compact = text(&view);
    for term in [
        "List Agents ×3 · ok",
        "Latest · 2 Agents at check",
        "reviewer (Idle, reply available)",
        "4 Agents at check · 2 need attention",
        "reviewer (Waiting for approval, 1 queued)",
        "audit (Last run failed)",
        "+1 more",
    ] {
        assert!(compact.contains(term), "missing {term}\n{compact}");
    }
    assert!(!compact.contains("Original large reply heading") && !compact.contains("preset"));
    let row = line(&view, "List Agents ×3");
    assert_eq!(
        Some(crate::theme::AGENT_SURFACE),
        view.lines[row].spans[0].style.bg
    );
    let group = transcript.detail_at(row).unwrap().clone();
    assert!(transcript.toggle_detail(&group));
    let expanded = transcript.project(108, 28);
    let member = transcript
        .detail_at(line(&expanded, "List Agents · 2 Agents at check"))
        .unwrap()
        .clone();
    assert!(transcript.toggle_detail(&member));
    let detail = text(&transcript.project(108, 28));
    assert!(
        detail.contains("Raw result · /tools") && detail.contains("Original large reply heading")
    );
    assert_eq!(source, transcript.tools()[1].details);
}

#[test]
fn query_counts_preserve_observed_types_even_when_a_row_was_clipped() {
    let mut journal = Journal::default();
    let source = "[\n{\"kind\":\"message.user\",\"text\":\"Fixture input\"},\n{\"kind\":\"model.error\",\"text\":\"model failed\"},\n{\"kind\":\"runtime.model_retry\"},\n{\"kind\":\"tool.call\",\"args\": ...(truncated)\n]";
    journal.tool(
        "query_session",
        json!({"conversation":"reviewer", "after":16}),
        json!({"status":"ok", "summary":"4 events returned", "model_content":source}),
    );
    journal.tool("query_session", json!({"conversation":"reviewer", "after":22}), json!({"status":"error", "summary":"query_session failed", "model_content":"Session read was denied"}));
    journal.tool(
        "query_session",
        json!({"conversation":"reviewer", "after":22}),
        json!({"status":"ok", "summary":"0 events returned", "model_content":"[]"}),
    );
    let mut transcript = journal.load();
    let compact = transcript.project(86, 30);
    let rendered = text(&compact);
    for term in [
        "4 events matched",
        "Agent reviewer · After #16",
        "1 model error · 1 retry · 1 message",
        "Preview incomplete",
        "Query Session · error",
        "Session read was denied",
        "0 events matched",
    ] {
        assert!(rendered.contains(term), "missing {term}\n{rendered}");
    }
    assert!(!rendered.contains("Fixture input") && !rendered.contains("\"kind\""));
    let call = transcript
        .detail_at(line(&compact, "4 events matched"))
        .unwrap()
        .clone();
    assert!(transcript.toggle_detail(&call));
    let inspected = text(&transcript.project(86, 30));
    assert!(
        inspected.contains("Requested arguments · query_session")
            && inspected.contains("Fixture input")
    );
    assert_eq!(source, transcript.tools()[2].details);
}

#[test]
fn a_full_window_of_large_agent_polls_keeps_default_paint_compact() {
    let mut journal = Journal::default();
    let snapshot = json!([{"address":"reviewer","status":"idle","last_output":"archived Markdown\n".repeat(500),"config":{"instructions":"instructions".repeat(40)}}]);
    let result = json!({"status":"ok", "summary":"session operation committed", "structured":snapshot, "model_content":snapshot.to_string()});
    for _ in 0..super::MAX_RECORDS {
        journal.tool("list_agents", json!({}), result.clone());
    }
    let mut transcript = journal.load();
    for width in [32, 72, 150].into_iter().cycle().take(120) {
        let view = transcript.project(width, 20);
        assert!(view.lines.len() <= 5);
        assert!(text(&view).contains("Agent at check"));
        assert!(!text(&view).contains("archived Markdown"));
    }
    assert!(transcript
        .recent
        .records
        .iter()
        .all(|record| record.rendered.preview.is_none()));
}

#[test]
fn older_page_can_extend_an_open_group_without_moving_the_reader_or_invalidating_its_key() {
    let mut journal = Journal::default();
    for index in 1..=8 {
        journal.read(7, "main", &format!("p{index}.txt"));
    }
    let latest = journal.0.split_off(6);
    let mut transcript = Transcript::default();
    transcript.replace_recent(HistoryPage {
        entries: latest,
        next_before: Some(7),
    });
    assert_eq!(
        Some(HistoryRequest::Older(7)),
        transcript.scroll(-20, 72, 3)
    );
    let partial = transcript.project(72, 3);
    let group = transcript
        .detail_at(line(&partial, "Read 5 files"))
        .unwrap()
        .clone();
    assert!(transcript.toggle_detail(&group));
    transcript.scroll(2, 72, 3);
    let before = text(&transcript.project(72, 3));
    assert!(before.starts_with("  Read file · p5.txt"), "{before}");
    let anchor = transcript.reading.as_ref().unwrap().anchor.clone();
    assert_eq!(AnchorLine::Body(0), anchor.line);

    transcript.apply_page(
        HistoryRequest::Older(7),
        HistoryPage {
            entries: journal.0,
            next_before: None,
        },
    );
    assert_eq!(before, text(&transcript.project(72, 3)));
    assert!(transcript.toggle_detail(&group));
    assert!(text(&transcript.project(72, 3)).starts_with("▸ Read 8 files · ok"));
    assert_eq!(&anchor, &transcript.reading.as_ref().unwrap().anchor);
    assert!(transcript.toggle_detail(&group));
    assert_eq!(before, text(&transcript.project(72, 3)));
}

#[test]
fn a_wrapped_group_caption_is_a_stable_reading_and_pointer_target() {
    let mut journal = Journal::default();
    journal.read(7, "main", "first.txt");
    journal.tool(
        "find_files",
        json!({"pattern":"*.txt"}),
        json!({"status":"ok"}),
    );
    journal.tool(
        "search_text",
        json!({"pattern":"notes"}),
        json!({"status":"ok"}),
    );
    journal.push(
        7,
        "main",
        json!({"type":"assistant","text":"first\n\nsecond\n\nthird\n\nfourth\n\nfifth"}),
    );
    let mut transcript = journal.load();
    transcript.scroll(-100, 37, 3);
    transcript.scroll(2, 37, 3);
    let view = transcript.project(37, 3);
    let caption = view.lines[0].to_string();
    assert!(caption.contains("1 file read · 1 file search"), "{caption}");
    let anchor = transcript.reading.as_ref().unwrap().anchor.clone();
    assert_eq!(AnchorLine::Group(1), anchor.line);
    let key = transcript.detail_at(0).unwrap().clone();
    assert!(transcript.toggle_detail(&key));
    assert_eq!(&anchor, &transcript.reading.as_ref().unwrap().anchor);
    assert_eq!(caption, transcript.project(37, 3).lines[0].to_string());
    assert!(transcript.toggle_detail(&key));
    assert_eq!(caption, transcript.project(37, 3).lines[0].to_string());
}

#[test]
fn live_result_joins_the_group_only_on_commit_and_reconnect_keeps_its_expansion() {
    let mut journal = Journal::default();
    journal.read(7, "main", "first.txt");
    journal.read(7, "main", "second.txt");
    let complete = journal.0.clone();
    let result = journal.0.pop().unwrap();
    let id = result.details["tool_call_id"].as_str().unwrap();
    let mut transcript = journal.load();
    transcript.live(4, Some(7), None, &json!({"type":"tool_end","id":id,"status":"ok","summary":"read second.txt","result":result.details["structured"]}));
    let provisional = text(&transcript.project(80, 20));
    assert!(provisional.contains("second.txt") && !provisional.contains("Read 2 files"));

    transcript.apply(result, Some(4));
    let compact = transcript.project(80, 20);
    let key = transcript
        .detail_at(line(&compact, "Read 2 files · ok"))
        .unwrap()
        .clone();
    assert!(transcript.toggle_detail(&key));
    assert!(text(&transcript.project(80, 20)).contains("Read file · second.txt"));

    transcript.replace_recent(HistoryPage {
        entries: complete,
        next_before: None,
    });
    let restored = text(&transcript.project(80, 20));
    assert!(
        restored.contains("Read file · first.txt") && restored.contains("Read file · second.txt")
    );
    assert_eq!(1, restored.matches("Read 2 files · ok").count());
}

#[test]
fn repeated_global_toggles_follow_the_group_state_before_the_next_frame() {
    let mut journal = Journal::default();
    journal.read(7, "main", "first.txt");
    journal.read(7, "main", "second.txt");
    let mut transcript = journal.load();
    transcript.project(80, 20);
    assert!(transcript.toggle_details());
    assert!(!transcript.toggle_details());
    assert!(!text(&transcript.project(80, 20)).contains("second.txt"));
    assert!(transcript.toggle_details());
    assert!(text(&transcript.project(80, 20)).contains("Read file · second.txt"));
}
