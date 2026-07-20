use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use httpmock::MockServer;
use kuku::event::{EventPayload, EventStore, ExecutionScope};
use kuku::query::{TaskQueryContext, WorkspaceQueryCapability};

mod common;

#[derive(Debug)]
struct ReplacingCapability {
    root: std::path::PathBuf,
    verifications: AtomicUsize,
}

impl WorkspaceQueryCapability for ReplacingCapability {
    fn workspace_id(&self) -> &str {
        "wsp_111111111111111111111111"
    }

    fn verify_identity(&self) -> kuku::Result<()> {
        if self.verifications.fetch_add(1, Ordering::SeqCst) == 1 {
            let displaced = self.root.with_extension("displaced");
            std::fs::rename(&self.root, &displaced)?;
            std::fs::create_dir(&self.root)?;
            std::fs::write(self.root.join("identity.txt"), "replacement")?;
        }
        Ok(())
    }

    fn file_exists(&self, relative_path: &str) -> kuku::Result<bool> {
        Ok(self
            .root
            .with_extension("displaced")
            .join(relative_path)
            .is_file())
    }

    fn read_file(&self, relative_path: &str, max_bytes: usize) -> kuku::Result<Vec<u8>> {
        let bytes = std::fs::read(self.root.with_extension("displaced").join(relative_path))?;
        if bytes.len() > max_bytes {
            return Err(kuku::Error::WorkspaceUnavailable(
                "workspace file exceeds read limit".to_string(),
            ));
        }
        Ok(bytes)
    }

    fn read_skill_source(
        &self,
        selected: &kuku::event::SkillContextFact,
        max_bytes: usize,
    ) -> kuku::Result<Vec<u8>> {
        let path = selected.source.relative_path.as_ref().ok_or_else(|| {
            kuku::Error::InvalidTaskContext("selected skill path is missing".to_string())
        })?;
        self.read_file(path.as_str(), max_bytes)
    }

    fn write_file(
        &self,
        relative_path: &str,
        contents: &[u8],
        max_bytes: usize,
    ) -> kuku::Result<()> {
        if contents.len() > max_bytes {
            return Err(kuku::Error::WorkspaceUnavailable(
                "workspace write exceeds limit".to_string(),
            ));
        }
        Ok(std::fs::write(
            self.root.with_extension("displaced").join(relative_path),
            contents,
        )?)
    }

    fn list_entries(
        &self,
        _relative_path: &str,
        _max_entries: usize,
    ) -> kuku::Result<Vec<kuku::WorkspaceEntry>> {
        Ok(Vec::new())
    }

    fn run_command<'a>(
        &'a self,
        request: kuku::WorkspaceCommandRequest,
        events: Option<tokio::sync::mpsc::Sender<kuku::WorkspaceCommandEvent>>,
        _cancellation: kuku::WorkspaceCommandCancellation,
    ) -> Pin<
        Box<
            dyn std::future::Future<Output = kuku::Result<kuku::WorkspaceCommandOutput>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let started = std::time::Instant::now();
            #[cfg(windows)]
            let mut command = {
                let mut command = tokio::process::Command::new("cmd");
                command.arg("/C").arg(&request.command);
                command
            };
            #[cfg(not(windows))]
            let mut command = {
                let mut command = tokio::process::Command::new("sh");
                command.arg("-c").arg(&request.command);
                command
            };
            let output = command
                .current_dir(self.root.with_extension("displaced"))
                .output()
                .await?;
            if let Some(events) = events {
                for index in 0..1024 {
                    events
                        .send(kuku::WorkspaceCommandEvent::Stdout(
                            format!("{index:04},").into_bytes(),
                        ))
                        .await
                        .map_err(|_| {
                            kuku::Error::WorkspaceUnavailable(
                                "command output receiver closed".to_string(),
                            )
                        })?;
                }
            }
            Ok(kuku::WorkspaceCommandOutput {
                exit_code: output.status.code(),
                timed_out: false,
                cancelled: false,
                stdout: output.stdout,
                stderr: output.stderr,
                duration_ms: started.elapsed().as_millis() as u64,
            })
        })
    }
}

fn task_store(path: &std::path::Path, scope: &ExecutionScope) -> EventStore {
    let mut store = EventStore::open(path).unwrap();
    let receipt = kuku::event::CommandReceipt::new(
        "create-task",
        "digest",
        kuku::event::CommandResult::TaskCreated {
            task_id: scope.task_id.clone(),
        },
    )
    .unwrap();
    let run = kuku::event::RunFact {
        run_id: scope.run_id.clone(),
        task_id: scope.task_id.clone(),
        state: kuku::event::RunState::Queued,
        started_at: "2026-07-20T00:00:00Z".to_string(),
        finished_at: None,
        summary: None,
        checks: None,
        metrics: None,
        workspace_changes: None,
    };
    let transaction = kuku::event::TaskTransaction::try_new(
        kuku::event::TaskRevision::try_new(0).unwrap(),
        receipt,
        vec![
            kuku::event::TaskEvent::TaskCreated {
                task_id: scope.task_id.clone(),
                workspace_id: scope.workspace_id.clone(),
                title: "Task".to_string(),
                created_at: "2026-07-20T00:00:00Z".to_string(),
            },
            kuku::event::TaskEvent::RunQueued { run },
        ],
    )
    .unwrap();
    store
        .append_synced(EventPayload::TaskLedger(
            kuku::event::TaskLedgerRecord::Control(transaction),
        ))
        .unwrap();
    store
}

