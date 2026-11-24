use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::process::{Command, Stdio};
use std::ptr;
use std::sync::Arc;

use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;
use tokio::time::{Duration, sleep, timeout};
use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Console::{GetConsoleProcessList, GetConsoleWindow};
use windows_sys::Win32::System::JobObjects::IsProcessInJob;
use windows_sys::Win32::System::Threading::{
    DETACHED_PROCESS, GetExitCodeProcess, OpenProcess, PROCESS_QUERY_INFORMATION,
    PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject,
};

use crate::tool::ToolResultEnvelope;

use super::super::test_helpers::workspace;
use super::{
    CommandEvent, OutputLimits, OutputRead, OutputStream, read_output, run_command,
    run_command_with_output,
};

const CHILD_START_TIMEOUT: Duration = Duration::from_secs(10);
const PROCESS_EXIT_TIMEOUT: Duration = Duration::from_secs(3);
const DETACHED_PROBE_ENV: &str = "KUKU_TEST_DETACHED_COMMAND";

#[tokio::test]
async fn windows_system_interpreters_are_found_without_path_entries() {
    let mut fixture = tokio::process::Command::new(std::env::current_exe().unwrap());
    fixture
        .env("PATH", "")
        .args([
            "--ignored",
            "--exact",
            "tool::builtin::run_command::windows_tests::system_shell_fixture",
            "--nocapture",
        ])
        .kill_on_drop(true);
    let output = timeout(Duration::from_secs(30), fixture.output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("test result: ok. 1 passed"));
}

#[tokio::test]
#[ignore = "subprocess fixture for Windows system shells with a controlled PATH"]
async fn system_shell_fixture() {
    let registry = crate::tool::builtin_registry(false);
    let tool = registry
        .iter()
        .find(|tool| tool.name == "run_command")
        .unwrap();
    let shells = tool.input_schema["properties"]["shell"]["enum"]
        .as_array()
        .unwrap();
    assert!(shells.contains(&serde_json::json!("cmd")));
    let dir = workspace();
    let result = run_command(
        &serde_json::json!({"shell":"cmd","command":"echo system cmd","cwd":".","timeout":CHILD_START_TIMEOUT.as_secs()}),
        dir.path(),
        None,
        None,
    )
    .await;
    assert_eq!("ok", result.status, "{result:?}");
    assert!(result.model_content.contains("system cmd"));
    assert!(shells.contains(&serde_json::json!("powershell")));
    windows_large_output_is_bounded_and_reopens_without_event_consumers().await;
}

#[tokio::test]
async fn windows_detached_tool_does_not_create_a_console_window() {
    let mut detached = tokio::process::Command::new(std::env::current_exe().unwrap());
    detached
        .args([
            "--ignored",
            "--exact",
            "tool::builtin::run_command::windows_tests::detached_command_fixture",
            "--nocapture",
        ])
        .creation_flags(DETACHED_PROCESS)
        .env(DETACHED_PROBE_ENV, "1")
        .kill_on_drop(true);
    let output = timeout(Duration::from_secs(15), detached.output())
        .await
        .unwrap()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}\n{stderr}");
    assert!(
        stdout.contains("tool console window=false"),
        "{stdout}\n{stderr}"
    );
    eprintln!("{stdout}");
}

#[tokio::test]
#[ignore = "subprocess fixture for the Windows console test"]
async fn detached_command_fixture() {
    if std::env::var_os(DETACHED_PROBE_ENV).is_none() {
        return;
    }
    let (window, attached) = own_console();
    println!("detached parent console window={window} attached_processes={attached}");
    assert_eq!((false, 0), (window, attached));

    let dir = workspace();
    place_test_executable(&dir.path().join("console-fixture.exe"));
    let outputs = dir.path().join("outputs");
    let result = run_command_with_output(
        &serde_json::json!({
            "program":"./console-fixture.exe",
            "args":["--ignored","--exact","tool::builtin::run_command::windows_tests::command_console_fixture","--nocapture"],
            "cwd":".","timeout":CHILD_START_TIMEOUT.as_secs(),"brief":"observe the background command console"
        }),
        dir.path(),
        None,
        None,
        &outputs,
        "console",
        OutputLimits::default(),
    )
    .await;
    assert_eq!("ok", result.status, "{result:?}");
    assert_eq!(0, result.structured.as_ref().unwrap()["exit_code"]);
    let capture = read_output(&outputs, "console", OutputRead::default()).unwrap();
    assert!(capture.complete);
    let text: String = capture.segments.into_iter().map(|part| part.text).collect();
    println!("{text}");
}

