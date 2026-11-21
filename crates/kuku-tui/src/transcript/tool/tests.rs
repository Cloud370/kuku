use serde_json::{json, Value};

use crate::transcript::Transcript;

use super::{Outcome, State, Tool, MAX_DETAIL_BYTES};

#[test]
fn remediation_truncated_edit_preview_is_not_full_inline_output() {
    let fields = json!({
        "status":"ok",
        "structured":{
            "kind":"file_edit",
            "change":{
                "added_lines":1,
                "removed_lines":1,
                "preview":"--- old\n+++ new\n-clipped\n+diff",
                "preview_truncated":true
            }
        }
    });
    let outcome = Outcome::from_fields(fields.as_object().unwrap(), None);
    let tool = Tool {
        state: State::Recorded(outcome),
        ..Tool::default()
    };

    assert!(!tool.view().has_full_output());
}

#[test]
fn command_invocations_display_literal_arguments_and_an_explicit_interpreter() {
    assert_eq!(
        r#"fixture ["","has space","雪&\"quote"]"#,
        super::command::invocation(
            &json!({"program":"fixture","args":["","has space","雪&\"quote"]})
        ),
    );
    assert_eq!(
        "cmd · echo first & echo second",
        super::command::invocation(&json!({"shell":"cmd","command":"echo first & echo second"})),
    );
}

#[test]
fn script_card_reports_completed_effects_and_unknown_actions_from_host_facts() {
    let mut transcript = Transcript::default();
    transcript.apply(
        serde_json::from_value(json!({
            "id":1, "agent":"main", "turn":1, "type":"tool_result",
            "tool_call_id":"script", "status":"ok", "summary":"Everything passed",
            "model_content":"All effects were applied",
            "structured":{
                "kind":"workspace_script_result", "value":{"verified":true},
                "timed_out":false,"cleanup_incomplete":true,"not_dispatched":1,
                "actions":[
                    {"id":"write","tool":"write_file","status":"ok","summary":"written","truncated":false,
                     "result":{"kind":"file_write","path":"report.txt","canonical_path":"/workspace/report.txt",
                               "commit_state":"committed","content_hash_before":null,"content_hash_after":"now","bytes_written":14}},
                    {"id":"append","tool":"append_output","status":"unknown","summary":"","truncated":false,"result":null},
                    {"id":"read","tool":"read_output","status":"ok","summary":"Output read","truncated":false,
                     "result":{"pages":5}},
                    {"id":"report","tool":"finish_output","status":"ok","summary":"report retained","truncated":false,
                     "result":{"output_ref":{"id":"retained-report","offset":0,"bytes":14}}}
                ]
            }
        })).unwrap(),
        None,
    );
    let rendered = transcript
        .project(100, 28)
        .lines
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    for evidence in [
        "Run JavaScript · partial",
        "7/9 host actions completed",
        "1 file change",
        "Wrote report.txt",
        "effects unknown",
        "stopped before dispatch",
    ] {
        assert!(rendered.contains(evidence), "{evidence}\n{rendered}");
    }
    assert!(!rendered.contains("Everything passed"));
    let message = transcript
        .message_info(
            &crate::transcript::MessageId::Tool {
                turn: Some(1),
                call_id: "script".into(),
            },
            "main",
        )
        .unwrap();
    assert_eq!(2, message.outputs.len());
    assert_eq!(
        Some("retained-report"),
        message.outputs[1].output_id.as_deref()
    );
}

#[test]
fn late_live_output_does_not_change_a_full_preview() {
    let mut tool = Tool::default();
    let first = json!({"type":"tool_output", "event":{"stdout":"中".repeat(MAX_DETAIL_BYTES)}});
    assert!(tool.apply_live(&first));
    let details = tool.view().details;
    assert!(details.ends_with("Display limit reached"));
    let extra = json!({"type":"tool_output", "event":{"stdout":"Late output"}});
    assert!(!tool.apply_live(&extra));
    assert_eq!(details, tool.view().details);
}

