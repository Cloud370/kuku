mod command_output;

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::{Barrier, Notify};

use crate::WorkspaceExecutionHost;
use crate::tool::builtin::common::{ExecutorObservation, ExecutorObservationSource};
use crate::tool::{ToolDefinition, ToolResultEnvelope};

use super::{
    ScriptCall, ScriptExecution, ScriptFuture, ScriptHost, ScriptOperation,
    WorkspaceScriptActionStatus, WorkspaceScriptFacts, WorkspaceScriptLimits, execute,
};

struct TestHost<F> {
    operations: Vec<ScriptOperation>,
    callback: F,
}

impl<F, Fut> ScriptHost for TestHost<F>
where
    F: Fn(ScriptCall, Arc<ScriptExecution>) -> Fut + Send + Sync,
    Fut: Future<Output = ToolResultEnvelope> + Send + 'static,
{
    fn methods(&self) -> Vec<ToolDefinition> {
        self.operations
            .iter()
            .map(|operation| ToolDefinition {
                name: operation.name().into(),
                description: "test operation".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {"limit": {"default": 65536}},
                }),
                read_only: true,
                max_result_chars: 4096,
                risk: "read".into(),
            })
            .collect()
    }

    fn dispatch(
        &self,
        call: ScriptCall,
        execution: Arc<ScriptExecution>,
        permit: tokio::sync::OwnedSemaphorePermit,
    ) -> ScriptFuture<'_, ToolResultEnvelope> {
        Box::pin(async move {
            let _permit = permit;
            (self.callback)(call, execution).await
        })
    }
}

#[tokio::test]
async fn script_syntax_errors_locate_user_code_before_any_host_action() {
    for (code, location) in [
        (
            "await tools.read_file({path:'a'});\nconst bad = [)]",
            "script line 2, column 14",
        ),
        (
            "await tools.read_file({path:'a'});\r\nconst bad = [)]",
            "script line 2, column 14",
        ),
        ("return [", "at the end of the script"),
    ] {
        let output = run(
            code,
            TestHost {
                operations: vec![ScriptOperation::ReadFile],
                callback: |_, _| async { panic!("compilation must not dispatch host actions") },
            },
            WorkspaceScriptLimits::default(),
        )
        .await;
        assert_eq!("error", output.status);
        assert_eq!("run_js failed", output.summary);
        assert!(
            output.model_content.contains(location),
            "{}",
            output.model_content
        );
        assert!(facts(&output).actions.is_empty());
    }
}

#[tokio::test]
async fn script_runtime_errors_locate_submitted_code_and_preserve_awaited_actions() {
    for (code, message, location, actions, missing_method) in [
        (
            "await tools.read_file({path:'notes'});\nconst result = 1;\nreturn tools.missing_method({result});",
            "not a function",
            "script line 3, column ",
            1,
            Some("missing_method"),
        ),
        (
            "const note = '前 🦉';\r\nconst after = '界'; return await tools.fetch_url({});",
            "not a function",
            "script line 2, column ",
            0,
            Some("fetch_url"),
        ),
        (
            "const other = {tools: {fetch_url: null}}; return other.tools.fetch_url();",
            "not a function",
            "script line 1, column ",
            0,
            None,
        ),
        (
            "return tools.methods();",
            "not a function",
            "script line 1, column ",
            0,
            None,
        ),
        (
            "function describe(item) {\n  return item.fields.map(String);\n}\nreturn describe({});",
            "cannot read property 'map' of undefined",
            "script line 2, column ",
            0,
            None,
        ),
        (
            "const selector = {id:'capture',stream:'other'};\nreturn await tools.output_text(selector);",
            "output_text requires id or name and an optional stdout or stderr stream",
            "script line 2, column ",
            0,
            None,
        ),
    ] {
        let host = TestHost {
            operations: vec![ScriptOperation::ReadFile, ScriptOperation::ReadOutput],
            callback: |call: ScriptCall, _| async move {
                assert_eq!(ScriptOperation::ReadFile, call.op);
                ToolResultEnvelope::ok("read", "read", json!({"raw_text":"hello"}))
            },
        };
        let output = run(code, host, WorkspaceScriptLimits::default()).await;
        assert_eq!("error", output.status);
        assert!(
            output
                .model_content
                .contains(&format!("{message} ({location}")),
            "{}",
            output.model_content
        );
        if let Some(name) = missing_method {
            assert!(
                output
                    .model_content
                    .contains(&format!("does not expose tools.{name}"))
            );
            assert!(
                output
                    .model_content
                    .contains(&format!("call {name} separately"))
            );
        } else {
            assert!(
                !output
                    .model_content
                    .contains("The run_js host does not expose")
            );
        }
        assert_eq!(actions, facts(&output).actions.len());
    }
}

