use crate::transcript::tests::card_text;
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
                "kind":"workspace_script_result",
                "value":{"verified":true,"agentsAndSkills":"Line one\nLine two","cliFiles":["main.rs","lib.rs"]},
                "timed_out":false,"cleanup_incomplete":true,"not_dispatched":1,
                "actions":[
                    {"id":"write","tool":"write_file","status":"ok","summary":"written","truncated":false,
                     "result":{"kind":"file_write","path":"report.txt","canonical_path":"/workspace/report.txt",
                               "commit_state":"committed","content_hash_before":null,"content_hash_after":"now","bytes_written":14}},
                    {"id":"append","tool":"append_output","status":"unknown","summary":"","truncated":false,"result":null},
                    {"id":"file","tool":"read_file","status":"ok",
                     "summary":"read docs/en/reference/tools.md, lines 1-200 of 231","truncated":false,
                     "result":{"kind":"file_content","path":"docs/en/reference/tools.md","start_line":1,"line_count":200,"total_lines":231}},
                    {"id":"find","tool":"find_files","status":"ok","summary":"found two entries somewhere","truncated":false,
                     "result":{"kind":"file_list","path":"crates/kuku-cli/src","pattern":"**/*.rs","entry_count":2}},
                    {"id":"read","tool":"read_output","status":"ok","summary":"Output read","truncated":false,
                     "result":{"pages":5}},
                    {"id":"report","tool":"finish_output","status":"ok","summary":"report retained","truncated":false,
                     "result":{"output_ref":{"id":"retained-report","offset":0,"bytes":14}}}
                ]
            }
        })).unwrap(),
        None,
    );
    let view = transcript.project(100, 28);
    let title_row = view
        .lines
        .iter()
        .position(|line| card_text(&line.to_string()) == "Script")
        .unwrap();
    let detail = transcript.detail_at(title_row).unwrap().clone();
    assert!(transcript.toggle_in_view(&detail, 100, 28));
    let view = transcript.project(100, 28);
    let rendered = view
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    for evidence in [
        "│   │ Status · partial",
        "Actions · 9/11 completed",
        "│   │   │ Find crates/kuku-cli/src · **/*.rs · 2 entries",
        "Output · 3 fields",
        "verified · true",
        "agentsAndSkills · Text · 2 lines",
        "cliFiles · 2 items",
        "1 file change",
        "Added    report.txt",
        "effects unknown",
        "stopped before dispatch",
    ] {
        assert!(rendered.contains(evidence), "{evidence}\n{rendered}");
    }
    let read = view
        .lines
        .iter()
        .find(|line| line.to_string().contains("docs/en/reference/tools.md"))
        .unwrap();
    for (field, color) in [
        ("Read ", Some(crate::theme::ACCENT)),
        (
            "docs/en/reference/tools.md",
            Some(crate::theme::SURFACE_TEXT),
        ),
        ("lines ", Some(crate::theme::MUTED)),
        ("1–200", Some(crate::theme::USAGE)),
        (" of ", Some(crate::theme::MUTED)),
        ("231", Some(crate::theme::USAGE)),
    ] {
        assert!(read
            .spans
            .iter()
            .any(|span| span.content.contains(field) && span.style.fg == color));
    }
    for (field, spans) in [
        (
            "agentsAndSkills",
            [
                ("agentsAndSkills", crate::theme::CODE_NAME),
                ("Text", crate::theme::MODEL),
                ("2", crate::theme::USAGE),
                (" lines", crate::theme::MUTED),
            ],
        ),
        (
            "cliFiles",
            [
                ("cliFiles", crate::theme::CODE_NAME),
                (" · ", crate::theme::MUTED),
                ("2", crate::theme::USAGE),
                (" items", crate::theme::MUTED),
            ],
        ),
    ] {
        let row = view
            .lines
            .iter()
            .find(|line| line.to_string().contains(field))
            .unwrap();
        for (label, color) in spans {
            assert!(
                row.spans
                    .iter()
                    .any(|span| span.content == label && span.style.fg == Some(color)),
                "{field} {label}"
            );
        }
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
    assert_eq!(4, message.outputs.len());
    assert_eq!(
        Some("retained-report"),
        message.outputs[2].output_id.as_deref()
    );
}