#[test]
fn command_preview_keeps_stdout_and_warnings_without_the_model_output_reference() {
    let mut transcript = Transcript::default();
    let marker = "Output: saved-run (42 captured bytes; read by merged byte offset).";
    let warning = "Capture truncated at the byte limit";
    let preview =
        "stdout:\nOutput: from the process\r\n系统\r\n64-bit\r\nstderr:\ntry fewer inputs\n";
    let raw = format!("{preview}\n\n{marker} An unrelated model-facing note.");
    let mut facts = command_facts("check", "saved-run", 42, preview);
    facts["termination"] = json!("timed_out");
    facts["exit_code"] = Value::Null;
    facts["quota_truncated"] = json!(true);
    facts["capture_complete"] = json!(false);
    transcript.apply(
        serde_json::from_value(json!({
            "id": 1, "agent": "main", "turn": 1, "type": "tool_result", "tool": "run_command",
            "tool_call_id": "cmd", "status": "error", "summary": "command timed out; output truncated",
            "model_content": raw,
            "structured": facts
        })).unwrap(),
        None,
    );

    let view = transcript.project(96, 18);
    let rendered = view
        .lines
        .iter()
        .map(|line| line.to_string().trim_matches('│').trim().to_owned())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("╭─ check") && rendered.contains("╰─ timed out"));
    assert!(rendered.contains("command timed out; output truncated"));
    assert!(rendered.contains("stdout:\nOutput: from the process"));
    assert!(
        rendered.contains(&format!(
            "stderr:\ntry fewer inputs\nSaved output\n{warning}"
        )),
        "{rendered}"
    );
    assert!(rendered.contains("系统\n") && rendered.contains("64-bit"));
    assert!(!rendered.contains('␍'));
    assert!(!rendered.contains(marker));
    assert_eq!(raw, transcript.tools()[0].details);
    assert_eq!(
        Some("saved-run"),
        transcript.tools()[0].output_id.as_deref()
    );
}

#[test]
fn custom_tool_arguments_and_output_have_bounded_inline_previews() {
    let mut transcript = Transcript::default();
    let args = "中文".repeat(8_000);
    transcript.apply(
        serde_json::from_value(json!({
            "id": 1, "agent": "main", "turn": 1, "type": "tool_call",
            "tool": "custom_fixture", "tool_call_id": "custom", "args": {"key": args}
        }))
        .unwrap(),
        None,
    );
    transcript.apply(serde_json::from_value(json!({
        "id": 2, "agent": "main", "turn": 1, "type": "tool_result", "tool_call_id": "custom",
        "status": "ok", "model_content": format!("first\u{001b}line\n{}later line", "中\n".repeat(50)),
        "output_id": "retained"
    })).unwrap(), None);
    let view = transcript.project(52, 40);
    let plain = view
        .lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(view.lines.len() <= 17);
    assert!(plain.contains("{\"key\":\"中文"));
    assert!(plain.contains("first␛line"));
    assert!(plain.contains('…'));
    assert!(!plain.contains("later line"));
    assert!(!plain.contains('\u{001b}'));
    let tools = transcript.tools();
    assert!(tools[0].details.ends_with("later line"));
    assert_eq!(Some("retained"), tools[0].output_id.as_deref());
}

#[test]
fn reads_separated_by_a_turn_boundary_show_each_path_with_its_actual_line_range() {
    let mut transcript = Transcript::default();
    let raw = "17\talpha\n18\tbeta\n19\tgamma\n20\tdelta\n21\tepsilon";
    for (first, name, path, turn, number) in [
        (1, "read-a", "src/lib.rs", 7, 3),
        (4, "read-b", "src/main.rs", 8, 4),
    ] {
        for (id, entry) in (first..).zip([
            json!({"type": "assistant", "text": "", "thinking": "Read this file"}),
            json!({"type": "tool_call", "tool": "read_file", "tool_call_id": name, "args": {"path": format!("./{path}")}}),
            json!({"type": "tool_result", "tool_call_id": name, "status": "ok", "summary": format!("read {path}, lines 17-21 of 30, results truncated"), "model_content": raw, "truncated": true, "structured": {"kind": "file_content", "path": path, "start_line": 17, "line_count": 5, "total_lines": 30, "cached": true}}),
        ]) {
            let mut entry = entry.as_object().unwrap().clone();
            entry.extend([
                ("id".into(), json!(id)),
                ("agent".into(), json!("main")),
                ("turn".into(), json!(turn)),
                ("turn_number".into(), json!(number)),
            ]);
            transcript.apply(serde_json::from_value(entry.into()).unwrap(), None);
        }
    }

    let view = transcript.project(84, 24);
    let rendered = view
        .lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(1, rendered.matches("Turn 3").count());
    assert_eq!(1, rendered.matches("Turn 4").count());
    assert_eq!(1, rendered.matches("src/lib.rs").count());
    assert_eq!(1, rendered.matches("src/main.rs").count());
    assert!(
        rendered.contains("Read file · ok · src/lib.rs · lines 17–21 of 30 · cached · truncated")
    );
    assert!(rendered.contains("…") && !rendered.contains("Assistant"));
    assert!(rendered.contains("19    gamma") && !rendered.contains("epsilon"));
    assert_eq!(Some("Ctrl+O hide"), view.tool_hint.as_deref());
    assert!(!rendered.contains("Ctrl+O"));
    assert_eq!(raw, transcript.tools()[0].details);

    transcript.toggle_details();
    let collapsed = transcript.project(84, 24);
    assert_eq!(Some("Ctrl+O show"), collapsed.tool_hint.as_deref());
    assert!(!collapsed
        .lines
        .iter()
        .any(|line| line.to_string().contains("alpha")));
    assert!(collapsed
        .lines
        .iter()
        .any(|line| line.to_string().contains("src/lib.rs")));
}