#[tokio::test]
async fn oversized_script_result_preserves_nonzero_exit_and_the_partial_capture_reference() {
    for can_read_output in [false, true] {
        let mut operations = vec![ScriptOperation::RunCommand];
        if can_read_output {
            operations.push(ScriptOperation::ReadOutput);
        }
        let output = run(
            "await tools.run_command({program:'fixture'}); return 'x'.repeat(2000);",
            TestHost {
                operations,
                callback: |call: ScriptCall, _| async move {
                    assert_eq!(ScriptOperation::RunCommand, call.op);
                    ToolResultEnvelope::ok("command exited 7", "", json!({
                        "termination":"exited","started":true,"exit_code":7,
                        "capture_complete":false,"output_ref":{"id":"known-capture","offset":0,"bytes":33},
                        "inline_pipes":{"stderr":"raw payload must stay in the capture"}
                    }))
                },
            },
            WorkspaceScriptLimits { result_bytes: 256, ..Default::default() },
        ).await;
        assert_eq!("error", output.status, "{}", output.model_content);
        assert!(
            output
                .model_content
                .contains("script result exceeds 256 bytes")
        );
        assert!(output.model_content.contains("\"exit_code\":7"));
        assert!(output.model_content.contains("\"capture_complete\":false"));
        assert!(output.model_content.contains("\"id\":\"known-capture\""));
        assert!(!output.model_content.contains("raw payload"));
        assert_eq!(
            can_read_output,
            output
                .model_content
                .contains("read_output({id, offset, limit})")
        );
        assert_eq!(1, facts(&output).actions.len());
    }
}

#[tokio::test]
async fn script_exposes_only_the_named_methods_and_supports_standard_async_javascript() {
    let host = TestHost {
        operations: vec![ScriptOperation::ReadFile],
        callback: |call: ScriptCall, _| async move {
            ToolResultEnvelope::ok(
                "read",
                "model-only view",
                json!({"raw_text":call.args["path"]}),
            )
        },
    };
    let result = run(
        r#"
        const args = {path: "中文 🦉"};
        const read = await tools.read_file(args);
        return {
            names: tools.methods.map(({name}) => name),
            text: read.result.raw_text,
            optional: tools.fetch_url?.() ?? null,
            available: [typeof tools.call, typeof tools.run_command, typeof require,
                typeof process, typeof fetch, typeof tools.output_lines, typeof workspace],
            reply_keys: Object.keys(read).sort(),
        };
        "#,
        host,
        WorkspaceScriptLimits::default(),
    )
    .await;
    assert_eq!("ok", result.status, "{}", result.model_content);
    let facts = facts(&result);
    assert_eq!(
        json!({
            "names": ["read_file"],
            "text": "中文 🦉",
            "optional": null,
            "available": ["undefined", "undefined", "undefined", "undefined", "undefined", "undefined", "undefined"],
            "reply_keys": ["result", "status", "truncated"],
        }),
        facts.value
    );
    assert_eq!(1, facts.actions.len());
    assert_eq!(Value::Null, facts.actions[0].result["raw_text"]);
    assert_eq!(
        facts.value,
        serde_json::from_str::<Value>(&result.model_content).unwrap()
    );
}