fn tool_response(name: &str, input: serde_json::Value) -> String {
    let input = serde_json::to_string(&input).unwrap();
    format!(
        "event: message_start\ndata: {{\"type\":\"message_start\",\"message\":{{\"id\":\"msg_1\",\"model\":\"test-model\",\"content\":[],\"usage\":{{\"input_tokens\":1,\"output_tokens\":1}}}}}}\n\n\
         event: content_block_start\ndata: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"tool_use\",\"id\":\"tool_1\",\"name\":{name:?},\"input\":{{}}}}}}\n\n\
         event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"input_json_delta\",\"partial_json\":{input:?}}}}}\n\n\
         event: content_block_stop\ndata: {{\"type\":\"content_block_stop\",\"index\":0}}\n\n\
         event: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"tool_use\"}},\"usage\":{{\"output_tokens\":1}}}}\n\n\
         event: message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n"
    )
}

async fn start_tool_run(
    root: &std::path::Path,
    home: &std::path::Path,
    tool: &str,
    input: serde_json::Value,
) -> kuku::Run {
    let server = MockServer::start();
    let response = tool_response(tool, input);
    server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/v1/messages");
        then.status(200)
            .header("content-type", "text/event-stream")
            .body(response);
    });
    let scope = common::execution_scope();
    let context = TaskQueryContext::new(
        scope.clone(),
        task_store(&home.join("events.jsonl"), &scope),
        Arc::new(ReplacingCapability {
            root: root.to_path_buf(),
            verifications: AtomicUsize::new(0),
        }),
    );
    kuku::query("exercise capability")
        .provider(kuku::Provider::Anthropic)
        .model("test-model")
        .base_url(server.base_url())
        .api_key("test-key")
        .kuku_home(home)
        .task_context(context)
        .start()
        .await
        .unwrap()
}

async fn next_tool_result(run: &mut kuku::Run) -> Option<String> {
    loop {
        match run.next().await.unwrap().unwrap() {
            kuku::UiEvent::PermissionRequested { request } => {
                run.decide(&request.id, kuku::PermissionChoice::Once, None)
                    .await
                    .unwrap();
            }
            kuku::UiEvent::ToolEnd { model_content, .. } => return model_content,
            _ => {}
        }
    }
}

#[tokio::test]
async fn task_read_never_uses_a_replacement_workspace_root() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("workspace");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("identity.txt"), "original").unwrap();
    let home = tempfile::tempdir().unwrap();

    let mut run = start_tool_run(
        &root,
        home.path(),
        "read_file",
        serde_json::json!({"path": "identity.txt"}),
    )
    .await;
    let result = next_tool_result(&mut run).await;

    assert!(!result
        .as_deref()
        .unwrap_or_default()
        .contains("replacement"));
}

#[tokio::test]
async fn task_command_never_executes_in_a_replacement_workspace_root() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("workspace");
    std::fs::create_dir(&root).unwrap();
    let home = tempfile::tempdir().unwrap();

    let mut run = start_tool_run(
        &root,
        home.path(),
        "run_command",
        serde_json::json!({
            "command": "printf replacement > command-marker.txt",
            "timeout": 5,
            "brief": "write marker"
        }),
    )
    .await;
    let mut streamed = String::new();
    loop {
        match run.next().await.unwrap().unwrap() {
            kuku::UiEvent::PermissionRequested { request } => {
                run.decide(&request.id, kuku::PermissionChoice::Once, None)
                    .await
                    .unwrap();
            }
            kuku::UiEvent::ToolOutput {
                event: kuku::ToolEvent::Stdout { text },
                ..
            } => streamed.push_str(&text),
            kuku::UiEvent::ToolEnd { .. } => break,
            _ => {}
        }
    }

    assert!(!root.join("command-marker.txt").exists());
    assert_eq!(
        streamed,
        (0..1024)
            .map(|index| format!("{index:04},"))
            .collect::<String>()
    );
}

#[tokio::test]
async fn task_write_targets_the_original_capability_after_root_replacement() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("workspace");
    std::fs::create_dir(&root).unwrap();
    let home = tempfile::tempdir().unwrap();

    let mut run = start_tool_run(
        &root,
        home.path(),
        "write_file",
        serde_json::json!({
            "path": "created.txt",
            "content": "capability",
            "brief": "create marker"
        }),
    )
    .await;
    let _ = next_tool_result(&mut run).await;

    assert_eq!(
        std::fs::read_to_string(root.with_extension("displaced").join("created.txt")).unwrap(),
        "capability"
    );
    assert!(!root.join("created.txt").exists());
}