#[test]
fn file_cards_preview_saved_content_and_diff_without_using_requested_text_as_evidence() {
    let mut transcript = Transcript::default();
    for (name, path, args, result) in [
        (
            "write_file",
            "notes.txt",
            json!({"content":"request preview sentinel"}),
            json!({"status":"ok", "structured":{"kind":"file_write", "path":"notes.txt", "line_count":2, "raw_text_after":"saved UTF-8 中\nactual\u{001b}line\n"}}),
        ),
        (
            "edit_file",
            "notes.txt",
            json!({"old_text":"sentinel old", "new_text":"sentinel new"}),
            json!({"status":"ok", "structured":{"kind":"file_edit", "path":"notes.txt", "replacement_count":1, "raw_text_after":"new line", "change":{"added_lines":1,"removed_lines":2,"preview":"-first\n-second\n+new line"}}}),
        ),
        (
            "write_file",
            "failed.txt",
            json!({"content":"failed request sentinel"}),
            json!({"status":"error", "summary":"write denied", "model_content":"No permission for failed.txt", "structured":{"kind":"file_write", "path":"failed.txt", "line_count":3, "raw_text_after":"incorrect preview"}}),
        ),
    ] {
        let id = transcript.recent.records.len() as u64 * 2 + 1;
        let mut call = json!({"id":id,"agent":"main","turn":1,"type":"tool_call","tool":name,"tool_call_id":id.to_string(),"args":args});
        call["args"]["path"] = json!(path);
        let mut result = result;
        result["id"] = json!(id + 1);
        result["agent"] = json!("main");
        result["turn"] = json!(1);
        result["type"] = json!("tool_result");
        result["tool_call_id"] = json!(id.to_string());
        for entry in [call, result] {
            transcript.apply(
                serde_json::from_value::<kuku_client::HistoryEntry>(entry).unwrap(),
                None,
            );
        }
    }
    let view = transcript.project(90, 32);
    let rendered = view
        .lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    for fact in [
        "Write file · ok · notes.txt",
        "saved UTF-8 中",
        "actual␛line",
        "2 lines written",
        "Edit file · ok · notes.txt",
        "-first",
        "-second",
        "+new line",
        "+1 −2",
        "No permission for failed.txt",
    ] {
        assert!(rendered.contains(fact), "missing {fact}\n{rendered}");
    }
    assert!(
        !rendered.contains("sentinel")
            && !rendered.contains("incorrect preview")
            && !rendered.contains("3 lines written")
    );
    let edit_header = view
        .lines
        .iter()
        .position(|line| line.to_string().contains("Edit file · ok · notes.txt"))
        .unwrap();
    assert!(view.lines[edit_header]
        .to_string()
        .starts_with("╭─ Edit file"));
    assert!(view.lines[edit_header..edit_header + 7]
        .iter()
        .all(|line| line.width() == 90));
    assert!(view.lines[edit_header + 6].to_string().contains("╰─ +1 −2"));
    let key = transcript.detail_at(edit_header).unwrap().clone();
    assert!(transcript.toggle_detail(&key));
    let inspected = transcript
        .project(90, 40)
        .lines
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        inspected.contains("Requested arguments · edit_file") && inspected.contains("sentinel new")
    );
}