#[test]
fn closed_script_card_previews_an_action_and_the_return_shape() {
    let mut transcript = Transcript::default();
    for event in [
        json!({"id":1,"agent":"main","turn":1,"type":"tool_call","tool":"run_js","tool_call_id":"check",
            "args":{"title":"Check workspace","code":"return await verify();"}}),
        json!({"id":2,"agent":"main","turn":1,"type":"tool_result","tool_call_id":"check","status":"ok",
        "structured":{"kind":"workspace_script_result","timed_out":false,
            "cleanup_incomplete":false,"not_dispatched":0,
            "value":{"verified":true},
            "actions":[
                {"id":"command","tool":"run_command","status":"ok","summary":"completed","truncated":false,
                 "result":command_facts("verify", "log", 8, "private command output")},
                {"id":"read","tool":"read_file","status":"ok","summary":"read guide.md","truncated":false,
                 "result":{"kind":"file_content","path":"guide.md","start_line":1,"line_count":3,"total_lines":3}}
            ]}}),
    ] {
        transcript.apply(serde_json::from_value(event).unwrap(), None);
    }
    let view = transcript.project(96, 20);
    let screen = view
        .lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    for row in [
        "Status · ok · 2/2 actions completed",
        "Run verify",
        "Output · 1 field",
    ] {
        assert!(screen.contains(row), "{screen}");
    }
    for row in [
        "guide.md",
        "private command output",
        "return await verify()",
        "verified · true",
    ] {
        assert!(!screen.contains(row), "{screen}");
    }
    let title = view
        .lines
        .iter()
        .position(|line| line.to_string().contains("Script   Check workspace"))
        .unwrap();
    let body = view
        .lines
        .iter()
        .position(|line| line.to_string().contains("Run verify"))
        .unwrap();
    assert_eq!(transcript.detail_at(title), transcript.detail_at(body));
    let action = transcript.detail_at(body).unwrap().clone();
    assert!(transcript.toggle_in_view(&action, 96, 20));
    assert!(transcript
        .project(96, 20)
        .lines
        .iter()
        .any(|line| line.to_string().contains("Input · JavaScript")));
}

#[test]
fn script_details_show_source_inside_the_tool_card() {
    let mut transcript = Transcript::default();
    for event in [
        json!({"id":1,"agent":"main","turn":1,"type":"tool_call","tool":"run_js","tool_call_id":"script",
            "args":{"title":"Save the report","code":"const marker = '```';\nconst first = 1;\nconst second = 2;\nreturn { marker };"}}),
        json!({"id":2,"agent":"main","turn":1,"type":"tool_result","tool_call_id":"script","status":"ok",
            "structured":{"kind":"workspace_script_result","actions":[],"value":null,"timed_out":false,
                "cleanup_incomplete":false,"not_dispatched":0,"output_ref":{"id":"saved","offset":0,"bytes":16000}}}),
    ] {
        transcript.apply(serde_json::from_value(event).unwrap(), None);
    }
    let view = transcript.project(80, 20);
    let label = "Script   Save the report";
    let index = view
        .lines
        .iter()
        .position(|line| line.to_string().contains(label))
        .unwrap();
    assert!(!view
        .lines
        .iter()
        .any(|line| line.to_string().contains("const marker")));
    assert!(view
        .lines
        .iter()
        .any(|line| line.to_string().contains("Output · Saved JSON")));
    let heading = view.lines[index].clone();
    for (text, color) in [
        ("Script", crate::theme::MODEL),
        ("Save the report", crate::theme::SURFACE_TEXT),
    ] {
        assert!(heading
            .spans
            .iter()
            .any(|span| span.content.trim() == text && span.style.fg == Some(color)));
    }
    let detail = transcript.detail_at(index).unwrap().clone();
    assert!(transcript.toggle_in_view(&detail, 80, 20));
    let view = transcript.project(80, 20);
    assert_eq!(heading, view.lines[index]);
    assert!(view.lines[index + 1]
        .to_string()
        .contains("│   │ Status · ok"));
    let source = view
        .lines
        .iter()
        .position(|line| line.to_string().contains("Input · JavaScript"))
        .unwrap();
    assert_eq!(source, index + 2);
    assert!(view.lines[source].to_string().contains("4 lines"));
    assert!(view.lines[source + 1]
        .to_string()
        .contains("│   │   │ const marker"));
    assert!(!view
        .lines
        .iter()
        .any(|line| line.to_string().contains("return { marker };")));
    let source_key = transcript.detail_at(source).unwrap().clone();
    assert!(transcript.toggle_in_view(&source_key, 80, 20));
    let view = transcript.project(80, 20);
    let text: Vec<_> = view.lines.iter().map(ToString::to_string).collect();
    assert!(text[index].contains(label));
    let source = text
        .iter()
        .position(|line| line.contains("│   │ Input · JavaScript"))
        .unwrap();
    assert_eq!(source, index + 2);
    assert!(view.lines[source].to_string().contains("4 lines"));
    assert!(text[source + 1].contains("│   │   │ const marker"));
    assert!(transcript.detail_at(source + 1).is_none());
    assert_eq!(
        view.lines[index].spans[0].style.bg,
        view.lines[source].spans[0].style.bg
    );
    assert_eq!(
        1,
        text.iter()
            .filter(|line| line.contains("│   │ Input · JavaScript"))
            .count()
    );
    assert!(text
        .iter()
        .any(|line| line.contains("const marker = '```';")));
    assert!(text.iter().any(|line| line.contains("return { marker };")));
    assert!(!text.iter().any(|line| line.contains("Requested arguments")));
}

