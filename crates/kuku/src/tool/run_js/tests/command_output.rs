use std::sync::Arc;

use serde_json::{Value, json};

use crate::OutputRead;
use crate::tool::ToolResultEnvelope;
use crate::tool::builtin::run_command::{CommandCapture, output::OutputLimits, run_command_to};

use super::{
    ScriptCall, ScriptExecution, ScriptOperation, TestHost, WorkspaceScriptLimits, facts, run,
};

#[tokio::test]
async fn command_readers_use_the_authorized_short_capture_and_keep_its_reference() {
    let workspace = Arc::new(tempfile::tempdir().unwrap());
    let capture = workspace.clone();
    let result = run(
        r#"
        const run = await tools.run_command({});
        const lines = [];
        for await (const line of run.stderr.lines()) lines.push(line);
        return {exit:run.exit_code, output:await run.stdout.json(), lines, id:run.output_ref.id,
            stdout:run.stdout, stderr:run.stderr};
        "#,
        TestHost {
            operations: vec![ScriptOperation::RunCommand],
            callback: move |_, execution: Arc<ScriptExecution>| {
                let workspace = workspace.clone();
                async move {
                    let (shell, command) = if cfg!(windows) {
                        ("powershell", r#"[Console]::Out.Write('{"value":7}'); [Console]::Error.Write("note`r`nlast"); exit 7"#)
                    } else {
                        ("sh", r#"printf '{"value":7}'; printf 'note\r\nlast' >&2; exit 7"#)
                    };
                    run_command_to(
                        &json!({"shell":shell, "command":command, "cwd":".", "timeout":5}),
                        workspace.path(),
                        None,
                        Some(execution.cancellation()),
                        CommandCapture::script(
                            workspace.path(),
                            "capture",
                            OutputLimits::default(),
                            execution.limits.host_bytes,
                        ),
                    ).await
                }
            },
        },
        WorkspaceScriptLimits::default(),
    ).await;
    assert_eq!("ok", result.status, "{}", result.model_content);
    let facts = facts(&result);
    assert_eq!(
        json!({"exit":7, "output":{"value":7}, "lines":["note","last"], "id":"capture",
                "stdout":"{\"value\":7}", "stderr":"note\r\nlast"}),
        facts.value
    );
    assert_eq!(1, facts.actions.len());
    let observed = &facts.actions[0].result;
    assert_eq!("capture", observed["output_ref"]["id"]);
    assert_eq!(true, observed["capture_complete"]);
    assert!(observed.get("inline_pipes").is_none());
    assert_eq!("", observed["output_preview"]);
    assert_eq!(1, std::fs::read_dir(capture.path()).unwrap().count());
    let saved = crate::tool::builtin::run_command::output::read_output(
        capture.path(),
        "capture",
        OutputRead::default(),
    )
    .unwrap();
    assert_eq!(21, saved.total_bytes);
}

#[tokio::test]
async fn command_readers_reassemble_interleaved_utf8_across_authorized_pages() {
    let result = run(
        r#"
        const run = await tools.run_command({program:'fixture'});
        const lines = [];
        for await (const line of run.stderr.lines()) lines.push(line);
        return {exit:run.exit_code, text:await run.stdout.text(), lines};
        "#,
        TestHost {
            operations: vec![ScriptOperation::RunCommand, ScriptOperation::ReadOutput],
            callback: |call: ScriptCall, _| async move {
                let (first, last) = (
                    [("stdout", b"hi \xe4".as_slice()), ("stderr", b"war")],
                    [
                        ("stdout", b"\xb8\xad\r\n".as_slice()),
                        ("stderr", b"n\r\nlast"),
                    ],
                );
                match call.op {
                    ScriptOperation::RunCommand => command(8, 10, true, Value::Null),
                    ScriptOperation::ReadOutput => {
                        assert_eq!("capture", call.args["id"]);
                        assert_eq!(65536, call.args["limit"]);
                        let offset = call.args["offset"].as_u64().unwrap();
                        let parts = if offset == 0 { &first } else { &last };
                        output_page(offset, 18, parts)
                    }
                    _ => panic!("unexpected workspace action"),
                }
            },
        },
        WorkspaceScriptLimits::default(),
    )
    .await;
    assert_eq!("ok", result.status, "{}", result.model_content);
    assert_eq!(
        json!({"exit":3, "text":"hi 中\r\n", "lines":["warn","last"]}),
        facts(&result).value
    );
}

#[tokio::test]
async fn command_failures_throw_and_incomplete_or_invalid_captures_reject_readers() {
    let result = run(
        r#"
        async function errorFor(action) {
            try { await action(); } catch (error) {
                return {name:error.name, message:error.message, status:error.status, termination:error.termination, program:error.program, id:error.output_ref?.id};
            }
            throw new Error("expected a rejection");
        }
        const missing = await errorFor(() => tools.run_command({program:'missing'}));
        const cancelled = await errorFor(() => tools.run_command({program:'cancelled'}));
        const partial = await tools.run_command({program:'partial'});
        const invalid = await tools.run_command({program:'invalid'});
        const binary = await tools.run_command({program:'binary'});
        const retained = await tools.run_command({program:'retained'});
        return [missing, cancelled, await errorFor(() => partial.stdout.json()),
            await errorFor(async () => { for await (const line of partial.stdout.lines()) {} }),
            await errorFor(() => invalid.stdout.json()), await errorFor(() => binary.stdout.text()),
            await errorFor(() => retained.stdout.text()),
            {partial:partial.stdout, binary:binary.stdout, retained:retained.stdout, empty:retained.stderr}];
        "#,
        TestHost {
            operations: vec![ScriptOperation::RunCommand],
            callback: |call: ScriptCall, _| async move {
                match call.args["program"].as_str().unwrap() {
                    "missing" => {
                        let mut missing = ToolResultEnvelope::error("program missing", "could not start missing program");
                        missing.structured = Some(json!({
                            "termination":"program_missing", "invocation":{"program":"missing"},
                            "output_ref":{"id":"capture","offset":0,"bytes":0}
                        }));
                        missing
                    }
                    "cancelled" => {
                        let mut cancelled = ToolResultEnvelope::cancelled("command cancelled");
                        cancelled.structured = Some(json!({"termination":"cancelled"}));
                        cancelled
                    }
                    "partial" => command(4, 0, false, Value::Null),
                    "invalid" => command(1, 0, true, json!({"stdout":[125]})),
                    "binary" => command(2, 0, true, json!({"stdout":[0, 255]})),
                    "retained" => command(2500, 0, true, Value::Null),
                    _ => panic!("unsupported program"),
                }
            },
        },
        WorkspaceScriptLimits::default(),
    ).await;
    assert_eq!("ok", result.status, "{}", result.model_content);
    let returned = facts(&result).value;
    assert_eq!("error", returned[0]["status"]);
    assert_eq!("program_missing", returned[0]["termination"]);
    assert_eq!("missing", returned[0]["program"]);
    assert_eq!("capture", returned[0]["id"]);
    assert!(
        returned[0]["message"]
            .as_str()
            .unwrap()
            .contains("could not start missing program")
    );
    assert_eq!("cancelled", returned[1]["status"]);
    assert_eq!("cancelled", returned[1]["termination"]);
    assert!(returned[1].get("program").is_none());
    assert!(
        returned[1]["message"]
            .as_str()
            .unwrap()
            .contains("command cancelled")
    );
    for index in [2, 3] {
        assert!(
            returned[index]["message"]
                .as_str()
                .unwrap()
                .contains("capture is incomplete")
        );
        assert_eq!("capture", returned[index]["id"]);
    }
    assert_eq!("SyntaxError", returned[4]["name"]);
    assert!(
        returned[5]["message"]
            .as_str()
            .unwrap()
            .contains("could not be decoded losslessly")
    );
    assert!(
        returned[6]["message"]
            .as_str()
            .unwrap()
            .contains("tools.read_output is required")
    );
    assert_eq!("capture", returned[6]["id"]);
    for (key, bytes, complete) in [
        ("partial", 4, false),
        ("binary", 2, true),
        ("retained", 2500, true),
    ] {
        assert_eq!(
            json!({
                "stream":"stdout", "bytes":bytes, "capture_complete":complete,
                "output_ref":{"id":"capture", "offset":0, "bytes":bytes}
            }),
            returned[7][key]
        );
    }
    assert_eq!("", returned[7]["empty"]);
    assert_eq!(6, facts(&result).actions.len());
}

#[tokio::test]
async fn uncaught_start_failure_keeps_its_program_after_compacting_host_facts() {
    let result = run(
        "await tools.run_command({program:'fixture-program'});",
        TestHost {
            operations: vec![ScriptOperation::RunCommand],
            callback: |_, _| async {
                let mut error = ToolResultEnvelope::error("start failed", "program could not start");
                error.structured = Some(json!({
                    "termination":"start_failed", "started":false,
                    "invocation":{"program":"fixture-program", "args":["private-argument".repeat(1024)]},
                    "output_ref":{"id":"unused-capture","offset":0,"bytes":1}
                }));
                error
            },
        },
        WorkspaceScriptLimits::default(),
    ).await;
    assert_eq!("error", result.status);
    let row: Value = result
        .model_content
        .lines()
        .find_map(|line| serde_json::from_str(line).ok())
        .expect("launch failure recovery evidence");
    assert_eq!(
        json!({"action":"action_1","tool":"run_command","program":"fixture-program",
        "termination":"start_failed","started":false}),
        row
    );
    assert!(!result.model_content.contains("private-argument"));
    assert_eq!(
        "fixture-program",
        facts(&result).actions[0].result["program"]
    );
}

#[tokio::test]
async fn returned_inline_command_text_counts_against_the_script_result_budget() {
    let result = run(
        r#"const run = await tools.run_command({}); return run.stdout;"#,
        TestHost {
            operations: vec![ScriptOperation::RunCommand],
            callback: |_, _| async {
                let bytes = vec![b'x'; 128];
                command(128, 0, true, json!({"stdout":bytes}))
            },
        },
        WorkspaceScriptLimits {
            result_bytes: 128,
            ..WorkspaceScriptLimits::default()
        },
    )
    .await;
    assert_eq!("error", result.status);
    assert!(
        result
            .model_content
            .contains("script result exceeds 128 bytes")
    );
    assert_eq!(1, facts(&result).actions.len());
    assert_eq!(
        "capture",
        facts(&result).actions[0].result["output_ref"]["id"]
    );
}

#[tokio::test]
async fn complete_readers_reject_oversized_text_and_lines() {
    const TEXT_BYTES: u64 = 1024 * 1024 + 1;
    let result = run(
        r#"
        const run = await tools.run_command({program:'fixture'});
        let textFailure, lineFailure;
        try { await run.stdout.text(); } catch (error) { textFailure = error.message; }
        try { for await (const line of run.stdout.lines()) throw new Error("unexpected line"); }
        catch (error) { lineFailure = error.message; }
        return {textFailure, lineFailure};
        "#,
        TestHost {
            operations: vec![ScriptOperation::RunCommand, ScriptOperation::ReadOutput],
            callback: |call: ScriptCall, _| async move {
                match call.op {
                    ScriptOperation::RunCommand => command(TEXT_BYTES, 0, true, Value::Null),
                    ScriptOperation::ReadOutput => {
                        let offset = call.args["offset"].as_u64().unwrap();
                        let limit = call.args["limit"].as_u64().unwrap();
                        assert!(offset + limit <= TEXT_BYTES);
                        let body = vec![b'x'; limit as usize];
                        output_page(offset, TEXT_BYTES, &[("stdout", &body)])
                    }
                    _ => panic!("unexpected workspace action"),
                }
            },
        },
        WorkspaceScriptLimits::default(),
    )
    .await;
    assert_eq!("ok", result.status, "{}", result.model_content);
    let returned = facts(&result).value;
    assert!(
        returned["textFailure"]
            .as_str()
            .unwrap()
            .contains("text exceeds 1048576 bytes")
    );
    assert!(
        returned["lineFailure"]
            .as_str()
            .unwrap()
            .contains("line exceeds 131072 bytes")
    );
}

#[test]
fn nested_command_title_rejection_points_to_the_outer_tool_call() {
    let workspace = tempfile::tempdir().unwrap();
    let direct = crate::tool::registry::run_command_definition(&[], true);
    let methods = super::super::method_definitions(|op| {
        (op == ScriptOperation::RunCommand).then(|| direct.clone())
    });
    let command = methods
        .iter()
        .find(|tool| tool.name == "run_command")
        .unwrap();
    let mut args = json!({"program":"python3", "title":"Check fixture"});
    let rejected = crate::tool::builtin::run_command::prepare_command_args(
        &mut args,
        workspace.path(),
        command,
    )
    .unwrap_err();
    assert!(rejected.model_content.contains("run_js({title, code})"));
    assert!(
        command
            .description
            .contains("Set title on the outer run_js call")
    );
    crate::tool::builtin::run_command::prepare_command_args(&mut args, workspace.path(), &direct)
        .unwrap();
    assert_eq!("Check fixture", args["title"]);
}

#[test]
fn model_definition_advertises_only_selected_script_methods() {
    let mut tools = crate::tool::builtin_registry(false);
    tools.retain(|tool| tool.name == "read_file");
    tools.push(super::super::definition());
    super::super::describe_selected_tools(&mut tools);
    let script = tools.iter().find(|tool| tool.name == "run_js").unwrap();
    assert!(
        script
            .description
            .contains("read_file({path,offset?,limit?}) returns result.raw_text")
    );
    assert!(script.description.contains("r.result.raw_text : r.error"));
    assert!(
        script
            .description
            .contains("create_output({}) returns result.id")
    );
    assert!(!script.description.contains("run_command"));
    assert!(!script.description.contains("output_lines"));
    assert!(!script.description.contains("output_text"));
    assert!(!script.description.contains("search_output"));
    assert!(!script.description.contains("write_file"));
    assert_eq!(
        "boolean",
        script.input_schema["properties"]["background"]["type"]
    );
}

#[tokio::test]
async fn complete_retained_text_rejects_growing_lossy_and_oversized_captures() {
    for (case, expected) in [
        ("growing", "capture is incomplete"),
        ("lossy", "could not be decoded losslessly"),
        ("oversized", "text exceeds 1048576 bytes"),
    ] {
        let result = run(
            r#"return await tools.output_text({id:"capture"});"#,
            TestHost {
                operations: vec![ScriptOperation::ReadOutput],
                callback: move |_, _| async move {
                    match case {
                        "growing" => {
                            let mut page = output_page(0, 1, &[("stdout", b"x")]);
                            page.structured.as_mut().unwrap()["complete"] = json!(false);
                            page
                        }
                        "lossy" => output_page(0, 2, &[("stdout", b"\xff\0")]),
                        _ => {
                            let bytes = crate::tool::run_js::MAX_OUTPUT_TEXT_BYTES + 1;
                            ToolResultEnvelope::ok("read retained output", "", json!({
                                "output_id":"capture", "offset":0,"next_offset":bytes,"total_bytes":bytes,
                                "complete":true,"has_more":false,
                                "segments":[{"stream":"stdout","offset":0,"text":"x".repeat(bytes)}],
                            }))
                        }
                    }
                },
            },
            WorkspaceScriptLimits::default(),
        )
        .await;
        assert_eq!("error", result.status, "{case}: {}", result.model_content);
        assert!(
            result.model_content.contains(expected),
            "{}",
            result.model_content
        );
    }
}

fn command(stdout: u64, stderr: u64, complete: bool, inline: Value) -> ToolResultEnvelope {
    ToolResultEnvelope::ok(
        "exited",
        "",
        json!({
            "termination":"exited", "exit_code":3, "capture_complete":complete,
            "output_ref":{"id":"capture","offset":0,"bytes":stdout+stderr},
            "stdout_bytes":stdout, "stderr_bytes":stderr, "inline_pipes":inline,
        }),
    )
}

fn output_page(offset: u64, total: u64, parts: &[(&str, &[u8])]) -> ToolResultEnvelope {
    let mut next_offset = offset;
    let segments = parts.iter().map(|(stream, bytes)| {
        let raw = json!({"stream":stream, "offset":next_offset, "bytes":bytes.len(), "raw_bytes":bytes});
        next_offset += bytes.len() as u64;
        raw
    }).collect::<Vec<_>>();
    ToolResultEnvelope::ok(
        "read retained output",
        "",
        json!({
            "output_id":"capture", "offset":offset, "next_offset":next_offset,
            "total_bytes":total, "segments":segments, "has_more":next_offset<total,
            "complete":true, "filtered":false, "capture_truncated":false, "cleanup_incomplete":false,
        }),
    )
}