#[tokio::test]
async fn named_value_methods_forward_direct_tool_arguments() {
    let result = run(
        r#"
        const literal = await tools.save_value({name: "report", value: {passed: 7}, replace: true});
        const output = await tools.save_value({name: "log", output_id: "retained"});
        const page = await tools.list_values({prefix: "log", after: "log", limit: 2});
        const defaultPage = await tools.list_values();
        return {
            methods: tools.methods.map(({name}) => name),
            literal: literal.result,
            output: output.result,
            page: page.result,
            defaultPage: defaultPage.result,
            sendMessage: typeof tools.send_message,
            askUser: typeof tools.ask_user,
        };
        "#,
        TestHost {
            operations: vec![ScriptOperation::SaveValue, ScriptOperation::ListValues],
            callback: |call: ScriptCall, _| async move {
                ToolResultEnvelope::ok(call.op.name(), "", call.args)
            },
        },
        WorkspaceScriptLimits::default(),
    )
    .await;
    assert_eq!("ok", result.status, "{}", result.model_content);
    let facts = facts(&result);
    assert_eq!(
        json!({
            "methods": ["save_value", "list_values"],
            "literal": {"name": "report", "value": {"passed": 7}, "replace": true},
            "output": {"name": "log", "output_id": "retained"},
            "page": {"prefix": "log", "after": "log", "limit": 2},
            "defaultPage": {},
            "sendMessage": "undefined",
            "askUser": "undefined",
        }),
        facts.value
    );
    assert_eq!(
        [
            ScriptOperation::SaveValue,
            ScriptOperation::SaveValue,
            ScriptOperation::ListValues,
            ScriptOperation::ListValues,
        ],
        facts
            .actions
            .iter()
            .map(|action| action.tool)
            .collect::<Vec<_>>()
            .as_slice()
    );
}

#[tokio::test]
async fn independent_host_promises_run_concurrently_and_keep_request_order() {
    let entered = Arc::new(Barrier::new(3));
    let barrier = entered.clone();
    let host = TestHost {
        operations: vec![ScriptOperation::ReadFile],
        callback: move |call: ScriptCall, _| {
            let barrier = barrier.clone();
            async move {
                barrier.wait().await;
                ToolResultEnvelope::ok("read", "", call.args)
            }
        },
    };
    let worker = tokio::spawn(run(
        r#"
        const results = await Promise.all([
            tools.read_file({path: "first"}),
            tools.read_file({path: "second"}),
        ]);
        return results.map(({result}) => result.path);
        "#,
        host,
        WorkspaceScriptLimits::default(),
    ));
    tokio::time::timeout(Duration::from_secs(3), entered.wait())
        .await
        .expect("both independent host operations entered");
    let result = worker.await.unwrap();
    assert_eq!("ok", result.status, "{}", result.model_content);
    let facts = facts(&result);
    assert_eq!(json!(["first", "second"]), facts.value);
    assert_eq!("first", facts.actions[0].result["path"]);
    assert_eq!("second", facts.actions[1].result["path"]);
}

#[tokio::test]
async fn slow_host_replies_do_not_consume_the_javascript_budget() {
    let result = run(
        r#"
        const slow = await tools.read_file({path: "slow"});
        const followUp = await tools.read_file({path: "follow-up"});
        return [slow.result.path, followUp.result.path];
        "#,
        TestHost {
            operations: vec![ScriptOperation::ReadFile],
            callback: |call: ScriptCall, _| async move {
                if call.args["path"] == "slow" {
                    tokio::time::sleep(Duration::from_millis(400)).await;
                }
                ToolResultEnvelope::ok("read finished", "", call.args)
            },
        },
        WorkspaceScriptLimits {
            timeout_ms: 250,
            ..Default::default()
        },
    )
    .await;
    assert_eq!("ok", result.status, "{}", result.model_content);
    let facts = facts(&result);
    assert_eq!(json!(["slow", "follow-up"]), facts.value);
    assert_eq!(2, facts.actions.len());
    assert!(!facts.timed_out);
}

#[tokio::test]
async fn ready_host_replies_progress_while_microtasks_are_queued() {
    let result = run(
        r#"
        let finished = false;
        const again = () => { if (!finished) Promise.resolve().then(again); };
        Promise.resolve().then(again);
        const read = await tools.read_file({path: "loop"});
        finished = true;
        return read.result.path;
        "#,
        TestHost {
            operations: vec![ScriptOperation::ReadFile],
            callback: |call: ScriptCall, _| async move {
                tokio::time::sleep(Duration::from_millis(25)).await;
                ToolResultEnvelope::ok("read finished", "", call.args)
            },
        },
        WorkspaceScriptLimits {
            timeout_ms: 500,
            ..Default::default()
        },
    )
    .await;
    assert_eq!("ok", result.status, "{}", result.model_content);
    assert_eq!(json!("loop"), facts(&result).value);
}

