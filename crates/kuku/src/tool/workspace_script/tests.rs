use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::{Barrier, Notify};

use crate::tool::builtin::common::ExecutorRead;
use crate::tool::{ToolDefinition, ToolResultEnvelope};
use crate::WorkspaceExecutionHost;

use super::{
    execute, ScriptCall, ScriptExecution, ScriptFuture, ScriptHost, ScriptOperation,
    WorkspaceScriptActionStatus, WorkspaceScriptFacts, WorkspaceScriptLimits,
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
                    "properties": {"limit": {"maximum": 80000}},
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
    ) -> ScriptFuture<'_, ToolResultEnvelope> {
        Box::pin((self.callback)(call, execution))
    }
}

#[tokio::test]
async fn script_exposes_only_the_named_methods_and_supports_standard_async_javascript() {
    let host = TestHost {
        operations: vec![ScriptOperation::ReadFile],
        callback: |call: ScriptCall, _| async move {
            ToolResultEnvelope::ok("read", "", json!({"raw_text":call.args["path"]}))
        },
    };
    let result = run(
        r#"
        const args = {path: "中文 🦉"};
        const read = await workspace.read_file(args);
        return {
            names: workspace.methods.map(({name}) => name),
            text: read.result.raw_text,
            available: [typeof workspace.call, typeof workspace.run_command, typeof require,
                typeof process, typeof fetch, typeof workspace.output_lines],
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
            "available": ["undefined", "undefined", "undefined", "undefined", "undefined", "undefined"],
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
        const literal = await workspace.save_value({name: "report", value: {passed: 7}, replace: true});
        const output = await workspace.save_value({name: "log", output_id: "retained"});
        const page = await workspace.list_values({prefix: "log", after: "log", limit: 2});
        const defaultPage = await workspace.list_values();
        return {
            methods: workspace.methods.map(({name}) => name),
            literal: literal.result,
            output: output.result,
            page: page.result,
            defaultPage: defaultPage.result,
            sendMessage: typeof workspace.send_message,
            askUser: typeof workspace.ask_user,
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
            workspace.read_file({path: "first"}),
            workspace.read_file({path: "second"}),
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
async fn host_queue_bounds_reject_individual_promises_before_dispatch() {
    let result =
        run(
            r#"
        const pending = [1,2,3].map(n => workspace.read_file({n}));
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
            const [read] = await Promise.allSettled([workspace.read_file({size})]);
            const write = await workspace.write_file({path: "file.txt", content: "updated"});
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
                        execution.stage_read(
                            &call.id,
                            ExecutorRead {
                                canonical_path: PathBuf::from("file.txt"),
                                content_hash: format!("hash-{size}"),
                                range: 0..size,
                                size_bytes: size,
                                complete: true,
                            },
                        );
                        ToolResultEnvelope::ok("read", "", json!({"raw_text":"x".repeat(size)}))
                    }
                    ScriptOperation::WriteFile => ToolResultEnvelope::ok(
                        "write",
                        "",
                        json!({"observed_reads":execution.reads().len()}),
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
    let capacity = WorkspaceExecutionHost::new(1, 1).unwrap();
    let worker = tokio::spawn(execute(
        "await workspace.run_command({program:'loop'});".into(),
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
    assert_eq!("cancelled", result.status);
    assert_eq!(0, capacity.metrics().active_workers);
    let facts = facts(&result);
    assert!(!facts.cleanup_incomplete);
    assert_eq!(1, facts.actions.len());
    assert_eq!(
        WorkspaceScriptActionStatus::Cancelled,
        facts.actions[0].status
    );
    assert_eq!("command finished cleanup", facts.actions[0].summary);
}

#[tokio::test]
async fn time_limit_interrupts_busy_javascript_and_includes_waiting_for_capacity() {
    let capacity = WorkspaceExecutionHost::new(1, 1).unwrap();
    let limits = WorkspaceScriptLimits {
        timeout_ms: 100,
        ..Default::default()
    };
    let permit = capacity.acquire_worker().await.unwrap();
    let waiting = execute(
        "return 1;".into(),
        empty_host(),
        limits.clone(),
        capacity.clone(),
        Arc::new(Notify::new()),
    )
    .await;
    assert!(facts(&waiting).timed_out);
    assert_eq!(1, capacity.metrics().started_workers);
    drop(permit);

    let running = execute(
        "while (true) {}".into(),
        empty_host(),
        limits,
        capacity.clone(),
        Arc::new(Notify::new()),
    )
    .await;
    assert!(facts(&running).timed_out);
    assert!(!facts(&running).cleanup_incomplete);
    assert_eq!(0, capacity.metrics().active_workers);
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
        for await (const item of workspace.output_lines({name: "sample", limit: 6})) {
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

async fn run<F, Fut>(
    code: &str,
    host: TestHost<F>,
    limits: WorkspaceScriptLimits,
) -> ToolResultEnvelope
where
    F: Fn(ScriptCall, Arc<ScriptExecution>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ToolResultEnvelope> + Send + 'static,
{
    execute(
        code.into(),
        Arc::new(host),
        limits,
        WorkspaceExecutionHost::new(1, 2).unwrap(),
        Arc::new(Notify::new()),
    )
    .await
}

fn empty_host() -> Arc<dyn ScriptHost> {
    Arc::new(TestHost {
        operations: Vec::new(),
        callback: |_, _| async { ToolResultEnvelope::error("unexpected host call", "test setup") },
    })
}
