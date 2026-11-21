use std::sync::Arc;

use serde_json::{json, Value};

use crate::tool::builtin::run_command::{output::OutputLimits, run_command_for_script};
use crate::tool::ToolResultEnvelope;
use crate::OutputRead;

use super::{
    facts, run, ScriptCall, ScriptExecution, ScriptOperation, TestHost, WorkspaceScriptLimits,
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
        return {exit:run.exit_code, output:await run.stdout.json(), lines, id:run.output_ref.id};
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
                    run_command_for_script(
                        &json!({"shell":shell, "command":command, "cwd":".", "timeout":5}),
                        workspace.path(),
                        execution.cancellation(),
                        workspace.path(),
                        "capture",
                        OutputLimits::default(),
                        execution.limits.host_bytes,
                    ).await
                }
            },
        },
        WorkspaceScriptLimits::default(),
    ).await;
    assert_eq!("ok", result.status, "{}", result.model_content);
    let facts = facts(&result);
    assert_eq!(
        json!({"exit":7, "output":{"value":7}, "lines":["note","last"], "id":"capture"}),
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
                return {name:error.name, message:error.message, status:error.status, termination:error.termination, id:error.output_ref?.id};
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
            await errorFor(() => retained.stdout.text())];
        "#,
        TestHost {
            operations: vec![ScriptOperation::RunCommand],
            callback: |call: ScriptCall, _| async move {
                match call.args["program"].as_str().unwrap() {
                    "missing" => {
                        let mut missing = ToolResultEnvelope::error("program missing", "could not start missing program");
                        missing.structured = Some(json!({"termination":"program_missing", "output_ref":{"id":"capture","offset":0,"bytes":0}}));
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
    assert_eq!("capture", returned[0]["id"]);
    assert!(returned[0]["message"]
        .as_str()
        .unwrap()
        .contains("could not start missing program"));
    assert_eq!("cancelled", returned[1]["status"]);
    assert_eq!("cancelled", returned[1]["termination"]);
    assert!(returned[1]["message"]
        .as_str()
        .unwrap()
        .contains("command cancelled"));
    for index in [2, 3] {
        assert!(returned[index]["message"]
            .as_str()
            .unwrap()
            .contains("capture is incomplete"));
        assert_eq!("capture", returned[index]["id"]);
    }
    assert_eq!("SyntaxError", returned[4]["name"]);
    assert!(returned[5]["message"]
        .as_str()
        .unwrap()
        .contains("could not be decoded losslessly"));
    assert!(returned[6]["message"]
        .as_str()
        .unwrap()
        .contains("tools.read_output is required"));
    assert_eq!("capture", returned[6]["id"]);
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
    assert!(returned["textFailure"]
        .as_str()
        .unwrap()
        .contains("text exceeds 1048576 bytes"));
    assert!(returned["lineFailure"]
        .as_str()
        .unwrap()
        .contains("line exceeds 131072 bytes"));
}

#[test]
fn model_definition_advertises_only_selected_script_methods() {
    let mut tools = crate::tool::builtin_registry(false, false);
    tools.retain(|tool| tool.name == "read_file");
    tools.push(super::super::definition());
    super::super::describe_selected_tools(&mut tools);
    let script = tools.iter().find(|tool| tool.name == "run_js").unwrap();
    assert!(script
        .description
        .contains("read_file({path,offset?,limit?}) returns result.raw_text"));
    assert!(script
        .description
        .contains("create_output({}) returns result.id"));
    assert!(!script.description.contains("run_command"));
    assert!(!script.description.contains("output_lines"));
    assert!(!script.description.contains("write_file"));
    assert_eq!(
        "boolean",
        script.input_schema["properties"]["background"]["type"]
    );
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