#[tokio::test]
async fn host_queue_bounds_reject_individual_promises_before_dispatch() {
    let result =
        run(
            r#"
        const pending = [1,2,3].map(n => tools.read_file({n}));
        return (await Promise.allSettled(pending)).map(({status}) => status);
        "#,
            TestHost {
                operations: vec![ScriptOperation::ReadFile],
                callback: |call: ScriptCall, _| async move {
                    ToolResultEnvelope::ok("read", "", call.args)
                },
            },
            WorkspaceScriptLimits {
                active_operations: 1,
                queued_operations: 2,
                ..Default::default()
            },
        )
        .await;
    assert_eq!("ok", result.status, "{}", result.model_content);
    let facts = facts(&result);
    assert_eq!(json!(["fulfilled", "fulfilled", "rejected"]), facts.value);
    assert_eq!(2, facts.actions.len());
    assert_eq!(0, facts.not_dispatched);
}

#[tokio::test]
async fn read_evidence_becomes_available_only_after_the_reply_reaches_javascript() {
    let result = run(
        r#"
        const reads = [];
        for (const size of [2048, 5]) {
            const [read] = await Promise.allSettled([tools.read_file({size})]);
            const write = await tools.write_file({path: "file.txt", content: "updated"});
            reads.push([read.status, write.result.observed_reads]);
        }
        return reads;
        "#,
        TestHost {
            operations: vec![ScriptOperation::ReadFile, ScriptOperation::WriteFile],
            callback: |call: ScriptCall, execution: Arc<ScriptExecution>| async move {
                match call.op {
                    ScriptOperation::ReadFile => {
                        let size = call.args["size"].as_u64().unwrap() as usize;
                        execution.stage_observation(
                            &call.id,
                            ExecutorObservation {
                                canonical_path: PathBuf::from("file.txt"),
                                content_hash: format!("hash-{size}"),
                                range: 0..size,
                                size_bytes: size,
                                complete: true,
                                source: ExecutorObservationSource::Read,
                            },
                        );
                        ToolResultEnvelope::ok("read", "", json!({"raw_text":"x".repeat(size)}))
                    }
                    ScriptOperation::WriteFile => ToolResultEnvelope::ok(
                        "write",
                        "",
                        json!({"observed_reads":execution.observations().len()}),
                    ),
                    _ => panic!("unexpected host call"),
                }
            },
        },
        WorkspaceScriptLimits {
            host_bytes: 1024,
            ..Default::default()
        },
    )
    .await;
    assert_eq!("ok", result.status, "{}", result.model_content);
    assert_eq!(
        json!([["rejected", 0], ["fulfilled", 1]]),
        facts(&result).value
    );
}

#[tokio::test]
async fn cancellation_waits_for_host_cleanup_and_keeps_the_observed_result() {
    let capacity = WorkspaceExecutionHost::new(1, 1).unwrap();
    for code in [
        "await tools.run_command({program:'loop'});",
        "tools.run_command({program:'loop'}); while (true) {}",
    ] {
        let started = Arc::new(Notify::new());
        let entered = started.clone();
        let cancel = Arc::new(Notify::new());
        let host = TestHost {
            operations: vec![ScriptOperation::RunCommand],
            callback: move |_: ScriptCall, execution: Arc<ScriptExecution>| {
                let entered = entered.clone();
                async move {
                    entered.notify_one();
                    execution.cancelled().await;
                    ToolResultEnvelope::cancelled("command finished cleanup")
                }
            },
        };
        let worker = tokio::spawn(execute(
            code.into(),
            Arc::new(host),
            WorkspaceScriptLimits::default(),
            capacity.clone(),
            cancel.clone(),
        ));
        tokio::time::timeout(Duration::from_secs(3), started.notified())
            .await
            .expect("host command entered");
        cancel.notify_one();
        let result = worker.await.unwrap();
        assert_eq!(
            "cancelled", result.status,
            "{code}: {}",
            result.model_content
        );
        assert_eq!(0, capacity.metrics().active_workers);
        let facts = facts(&result);
        assert!(!facts.cleanup_incomplete);
        assert!(!facts.timed_out);
        assert_eq!(1, facts.actions.len());
        assert_eq!(
            WorkspaceScriptActionStatus::Cancelled,
            facts.actions[0].status
        );
        assert_eq!("command finished cleanup", facts.actions[0].summary);
    }
}