#[test]
#[ignore = "reports the console of a subprocess launched by the SDK tool"]
fn command_console_fixture() {
    if std::env::var_os(DETACHED_PROBE_ENV).is_none() {
        return;
    }
    let (window, attached) = own_console();
    println!("tool console window={window} attached_processes={attached}");
}

fn own_console() -> (bool, u32) {
    let mut pid = 0;
    // SAFETY: These APIs only inspect the calling process's console. One writable slot suffices to query the process count.
    unsafe {
        (
            !GetConsoleWindow().is_null(),
            GetConsoleProcessList(&mut pid, 1),
        )
    }
}

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
        // SAFETY: The pid was written by this test's owned child; the requested rights only inspect, wait for, or clean up that process.
        let raw = unsafe { OpenProcess(access, 0, pid) };
        assert!(
            !raw.is_null(),
            "could not observe child {pid}: {}",
            io::Error::last_os_error()
        );
        // SAFETY: OpenProcess returned a new valid unique handle, whose ownership is transferred to OwnedHandle.
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut in_job = 0;
        // SAFETY: The live process handle has query rights, and the writable result remains valid for this call.
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
        // SAFETY: This owned process handle has synchronize rights and remains valid until this observer is dropped.
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
        // SAFETY: This live handle has process query rights, and the exit-code pointer stays writable throughout the call.
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
            // SAFETY: This handle identifies only a descendant created by this test; terminate rights were requested for failure cleanup.
            unsafe { TerminateProcess(self.handle.as_raw_handle(), 1) };
            // SAFETY: The same owned process handle remains live while bounded failure cleanup waits for its exit.
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
        let local_exe = dir.path().join("tree-fixture.exe");
        place_test_executable(&local_exe);
        let workspace = dir.path().to_path_buf();
        let cancel = Arc::new(Notify::new());
        let worker_cancel = cancel.clone();
        let worker = tokio::spawn(async move {
            run_command_with_output(
                &serde_json::json!({
                    "program":"./tree-fixture.exe",
                    "args":["--ignored","--exact","tool::builtin::run_command::windows_tests::command_tree_fixture"],
                    "cwd":".","timeout":timeout_seconds,"brief":"verify Windows Job cleanup"
                }),
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

fn place_test_executable(path: &Path) {
    let test_exe = std::env::current_exe().unwrap();
    if std::fs::hard_link(&test_exe, path).is_err() {
        std::fs::copy(test_exe, path).unwrap();
    }
}

#[tokio::test]
async fn windows_shell_runs_quoted_powershell_in_the_workspace() {
    let dir = workspace();
    let path = dir.path().join("工作区 with spaces");
    std::fs::create_dir(&path).unwrap();
    let command = r#"
Set-Content -LiteralPath 'gate started.txt' -Value 'ready with spaces'
$payload = [pscustomobject]@{ Text = '"quoted" | literal & payload' }
$payload | ForEach-Object { $_.Text }
[Console]::Error.WriteLine('fixture raw stderr')
Write-Error -Message ('fixture PowerShell ' + 'error')
exit 7
"#;
    let output_dir = path.join("outputs");
    let result = run_command_with_output(
        &serde_json::json!({"shell":"powershell","command":command,"cwd":".","timeout":CHILD_START_TIMEOUT.as_secs(),"brief":"write a marker with PowerShell"}),
        &path,
        None,
        None,
        &output_dir,
        "powershell",
        OutputLimits::default(),
    )
    .await;
    assert_eq!("ok", result.status, "{}", result.model_content);
    assert_eq!(7, result.structured.unwrap()["exit_code"]);
    assert_eq!(
        "ready with spaces",
        std::fs::read_to_string(path.join("gate started.txt"))
            .unwrap_or_else(|error| panic!(
                "PowerShell did not write its marker: {error}; {}",
                result.model_content
            ))
            .trim_end()
    );
    let saved = read_output(&output_dir, "powershell", OutputRead::default()).unwrap();
    let text = |stream| {
        saved
            .segments
            .iter()
            .filter(|segment| segment.stream == stream)
            .map(|segment| segment.text.as_str())
            .collect::<String>()
    };
    let stdout = text(OutputStream::Stdout);
    let stderr = text(OutputStream::Stderr);
    assert_eq!("\"quoted\" | literal & payload", stdout.trim_end());
    assert!(
        stderr.lines().any(|line| line == "fixture raw stderr"),
        "{stderr}"
    );
    assert!(stderr.contains("fixture PowerShell error"), "{stderr}");
    assert!(!stderr.contains("#< CLIXML"), "{stderr}");
}

#[tokio::test]
async fn windows_powershell_bare_dash_is_evaluated_as_script_text() {
    let dir = workspace();
    let result = run_command(
        &serde_json::json!({"shell":"powershell","command":"-","cwd":".","timeout":CHILD_START_TIMEOUT.as_secs()}),
        dir.path(),
        None,
        None,
    )
    .await;
    assert_eq!("ok", result.status, "{result:?}");
    assert_eq!(1, result.structured.unwrap()["exit_code"]);
}

#[tokio::test]
async fn windows_shell_runs_a_quoted_executable_and_the_next_command() {
    let dir = workspace();
    place_test_executable(&dir.path().join("quoted executable.exe"));
    std::fs::write(
        dir.path().join("workspace marker.txt"),
        "workspace marker\r\n",
    )
    .unwrap();
    let command = r#"".\quoted executable.exe" --list --exact "tool::builtin::run_command::windows_tests::command_tree_fixture" && type "workspace marker.txt""#;
    let result = run_command(
        &serde_json::json!({"shell":"cmd","command":command,"cwd":".","timeout":CHILD_START_TIMEOUT.as_secs(),"brief":"list a test and read a marker"}),
        dir.path(),
        None,
        None,
    )
    .await;
    assert_eq!("ok", result.status, "{}", result.model_content);
    assert_eq!(
        0,
        result.structured.unwrap()["exit_code"],
        "{}",
        result.model_content
    );
    assert!(
        result.model_content.contains("command_tree_fixture: test"),
        "{}",
        result.model_content
    );
    assert!(
        result.model_content.contains("workspace marker"),
        "{}",
        result.model_content
    );
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
    let mut tree = RunningTree::start(20).await;
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(20)).await;
    tokio::time::resume();
    let result = tree.result(PROCESS_EXIT_TIMEOUT).await;
    assert_eq!("error", result.status);
    assert_eq!(true, result.structured.unwrap()["timed_out"]);
    tree.assert_descendants_exited().await;
}

#[tokio::test]
async fn windows_dropped_command_future_stops_children_and_grandchildren() {
    let mut tree = RunningTree::start(20).await;
    tree.worker.abort();
    assert!(
        timeout(PROCESS_EXIT_TIMEOUT, &mut tree.worker)
            .await
            .unwrap()
            .unwrap_err()
            .is_cancelled()
    );
    tree.assert_descendants_exited().await;
}

async fn windows_large_output_is_bounded_and_reopens_without_event_consumers() {
    let dir = workspace();
    let outputs = dir.path().join("outputs");
    let (tx, _rx) = mpsc::channel::<CommandEvent>(1);
    let result = timeout(Duration::from_secs(15), run_command_with_output(
        &serde_json::json!({"shell":"powershell","cwd":".","command":"[Console]::Out.Write('x' * 2097152); [Console]::Error.Write('e' * 4096)","timeout":CHILD_START_TIMEOUT.as_secs(),"brief":"verify bounded Windows output"}),
        dir.path(), Some(tx), None, &outputs, "large", OutputLimits { max_output_bytes: 8192, max_preview_bytes: 1024 },
    )).await.unwrap();
    assert_eq!("ok", result.status, "{result:?}");
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
    assert!(
        reopened
            .segments
            .iter()
            .all(|segment| segment.text.bytes().all(|byte| byte == b'x'))
    );
    eprintln!(
        "{}",
        serde_json::json!({"windows_output_file_bytes": file_bytes, "reopened_payload_bytes": reopened.total_bytes, "facts": facts})
    );
}

#[tokio::test]
async fn windows_batch_execution_requires_an_explicit_cmd_interpreter() {
    let dir = workspace();
    std::fs::write(
        dir.path().join("fixture.cmd"),
        b"@echo off\r\necho batch launched\r\nexit /b 4\r\n",
    )
    .unwrap();
    let native = serde_json::json!({"program":"./fixture.cmd","args":[],"cwd":".","timeout":CHILD_START_TIMEOUT.as_secs()});
    let rejected = run_command(&native, dir.path(), None, None).await;
    assert_eq!("error", rejected.status);
    assert!(
        rejected
            .model_content
            .contains("explicit cmd.exe invocation")
    );
    let shell = serde_json::json!({"program":"cmd.exe","args":["/D","/S","/C","fixture.cmd"],"cwd":".","timeout":CHILD_START_TIMEOUT.as_secs()});
    let explicit = run_command(&shell, dir.path(), None, None).await;
    assert_eq!("ok", explicit.status, "{explicit:?}");
    assert_eq!(4, explicit.structured.unwrap()["exit_code"]);
    assert!(explicit.model_content.contains("batch launched"));
}
