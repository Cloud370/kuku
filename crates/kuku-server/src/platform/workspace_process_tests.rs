use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use cap_std::fs::Dir;

use super::{IdentityBoundProcessRoot, ProcessChunk, ProcessChunkSink, ProcessLimits, RootCommand};
use crate::api::ApiError;

struct CancellingSink {
    cancellation: Arc<tokio::sync::Notify>,
    child_pid: Option<i32>,
}

struct SilentSink;

impl ProcessChunkSink for SilentSink {
    fn push<'a>(
        &'a mut self,
        _chunk: ProcessChunk,
    ) -> Pin<Box<dyn Future<Output = Result<(), ApiError>> + Send + 'a>> {
        Box::pin(async { panic!("silent command unexpectedly produced output") })
    }
}

impl ProcessChunkSink for CancellingSink {
    fn push<'a>(
        &'a mut self,
        chunk: ProcessChunk,
    ) -> Pin<Box<dyn Future<Output = Result<(), ApiError>> + Send + 'a>> {
        Box::pin(async move {
            let line = std::str::from_utf8(chunk.bytes()).unwrap().trim();
            let child_pid = line.strip_prefix("started:").unwrap().parse().unwrap();
            self.child_pid = Some(child_pid);
            self.cancellation.notify_one();
            Ok(())
        })
    }
}

#[cfg(unix)]
#[tokio::test]
async fn external_cancellation_streams_then_reaps_the_process_tree() {
    let workspace = tempfile::tempdir().unwrap();
    let directory =
        Arc::new(Dir::open_ambient_dir(workspace.path(), cap_std::ambient_authority()).unwrap());
    let root = IdentityBoundProcessRoot::new(directory, workspace.path().to_path_buf()).unwrap();
    let cancellation = Arc::new(tokio::sync::Notify::new());
    let mut sink = CancellingSink {
        cancellation: cancellation.clone(),
        child_pid: None,
    };

    let status = root
        .stream_with_notification(
            RootCommand::new("sh").args([
                "-c",
                "sh -c 'while :; do sleep 30; done' & child=$!; printf 'started:%s\\n' \"$child\"; wait",
            ]),
            ProcessLimits::new(Duration::from_secs(30), 4096).unwrap(),
            &mut sink,
            Some(cancellation),
        )
        .await
        .unwrap();

    assert!(status.cancelled());
    assert!(!status.success());
    let child_pid = rustix::process::Pid::from_raw(sink.child_pid.unwrap()).unwrap();
    assert!(rustix::process::kill_process(child_pid, rustix::process::Signal::CONT).is_err());
}

#[tokio::test]
async fn early_cancellation_stops_a_command_that_never_writes_output() {
    let workspace = tempfile::tempdir().unwrap();
    let directory =
        Arc::new(Dir::open_ambient_dir(workspace.path(), cap_std::ambient_authority()).unwrap());
    let root = IdentityBoundProcessRoot::new(directory, workspace.path().to_path_buf()).unwrap();
    let cancellation = Arc::new(tokio::sync::Notify::new());
    cancellation.notify_one();
    let mut sink = SilentSink;

    let status = tokio::time::timeout(
        Duration::from_secs(1),
        root.stream_with_notification(
            RootCommand::new("this-command-must-not-be-started"),
            ProcessLimits::new(Duration::from_secs(30), 4096).unwrap(),
            &mut sink,
            Some(cancellation),
        ),
    )
    .await
    .unwrap()
    .unwrap();

    assert!(status.cancelled());
}