#[test]
fn short_command_results_show_stdout_without_capture_metadata_or_a_trailing_carriage_return() {
    let mut transcript = Transcript::default();
    transcript.apply(serde_json::from_value(json!({
        "id":0,"agent":"main","turn":1,"type":"tool_call","tool":"run_command","tool_call_id":"cmd",
        "args":{"program":"check-file","args":["file with spaces"],"cwd":"."}
    })).unwrap(),None);
    let raw = "stdout:\nGONE\r\n\n\nOutput: saved (6 captured bytes; read by merged byte offset).";
    let mut facts = command_facts("check-file", "saved", 6, "GONE\r\n");
    facts["invocation"]["args"] = json!(["file with spaces"]);
    transcript.apply(serde_json::from_value(json!({
        "id":1,"agent":"main","turn":1,"type":"tool_result","tool":"run_command","tool_call_id":"cmd",
        "status":"ok","summary":"command exited 0 in 180ms, stdout 1 lines, stderr 0 lines","model_content":raw,
        "structured":facts
    })).unwrap(),None);
    let view = transcript.project(80, 16);
    let screen = view
        .lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    let lines: Vec<_> = screen.lines().collect();
    let heading = lines
        .iter()
        .position(|line| line.starts_with(r#"╭─ check-file ["file with spaces"]"#))
        .unwrap();
    assert!(lines[heading].ends_with('╮') && !lines[heading].contains("exit"));
    assert!(lines[heading + 1].trim_matches('│').trim().is_empty());
    assert_eq!(lines[heading + 2].trim_matches('│').trim(), "GONE");
    assert!(lines[heading + 3].trim_matches('│').trim().is_empty());
    assert!(lines[heading + 4].starts_with("╰─ ok · exit 0 · 180ms"));
    let key = transcript.detail_at(heading).unwrap().clone();
    assert!(transcript.toggle_detail(&key));
    let inspected = transcript.project(80, 24);
    assert!(inspected
        .lines
        .iter()
        .any(|line| line.to_string().contains("check-file")));
    assert!(
        !screen.contains("stdout") && !screen.contains("Saved output") && !screen.contains('␍')
    );
    assert_eq!(transcript.tools()[0].details, raw);
}

#[test]
fn shell_card_distinguishes_stderr_from_the_outer_exit_status() {
    let mut transcript = Transcript::default();
    let mut facts = command_facts("sh", "saved", 0, "stderr:\ncheck failed");
    facts["invocation"] = json!({"mode":"shell","shell":"sh","command":"check; echo done"});
    facts["duration_ms"] = json!(5_900);
    facts["stderr_bytes"] = json!(12);
    facts["stderr_lines"] = json!(1);
    transcript.apply(serde_json::from_value(json!({
        "id":1,"agent":"main","turn":1,"type":"tool_result","tool":"run_command","tool_call_id":"cmd",
        "status":"ok","summary":"shell exited 0","model_content":"check failed","structured":facts
    })).unwrap(),None);
    let view = transcript.project(80, 16);
    assert!(view.lines.iter().any(|line| line
        .to_string()
        .contains("ok · shell exit 0 · stderr · 5.9s")));
}

fn command_facts(program: &str, id: &str, bytes: u64, preview: &str) -> Value {
    json!({
        "kind":"command_result",
        "invocation":{"mode":"program","program":program,"args":[]},
        "cwd":"/workspace",
        "stdin":"null",
        "termination":"exited",
        "started":true,
        "exit_code":0,
        "signal":null,
        "start_error":null,
        "duration_ms":180,
        "stdout_lines":1,
        "stderr_lines":0,
        "stdout_bytes":bytes,
        "stderr_bytes":0,
        "timed_out":false,
        "cancelled":false,
        "output_id":id,
        "output_ref":{"id":id,"offset":0,"bytes":bytes},
        "capture_complete":true,
        "quota_truncated":false,
        "cleanup_incomplete":false,
        "output_filtered":false,
        "preview_truncated":false,
        "lossy_utf8":false,
        "dropped_events":0,
        "summary_lines":[],
        "reported_tests":[],
        "side_effects_observed":false,
        "output_preview":preview,
        "output_preview_truncated":false
    })
}