#[tokio::test]
async fn steering_cancellation_explains_completed_and_cancelled_actions() {
    let result = run(
        "await tools.read_file({path:'first'}); await tools.read_file({path:'second'});",
        TestHost {
            operations: vec![ScriptOperation::ReadFile],
            callback: |call: ScriptCall, execution: Arc<ScriptExecution>| async move {
                if call.args["path"] == "first" {
                    return ToolResultEnvelope::ok("first read", "first", json!({}));
                }
                execution.stop_for_new_input();
                ToolResultEnvelope::cancelled("new input arrived at the workspace action boundary")
            },
        },
        WorkspaceScriptLimits::default(),
    )
    .await;
    assert_eq!("cancelled", result.status);
    assert!(result.model_content.contains("because new input arrived"));
    assert!(result.model_content.contains("completed: 1. Cancelled: 1."));
    let facts = facts(&result);
    assert_eq!(2, facts.actions.len());
    assert_eq!(WorkspaceScriptActionStatus::Ok, facts.actions[0].status);
    assert_eq!(
        WorkspaceScriptActionStatus::Cancelled,
        facts.actions[1].status
    );
}

#[tokio::test]
async fn returning_with_pending_host_work_cancels_it_and_waits_for_cleanup() {
    let entered = Arc::new(Notify::new());
    let result = run(
        r#"
        tools.read_file({path: "abandoned"});
        await tools.read_file({path: "ready"});
        return true;
        "#,
        TestHost {
            operations: vec![ScriptOperation::ReadFile],
            callback: move |call: ScriptCall, execution: Arc<ScriptExecution>| {
                let entered = entered.clone();
                async move {
                    if call.args["path"] == "abandoned" {
                        entered.notify_one();
                        execution.cancelled().await;
                        ToolResultEnvelope::cancelled("read finished cleanup")
                    } else {
                        entered.notified().await;
                        ToolResultEnvelope::ok("read ready", "", json!(null))
                    }
                }
            },
        },
        WorkspaceScriptLimits::default(),
    )
    .await;
    assert_eq!("error", result.status);
    assert!(result.model_content.contains("pending host operations"));
    let facts = facts(&result);
    assert!(!facts.cleanup_incomplete);
    assert_eq!(2, facts.actions.len());
    assert_eq!(
        WorkspaceScriptActionStatus::Cancelled,
        facts.actions[0].status
    );
    assert_eq!(WorkspaceScriptActionStatus::Ok, facts.actions[1].status);
}

#[tokio::test]
async fn waiting_for_worker_capacity_does_not_consume_the_javascript_budget() {
    let capacity = WorkspaceExecutionHost::new(1, 1).unwrap();
    let limits = WorkspaceScriptLimits {
        timeout_ms: 100,
        ..Default::default()
    };
    let permit = capacity.acquire_worker().await.unwrap();
    let waiting = execute(
        "return 1;".into(),
        empty_host(),
        limits,
        capacity.clone(),
        Arc::new(Notify::new()),
    );
    tokio::pin!(waiting);
    assert!(
        tokio::time::timeout(Duration::from_millis(200), waiting.as_mut())
            .await
            .is_err()
    );
    assert_eq!(1, capacity.metrics().started_workers);
    drop(permit);
    let result = waiting.await;
    assert_eq!("ok", result.status, "{}", result.model_content);
    assert_eq!(json!(1), facts(&result).value);
    assert_eq!(2, capacity.metrics().started_workers);
    assert_eq!(0, capacity.metrics().active_workers);
}

#[tokio::test]
async fn the_javascript_budget_interrupts_busy_loops_and_unending_microtasks() {
    let capacity = WorkspaceExecutionHost::new(1, 1).unwrap();
    let limits = WorkspaceScriptLimits {
        timeout_ms: 100,
        ..Default::default()
    };
    for code in ["while (true) {}", "while (true) await Promise.resolve();"] {
        let running = tokio::time::timeout(
            Duration::from_secs(5),
            execute(
                code.into(),
                empty_host(),
                limits.clone(),
                capacity.clone(),
                Arc::new(Notify::new()),
            ),
        )
        .await
        .expect("JavaScript did not stop within its budget");
        assert!(
            facts(&running).timed_out,
            "{code}: {}",
            running.model_content
        );
        assert!(!facts(&running).cleanup_incomplete);
    }
    assert_eq!(0, capacity.metrics().active_workers);
}

