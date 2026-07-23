use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use cap_std::fs::Dir;

use super::{
    IdentityBoundProcessRoot, ProcessCancellation, ProcessChunk, ProcessChunkSink, ProcessLimits,
    RootCommand,
};
use crate::api::ApiError;

#[cfg(unix)]
struct CancellingSink {
    cancellation: ProcessCancellation,
    child_pid: Option<i32>,
}

struct SilentSink;

#[cfg(unix)]
struct ProcessTreeBarrierSink {
    started: Option<tokio::sync::oneshot::Sender<(i32, i32)>>,
}

impl ProcessChunkSink for SilentSink {
    fn push<'a>(
        &'a mut self,
        _chunk: ProcessChunk,
    ) -> Pin<Box<dyn Future<Output = Result<(), ApiError>> + Send + 'a>> {
        Box::pin(async { panic!("silent command unexpectedly produced output") })
    }
}

#[cfg(unix)]
impl ProcessChunkSink for CancellingSink {
    fn push<'a>(
        &'a mut self,
        chunk: ProcessChunk,
    ) -> Pin<Box<dyn Future<Output = Result<(), ApiError>> + Send + 'a>> {
        Box::pin(async move {
            let line = std::str::from_utf8(chunk.bytes()).unwrap().trim();
            let child_pid = line.strip_prefix("started:").unwrap().parse().unwrap();
            self.child_pid = Some(child_pid);
            self.cancellation.cancel();
            Ok(())
        })
    }
}

#[cfg(unix)]
impl ProcessChunkSink for ProcessTreeBarrierSink {
    fn push<'a>(
        &'a mut self,
        chunk: ProcessChunk,
    ) -> Pin<Box<dyn Future<Output = Result<(), ApiError>> + Send + 'a>> {
        Box::pin(async move {
            let line = std::str::from_utf8(chunk.bytes()).unwrap().trim();
            let mut fields = line.split(':');
            assert_eq!(Some("started"), fields.next());
            let leader = fields.next().unwrap().parse().unwrap();
            let descendant = fields.next().unwrap().parse().unwrap();
            self.started
                .take()
                .unwrap()
                .send((leader, descendant))
                .unwrap();
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
    let cancellation = ProcessCancellation::new();
    let mut sink = CancellingSink {
        cancellation: cancellation.clone(),
        child_pid: None,
    };

    let status = root
        .stream_with_cancellation(
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
    let cancellation = ProcessCancellation::new();
    cancellation.cancel();
    let mut sink = SilentSink;

    let status = tokio::time::timeout(
        Duration::from_secs(1),
        root.stream_with_cancellation(
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

#[cfg(unix)]
#[tokio::test]
async fn dropping_stream_future_reaps_a_silent_process_tree() {
    let workspace = tempfile::tempdir().unwrap();
    let marker = workspace.path().join("descendant-exited");
    let directory =
        Arc::new(Dir::open_ambient_dir(workspace.path(), cap_std::ambient_authority()).unwrap());
    let root = IdentityBoundProcessRoot::new(directory, workspace.path().to_path_buf()).unwrap();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let stream = tokio::spawn(async move {
        let mut sink = ProcessTreeBarrierSink {
            started: Some(started_tx),
        };
        root.stream_with_cancellation(
            RootCommand::new("sh").args([
                "-c",
                "sh -c 'trap \"printf marker > descendant-exited\" EXIT; while :; do sleep 30; done' >/dev/null 2>&1 & child=$!; printf 'started:%s:%s\\n' \"$$\" \"$child\"; wait",
            ]),
            ProcessLimits::new(Duration::from_secs(30), 4096).unwrap(),
            &mut sink,
            None,
        )
        .await
    });

    let (leader, descendant) = tokio::time::timeout(Duration::from_secs(1), started_rx)
        .await
        .unwrap()
        .unwrap();
    stream.abort();
    assert!(stream.await.unwrap_err().is_cancelled());

    tokio::time::timeout(Duration::from_secs(1), async {
        while process_exists(leader) || process_exists(descendant) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!marker.exists());
}

#[cfg(unix)]
fn process_exists(pid: i32) -> bool {
    let pid = rustix::process::Pid::from_raw(pid).unwrap();
    rustix::process::kill_process(pid, rustix::process::Signal::CONT).is_ok()
}
