use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::process::{Command, Stdio};
use std::ptr;
use std::sync::Arc;

use tokio::sync::{mpsc, Notify};
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout, Duration};
use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::JobObjects::IsProcessInJob;
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, TerminateProcess, WaitForSingleObject,
    PROCESS_QUERY_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
};

use crate::tool::ToolResultEnvelope;

use super::super::test_helpers::workspace;
use super::{read_output, run_command_with_output, CommandEvent, OutputLimits, OutputRead};

const CHILD_START_TIMEOUT: Duration = Duration::from_secs(10);
const PROCESS_EXIT_TIMEOUT: Duration = Duration::from_secs(3);

#[test]
#[ignore = "subprocess fixture for the Windows process tree tests"]
fn command_tree_fixture() {
    if !Path::new("tree-fixture.exe").is_file() {
        return;
    }
    let mut grandchild = Command::new("ping.exe")
        .args(["-n", "61", "127.0.0.1"])
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    std::fs::write("grandchild.pid", grandchild.id().to_string()).unwrap();
    std::fs::write("child.pid", std::process::id().to_string()).unwrap();
    while !Path::new("release").exists() {
        assert!(grandchild.try_wait().unwrap().is_none());
        std::thread::sleep(Duration::from_millis(20));
    }
}

struct ObservedProcess {
    pid: u32,
    handle: OwnedHandle,
}

impl ObservedProcess {
    fn open(pid: u32) -> Self {
        let access = PROCESS_QUERY_INFORMATION | PROCESS_SYNCHRONIZE | PROCESS_TERMINATE;
        // NOTE: The pid was written by this test's owned child; the requested rights only inspect, wait for, or clean up that process.
        let raw = unsafe { OpenProcess(access, 0, pid) };
        assert!(
            !raw.is_null(),
            "could not observe child {pid}: {}",
            io::Error::last_os_error()
        );
        // NOTE: OpenProcess returned a new valid unique handle, whose ownership is transferred to OwnedHandle.
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut in_job = 0;
        // NOTE: The live process handle has query rights, and the writable result remains valid for this call.
        assert_ne!(0, unsafe {
            IsProcessInJob(handle.as_raw_handle(), ptr::null_mut(), &mut in_job)
        });
        assert_ne!(0, in_job, "child {pid} did not inherit a Windows Job");
        eprintln!(
            "{}",
            serde_json::json!({"windows_process": pid, "in_job": true, "state": "running"})
        );
        Self { pid, handle }
    }

    fn is_running(&self) -> bool {
        // NOTE: This owned process handle has synchronize rights and remains valid until this observer is dropped.
        match unsafe { WaitForSingleObject(self.handle.as_raw_handle(), 0) } {
            WAIT_OBJECT_0 => false,
            WAIT_TIMEOUT => true,
            value => panic!(
                "process {} wait failed with {value}: {}",
                self.pid,
                io::Error::last_os_error()
            ),
        }
    }