#[tokio::test]
async fn pending_host_work_does_not_pause_a_runnable_microtask_queue() {
    let result = run(
        r#"
        tools.run_command({program: "blocked"});
        while (true) await Promise.resolve();
        "#,
        TestHost {
            operations: vec![ScriptOperation::RunCommand],
            callback: |_, execution: Arc<ScriptExecution>| async move {
                execution.cancelled().await;
                ToolResultEnvelope::cancelled("command finished cleanup")
            },
        },
        WorkspaceScriptLimits {
            timeout_ms: 250,
            ..Default::default()
        },
    )
    .await;
    let facts = facts(&result);
    assert!(facts.timed_out, "{}", result.model_content);
    assert!(!facts.cleanup_incomplete);
    assert_eq!(1, facts.actions.len());
    assert_eq!(
        WorkspaceScriptActionStatus::Cancelled,
        facts.actions[0].status
    );
}

#[tokio::test]
async fn the_javascript_budget_is_not_reset_by_host_progress() {
    let result = run(
        r#"
        const spin = () => {
            const stop = Date.now() + 250;
            while (Date.now() < stop) {}
        };
        spin();
        await tools.read_file({path: "slow"});
        spin();
        return true;
        "#,
        TestHost {
            operations: vec![ScriptOperation::ReadFile],
            callback: |_, _| async {
                tokio::time::sleep(Duration::from_millis(350)).await;
                ToolResultEnvelope::ok("read finished", "", json!(null))
            },
        },
        WorkspaceScriptLimits {
            timeout_ms: 400,
            ..Default::default()
        },
    )
    .await;
    let facts = facts(&result);
    assert!(facts.timed_out, "{}", result.model_content);
    assert!(!facts.cleanup_incomplete);
    assert_eq!(1, facts.actions.len());
    assert_eq!(WorkspaceScriptActionStatus::Ok, facts.actions[0].status);
}

#[tokio::test]
async fn bounds_heap_stack_results_and_exception_messages() {
    let capacity = WorkspaceExecutionHost::new(1, 1).unwrap();
    let limits = WorkspaceScriptLimits {
        heap_bytes: 1024 * 1024,
        stack_bytes: 64 * 1024,
        result_bytes: 256,
        ..Default::default()
    };
    for (script, message) in [
        (
            "return new Uint8Array(2 * 1024 * 1024).length",
            "out of memory",
        ),
        (
            "const a = new Uint8Array(700 * 1024); const b = new Uint8Array(700 * 1024); return a.length + b.length;",
            "out of memory",
        ),
        (
            "function recurse(){return 1 + recurse()}; return recurse()",
            "maximum call stack size exceeded",
        ),
        ("return 'x'.repeat(2000)", "result exceeds 256 bytes"),
        ("throw new Error('限'.repeat(10000))", "限"),
        ("return await new Promise(() => {})", "no host work"),
    ] {
        let output = execute(
            script.into(),
            empty_host(),
            limits.clone(),
            capacity.clone(),
            Arc::new(Notify::new()),
        )
        .await;
        assert_eq!("error", output.status, "{script}: {}", output.model_content);
        assert!(
            output.model_content.to_lowercase().contains(message),
            "{script}: {}",
            output.model_content
        );
        assert!(output.model_content.len() <= 2051);
        assert_eq!(0, capacity.metrics().active_workers);
    }
}