#[test]
fn creation_and_overwrite_previews_share_the_same_diff_rules() {
    let created = super::FileChange::from_result(
        &json!({
            "kind": "file_write",
            "path": "guide.md",
            "created": true,
            "line_count": 2,
            "raw_text_after": "# Usage\r\nTry `/resume`.\r\n",
        }),
        false,
    )
    .unwrap();
    let overwritten = super::FileChange::from_result(
        &json!({
            "kind": "file_write",
            "path": "guide.md",
            "created": false,
            "line_count": 2,
            "change": {
                "added_lines": 1, "removed_lines": 1,
                "preview": "-Try `/resume`.\n+Open a session.",
                "preview_line_numbers": [2, 2],
            }
        }),
        false,
    )
    .unwrap();
    for (change, heading, body) in [
        (created, "Added    guide.md (+2 -0)", "│ 2 + Try `/resume`."),
        (
            overwritten,
            "Edited   guide.md (+1 -1)",
            "│ 2 + Open a session.",
        ),
    ] {
        let row = super::file_change::compact_change_row(&change, "guide.md", 62);
        assert_eq!(row.line.to_string(), heading);
        for rows in [change.compact_preview(62), change.expanded_rows(62)] {
            let row = rows
                .iter()
                .find(|row| row.line.to_string().contains(body))
                .unwrap();
            assert!(row
                .line
                .spans
                .iter()
                .any(|span| span.content.starts_with("│")
                    && span.style.fg == Some(crate::theme::TURN_RULE)));
            assert!(row
                .line
                .spans
                .iter()
                .any(|span| body.ends_with(span.content.as_ref())
                    && span.style.fg == Some(crate::theme::SUCCESS)));
        }
    }
}