    async fn assert_exited(&self) {
        timeout(PROCESS_EXIT_TIMEOUT, async {
            while self.is_running() {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("command left descendant {} running", self.pid));
        let mut code = 0;
        // NOTE: This live handle has process query rights, and the exit-code pointer stays writable throughout the call.
        assert_ne!(0, unsafe {
            GetExitCodeProcess(self.handle.as_raw_handle(), &mut code)
        });
        eprintln!(
            "{}",
            serde_json::json!({"windows_process": self.pid, "state": "exited", "exit_code": code})
        );
    }
}

impl Drop for ObservedProcess {
    fn drop(&mut self) {
        if self.is_running() {
            // NOTE: This handle identifies only a descendant created by this test; terminate rights were requested for failure cleanup.
            unsafe { TerminateProcess(self.handle.as_raw_handle(), 1) };
            // NOTE: The same owned process handle remains live while bounded failure cleanup waits for its exit.
            unsafe {
                WaitForSingleObject(
                    self.handle.as_raw_handle(),
                    PROCESS_EXIT_TIMEOUT.as_millis() as u32,
                )
            };
        }
    }
}

struct RunningTree {
    dir: tempfile::TempDir,
    worker: JoinHandle<ToolResultEnvelope>,
    cancel: Arc<Notify>,
    child: ObservedProcess,
    grandchild: ObservedProcess,
}

impl RunningTree {
    async fn start(timeout_seconds: u64) -> Self {
        let dir = workspace();
        let test_exe = std::env::current_exe().unwrap();
        let local_exe = dir.path().join("tree-fixture.exe");
        if std::fs::hard_link(&test_exe, &local_exe).is_err() {
            std::fs::copy(test_exe, local_exe).unwrap();
        }
        let command = "tree-fixture.exe --ignored --exact tool::builtin::run_command::windows_tests::command_tree_fixture";
        let workspace = dir.path().to_path_buf();
        let cancel = Arc::new(Notify::new());
        let worker_cancel = cancel.clone();
        let worker = tokio::spawn(async move {
            run_command_with_output(
                &serde_json::json!({"command": command, "timeout": timeout_seconds, "brief": "verify Windows Job cleanup"}),
                &workspace, None, Some(worker_cancel), &workspace.join("outputs"), "windows-tree", OutputLimits::default(),
            ).await
        });
        let child =
            ObservedProcess::open(wait_for_pid(&dir.path().join("child.pid"), &worker).await);
        let grandchild =
            ObservedProcess::open(wait_for_pid(&dir.path().join("grandchild.pid"), &worker).await);
        assert!(child.is_running());
        assert!(grandchild.is_running());
        Self {
            dir,
            worker,
            cancel,
            child,
            grandchild,
        }
    }

    async fn result(&mut self, wait: Duration) -> ToolResultEnvelope {
        let result = timeout(wait, &mut self.worker).await.unwrap().unwrap();
        eprintln!(
            "{}",
            serde_json::json!({"windows_command_status": result.status, "facts": result.structured})
        );
        result
    }

    async fn assert_descendants_exited(&self) {
        self.child.assert_exited().await;
        self.grandchild.assert_exited().await;
    }
}

async fn wait_for_pid(path: &Path, worker: &JoinHandle<ToolResultEnvelope>) -> u32 {
    timeout(CHILD_START_TIMEOUT, async {
        loop {
            if let Ok(text) = std::fs::read_to_string(path) {
                if let Ok(pid) = text.trim().parse() {
                    return pid;
                }
            }
            assert!(
                !worker.is_finished(),
                "command exited before writing {}",
                path.display()
            );
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("command did not write {} in time", path.display()))
}

#[tokio::test]
async fn windows_normal_command_exit_stops_remaining_descendants() {
    let mut tree = RunningTree::start(20).await;
    std::fs::write(tree.dir.path().join("release"), "release").unwrap();
    let result = tree.result(PROCESS_EXIT_TIMEOUT).await;
    assert_eq!("ok", result.status);
    assert_eq!(0, result.structured.unwrap()["exit_code"]);
    tree.assert_descendants_exited().await;
}

#[tokio::test]
async fn windows_cancellation_stops_children_and_grandchildren() {
    let mut tree = RunningTree::start(20).await;
    tree.cancel.notify_waiters();
    let result = tree.result(PROCESS_EXIT_TIMEOUT).await;
    assert_eq!("cancelled", result.status);
    tree.assert_descendants_exited().await;
}

#[tokio::test]
async fn windows_timeout_stops_children_and_grandchildren() {
    let mut tree = RunningTree::start(5).await;
    let result = tree.result(Duration::from_secs(8)).await;
    assert_eq!("error", result.status);
    assert_eq!(true, result.structured.unwrap()["timed_out"]);
    tree.assert_descendants_exited().await;
}

#[tokio::test]
async fn windows_dropped_command_future_stops_children_and_grandchildren() {
    let mut tree = RunningTree::start(20).await;
    tree.worker.abort();
    assert!(timeout(PROCESS_EXIT_TIMEOUT, &mut tree.worker)
        .await
        .unwrap()
        .unwrap_err()
        .is_cancelled());
    tree.assert_descendants_exited().await;
}

#[tokio::test]
async fn windows_large_output_is_bounded_and_reopens_without_event_consumers() {
    let dir = workspace();
    std::fs::write(
        dir.path().join("large.ps1"),
        "[Console]::Out.Write('x' * 2097152); [Console]::Error.Write('e' * 4096)",
    )
    .unwrap();
    let outputs = dir.path().join("outputs");
    let (tx, _rx) = mpsc::channel::<CommandEvent>(1);
    let result = timeout(Duration::from_secs(15), run_command_with_output(
        &serde_json::json!({"command": "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File large.ps1", "timeout": 10, "brief": "verify bounded Windows output"}),
        dir.path(), Some(tx), None, &outputs, "large", OutputLimits { max_output_bytes: 8192, max_preview_bytes: 1024 },
    )).await.unwrap();
    assert_eq!("ok", result.status);
    assert!(result.truncated);
    assert!(result.model_content.len() <= 1024);
    let facts = result.structured.unwrap();
    assert_eq!(2097152, facts["stdout_bytes"]);
    assert_eq!(4096, facts["stderr_bytes"]);
    assert_eq!(true, facts["quota_truncated"]);
    assert!(facts["dropped_events"].as_u64().unwrap() > 0);
    let file_bytes = std::fs::metadata(outputs.join("large.output"))
        .unwrap()
        .len();
    assert!(file_bytes <= 8192);
    let reopened = read_output(&outputs, "large", OutputRead::default()).unwrap();
    assert!(reopened.complete);
    assert!(reopened.capture_truncated);
    assert!(reopened
        .segments
        .iter()
        .all(|segment| segment.text.bytes().all(|byte| byte == b'x')));
    eprintln!(
        "{}",
        serde_json::json!({"windows_output_file_bytes": file_bytes, "reopened_payload_bytes": reopened.total_bytes, "facts": facts})
    );
}