#[tokio::test]
async fn output_lines_join_interleaved_utf8_and_pin_the_first_resolved_output() {
    let result = run(
        r#"
        const lines = [];
        for await (const item of tools.output_lines({name: "sample", limit: 6})) {
            const {text, stream, offset, next_offset, bytes, lossy_utf8} = item;
            lines.push({text, stream, offset, next_offset, bytes, lossy_utf8});
        }
        return lines;
        "#,
        TestHost {
            operations: vec![ScriptOperation::ReadOutput],
            callback: |call: ScriptCall, _| async move {
                let offset = call.args["offset"].as_u64().unwrap();
                let (next_offset, segments) = if offset == 0 {
                    assert_eq!("sample", call.args["name"]);
                    (
                        6,
                        json!([
                            {"stream":"stdout","offset":0,"raw_bytes":[120,228]},
                            {"stream":"stderr","offset":2,"text":"err\n"},
                        ]),
                    )
                } else {
                    assert_eq!("retained", call.args["id"]);
                    (
                        10,
                        json!([{"stream":"stdout","offset":6,"raw_bytes":[184,173,13,10]}]),
                    )
                };
                ToolResultEnvelope::ok(
                    "read retained bytes",
                    "",
                    json!({
                        "offset":offset,"next_offset":next_offset,"output_id":"retained",
                        "total_bytes":10,"segments":segments,"has_more":next_offset<10,
                        "complete":true,"capture_truncated":false,
                    }),
                )
            },
        },
        WorkspaceScriptLimits::default(),
    )
    .await;
    assert_eq!("ok", result.status, "{}", result.model_content);
    let facts = facts(&result);
    assert_eq!(
        json!([
            {"text":"err","stream":"stderr","offset":2,"next_offset":6,"bytes":4,"lossy_utf8":false},
            {"text":"x中","stream":"stdout","offset":0,"next_offset":10,"bytes":6,"lossy_utf8":false},
        ]),
        facts.value
    );
    assert_eq!(1, facts.actions.len());
    assert_eq!(2, facts.actions[0].result["pages"]);
    assert_eq!(10, facts.actions[0].result["next_offset"]);
    assert_eq!(0, facts.not_dispatched);
}

fn facts(result: &ToolResultEnvelope) -> WorkspaceScriptFacts {
    WorkspaceScriptFacts::from_structured(result.structured.as_ref().unwrap()).unwrap()
}

#[tokio::test]
async fn console_log_returns_text_after_host_actions() {
    let result = run(
        r#"
        const read = await tools.read_file({path: "input"});
        console.log("rows", {found: read.result.count});
        console.log("中文 🦉");
        "#,
        TestHost {
            operations: vec![ScriptOperation::ReadFile],
            callback: |_, _| async { ToolResultEnvelope::ok("read", "", json!({"count": 3})) },
        },
        WorkspaceScriptLimits::default(),
    )
    .await;
    assert_eq!("ok", result.status, "{}", result.model_content);
    let recorded = facts(&result);
    assert_eq!("rows {\"found\":3}\n中文 🦉", recorded.value);
    assert_eq!(1, recorded.actions.len());
    assert_eq!(WorkspaceScriptActionStatus::Ok, recorded.actions[0].status);
}

#[tokio::test]
async fn console_log_respects_the_result_bound_and_explicit_returns() {
    for (source, expected) in [
        (r#"console.log("\u0000".repeat(1024));"#, None),
        (
            r#"console.log("large payload".repeat(1024)); return null;"#,
            Some(Value::Null),
        ),
    ] {
        let result = run(
            source,
            TestHost {
                operations: vec![],
                callback: |_, _| async { panic!("logging must stay in the worker") },
            },
            WorkspaceScriptLimits {
                result_bytes: 128,
                ..WorkspaceScriptLimits::default()
            },
        )
        .await;
        assert_eq!("ok", result.status, "{}", result.model_content);
        assert!(result.model_content.len() <= 128);
        let value = facts(&result).value;
        if let Some(expected) = expected {
            assert_eq!(expected, value);
        } else {
            let text = value.as_str().unwrap();
            assert!(text.starts_with('\0'));
            assert!(text.ends_with("[console output truncated]"));
        }
    }
}

async fn run<F, Fut>(
    code: &str,
    host: TestHost<F>,
    limits: WorkspaceScriptLimits,
) -> ToolResultEnvelope
where
    F: Fn(ScriptCall, Arc<ScriptExecution>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ToolResultEnvelope> + Send + 'static,
{
    tokio::time::timeout(
        Duration::from_secs(5),
        execute(
            code.into(),
            Arc::new(host),
            limits,
            WorkspaceExecutionHost::new(1, 2).unwrap(),
            Arc::new(Notify::new()),
        ),
    )
    .await
    .expect("workspace script did not finish")
}

fn empty_host() -> Arc<dyn ScriptHost> {
    Arc::new(TestHost {
        operations: Vec::new(),
        callback: |_, _| async { ToolResultEnvelope::error("unexpected host call", "test setup") },
    })
}