#[test]
fn omitted_diff_preview_still_shows_the_recorded_line() {
    let change = super::FileChange::from_result(
        &json!({"kind":"file_edit","change":{
            "added_lines": 1, "removed_lines": 2, "first_changed_line": 37,
        }}),
        false,
    )
    .unwrap();
    let row = super::file_change::compact_change_row(&change, "guide.md", 72);
    assert_eq!("Edited   guide.md (+1 -2) · line 37", row.line.to_string());
    assert!(change.compact_preview(72).is_empty());
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
    assert!(rendered.contains("check · timed out"));
    assert!(rendered.contains("command timed out; output truncated"));
    assert!(rendered.contains("stdout:\nOutput: from the process"));
    assert!(
        rendered.contains(&format!(
            "stderr:\ntry fewer inputs\nView output\n{warning}"
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
    assert!(rendered.contains("Read src/lib.rs · lines 17–21 of 30 · cached · truncated"));
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
            json!({"status":"ok", "structured":{"kind":"file_edit", "path":"notes.txt", "replacement_count":1, "raw_text_after":"new line", "change":{"added_lines":1,"removed_lines":2,"preview":"-first\n-second\n+new line","preview_line_numbers":[17,18,17]}}}),
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
    let compact = transcript.project(72, 32);
    let rows: Vec<_> = compact.lines.iter().map(ToString::to_string).collect();
    let write_row = rows
        .iter()
        .position(|line| line.contains("Wrote    notes.txt"))
        .unwrap();
    let edit_row = rows
        .iter()
        .position(|line| line.contains("Edited   notes.txt"))
        .unwrap();
    assert!(write_row < edit_row);
    let compact_text = rows.join("\n");
    assert!(compact_text.contains("│ 17 - first") && compact_text.contains("│ 17 + new line"));
    assert!(compact_text.contains("18 - second") && !compact_text.contains("sentinel"));
    let write_key = transcript.detail_at(write_row).unwrap().clone();
    let edit_key = transcript.detail_at(edit_row).unwrap().clone();
    assert!(transcript.toggle_detail(&write_key));
    assert!(transcript.toggle_detail(&edit_key));

    let view = transcript.project(90, 40);
    let rendered = view
        .lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    for fact in [
        "Wrote    notes.txt",
        "saved UTF-8 中",
        "actual␛line",
        "(2 lines)",
        "Edited   notes.txt",
        "17 - first",
        "18 - second",
        "17 + new line",
        "(+1 -2)",
        "No permission for failed.txt",
    ] {
        assert!(rendered.contains(fact), "missing {fact}\n{rendered}");
    }
    assert!(!rendered.contains("incorrect preview") && !rendered.contains("(3 lines)"));
    assert!(!rendered.contains("Requested arguments · edit_file"));
    assert!(!rendered.contains("sentinel new"));
    assert_eq!("-first\n-second\n+new line", transcript.tools()[1].diff);
    assert!(transcript.toggle_detail(&edit_key));
    let folded = transcript.project(90, 40);
    assert!(folded
        .lines
        .iter()
        .any(|line| line.to_string().contains("Edited   notes.txt")));
    assert!(folded
        .lines
        .iter()
        .any(|line| line.to_string().contains("17 - first")));
    assert!(folded
        .lines
        .iter()
        .any(|line| line.to_string().contains("18 - second")));
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
        .position(|line| card_text(line).starts_with(r#"check-file ["file with spaces"]"#))
        .unwrap();
    assert!(card_text(lines[heading]).ends_with("ok · exit 0 · 180ms"));
    for (text, tone) in [
        ("ok", crate::theme::TranscriptTone::Success),
        ("0", crate::theme::TranscriptTone::Success),
        ("180", crate::theme::TranscriptTone::Value),
        ("ms", crate::theme::TranscriptTone::Label),
    ] {
        assert!(
            view.lines[heading]
                .spans
                .iter()
                .any(|span| span.content == text && span.style.fg == Some(tone.color())),
            "{text}"
        );
    }
    assert_eq!(card_text(lines[heading + 1]), "GONE");
    let key = transcript.detail_at(heading).unwrap().clone();
    assert!(transcript.toggle_detail(&key));
    let inspected = transcript.project(80, 24);
    for label in ["View output", "View arguments"] {
        let link = inspected
            .lines
            .iter()
            .find(|line| line.to_string().contains(label))
            .unwrap();
        for span in &link.spans {
            assert_eq!(
                span.content == label,
                span.style
                    .add_modifier
                    .contains(ratatui::style::Modifier::UNDERLINED)
            );
        }
    }
    let inspected: Vec<_> = inspected.lines.iter().map(ToString::to_string).collect();
    assert_eq!(
        1,
        inspected
            .iter()
            .filter(|line| line.contains("check-file"))
            .count()
    );
    assert!(inspected.iter().any(|line| line.contains("View output")));
    assert!(inspected
        .iter()
        .any(|line| line.contains("│   │ Output · 1 line")));
    assert!(inspected.iter().any(|line| line.contains("│   │ 1 GONE")));
    assert!(!inspected
        .iter()
        .any(|line| line.contains("Requested arguments")));
    let args_row = inspected
        .iter()
        .position(|line| line.contains("View arguments"))
        .unwrap();
    let args = transcript.detail_at(args_row).unwrap().clone();
    assert!(transcript.toggle_in_view(&args, 80, 24));
    let args_view = transcript.project(80, 24);
    let args_text = args_view
        .lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(args_text.contains("Hide arguments"));
    assert!(args_text.contains("Program    check-file"));
    assert!(args_text.contains(r#"Arguments  "file with spaces""#));
    assert!(args_text.contains("Directory"));
    let output = transcript
        .detail_at(
            args_text
                .lines()
                .position(|line| line.contains("View output"))
                .unwrap(),
        )
        .unwrap()
        .clone();
    assert!(transcript.detail_outputs(&output).is_none());
    assert!(matches!(
        transcript.activate_inline_output(&output, 1, 80, 24),
        Some(crate::transcript::InlineOutputEffect::Read { id: 1, output, offset: 0, limit: 4096 }) if output == "saved"
    ));
    let first_page = "page one\npartial ";
    let second_page = "continued\npage two";
    let second_offset = first_page.len() as u64;
    let page = |offset, text: &str| kuku_client::OutputChunk {
        output_id: "saved".into(),
        offset,
        next_offset: offset + text.len() as u64,
        total_bytes: second_offset + second_page.len() as u64,
        segments: vec![kuku_client::OutputSegment {
            stream: kuku_client::OutputStream::Stdout,
            offset,
            bytes: text.len(),
            raw_bytes: None,
            text: text.to_owned(),
        }],
        summary: Vec::new(),
        has_more: offset == 0,
        complete: true,
        capture_truncated: false,
        cleanup_incomplete: false,
        filtered: false,
        lossy_utf8: false,
    };
    transcript.apply_inline_output(1, Ok(page(0, first_page)));
    let first = transcript
        .project(80, 24)
        .lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(first.contains("Hide arguments") && first.contains("Hide output"));
    assert!(
        first.contains("│   │ 1 page one")
            && first.contains("│   │ 2 partial ")
            && !first.contains("GONE")
    );
    transcript.show_latest();
    let following = transcript.project(80, 24);
    let next = transcript
        .detail_at(
            following
                .lines
                .iter()
                .position(|line| line.to_string().contains("Next output page"))
                .unwrap(),
        )
        .unwrap()
        .clone();
    assert!(matches!(
        transcript.activate_inline_output(&next, 2, 80, 24),
        Some(crate::transcript::InlineOutputEffect::Read {
            id: 2,
            offset,
            ..
        }) if offset == second_offset
    ));
    assert!(transcript.is_reading());
    transcript.apply_inline_output(2, Ok(page(second_offset, second_page)));
    let second = transcript
        .project(80, 24)
        .lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        second.contains("│   │ 2 continued")
            && second.contains("│   │ 3 page two")
            && !second.contains("page one")
    );
    assert!(second.contains("Previous output page") && !second.contains("Next output page"));
    let close = transcript
        .detail_at(
            second
                .lines()
                .position(|line| line.contains("Hide output"))
                .unwrap(),
        )
        .unwrap()
        .clone();
    assert!(matches!(
        transcript.activate_inline_output(&close, 3, 80, 24),
        Some(crate::transcript::InlineOutputEffect::Render)
    ));
    let collapsed = transcript
        .project(80, 24)
        .lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        collapsed.contains("View output")
            && collapsed.contains("GONE")
            && !collapsed.contains("page two")
    );
    assert!(!screen.contains("stdout") && !screen.contains("View output") && !screen.contains('␍'));
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

#[test]
fn command_card_labels_selected_evidence_and_exposes_its_capture() {
    let mut transcript = Transcript::default();
    let preview = "Output summary (cargo-humans):\nerror[E0308]: mismatched types";
    let mut facts = command_facts("cargo", "saved", 4000, preview);
    facts["invocation"]["args"] = json!(["check"]);
    facts["output_summary"] = json!({
        "kind":"rule", "rule":"cargo-humans",
        "ranges":[{"stream":"stderr","offset":3600,"end_offset":3720}]
    });
    facts["output_preview_truncated"] = json!(true);
    facts["output_filtered"] = json!(true);
    facts["exit_code"] = json!(101);
    transcript.apply(
        serde_json::from_value(json!({
            "id":1, "agent":"main", "turn":1, "type":"tool_result",
            "tool":"run_command", "tool_call_id":"cargo-check", "status":"ok",
            "summary":"command exited 101, summarized output", "model_content":preview,
            "structured":facts
        }))
        .unwrap(),
        None,
    );
    let rendered = transcript
        .project(90, 22)
        .lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("error[E0308]: mismatched types"),
        "{rendered}"
    );
    assert!(
        rendered.contains("Output summary (cargo-humans"),
        "{rendered}"
    );
    assert!(rendered.contains("Raw output retained"), "{rendered}");
    let message = transcript
        .message_info(
            &crate::transcript::MessageId::Tool {
                turn: Some(1),
                call_id: "cargo-check".into(),
            },
            "main",
        )
        .unwrap();
    assert_eq!(message.outputs[0].output_id.as_deref(), Some("saved"));
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
