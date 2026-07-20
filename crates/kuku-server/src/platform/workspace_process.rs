use std::collections::BTreeMap;
use std::future::Future;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cap_std::fs::{Dir, Metadata, MetadataExt};
use tokio::sync::mpsc;

use crate::api::{ApiError, ApiErrorCode};

const READ_CHUNK_BYTES: usize = 8 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(10);
const MAX_PROCESS_OUTPUT_BYTES: usize = 1024 * 1024 * 1024;

/// A shell-free command executed at a workspace root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootCommand {
    program: String,
    arguments: Vec<String>,
    environment: BTreeMap<String, String>,
}

impl RootCommand {
    /// Creates a command for one executable without invoking a shell.
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            arguments: Vec::new(),
            environment: BTreeMap::new(),
        }
    }

    /// Appends one argument.
    pub fn arg(mut self, argument: impl Into<String>) -> Self {
        self.arguments.push(argument.into());
        self
    }

    /// Appends multiple arguments.
    pub fn args<I, S>(mut self, arguments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.arguments.extend(arguments.into_iter().map(Into::into));
        self
    }

    /// Adds one explicitly allowed environment value.
    pub fn env(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.environment.insert(name.into(), value.into());
        self
    }
}

/// Bounds one workspace process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessLimits {
    timeout: Duration,
    max_output_bytes: usize,
}

impl ProcessLimits {
    /// Validates a nonzero timeout and total output budget.
    pub fn new(timeout: Duration, max_output_bytes: usize) -> Result<Self, ApiError> {
        if timeout.is_zero() || max_output_bytes == 0 || max_output_bytes > MAX_PROCESS_OUTPUT_BYTES
        {
            return Err(api_error(
                ApiErrorCode::InvalidRequest,
                "process limits are outside the supported range",
            ));
        }
        Ok(Self {
            timeout,
            max_output_bytes,
        })
    }
}

/// Identifies the pipe that produced a process chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessStream {
    /// Standard output.
    Stdout,
    /// Standard error.
    Stderr,
}

/// Carries one bounded process output chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessChunk {
    stream: ProcessStream,
    bytes: Vec<u8>,
}

impl ProcessChunk {
    /// Returns the pipe that produced this chunk.
    pub fn stream(&self) -> ProcessStream {
        self.stream
    }

    /// Returns the chunk bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Receives bounded chunks while a workspace process is running.
pub trait ProcessChunkSink: Send {
    /// Accepts one chunk or returns an error to cancel the process.
    fn push<'a>(
        &'a mut self,
        chunk: ProcessChunk,
    ) -> Pin<Box<dyn Future<Output = Result<(), ApiError>> + Send + 'a>>;
}

/// Persists workspace-process cancellation across clones and late observers.
#[derive(Debug, Clone, Default)]
pub struct ProcessCancellation {
    cancelled: Arc<AtomicBool>,
}

impl ProcessCancellation {
    /// Creates an active cancellation token.
    pub fn new() -> Self {
        Self::default()
    }

    /// Persists a cancellation request for every current and future clone.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// Returns whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

struct CancelProcessOnDrop {
    cancellation: ProcessCancellation,
    armed: bool,
}

impl CancelProcessOnDrop {
    fn new(cancellation: ProcessCancellation) -> Self {
        Self {
            cancellation,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CancelProcessOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.cancellation.cancel();
        }
    }
}

/// Describes how a workspace process exited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessStatus {
    code: Option<i32>,
    timed_out: bool,
    cancelled: bool,
}

impl ProcessStatus {
    /// Returns true only for a normal zero exit status.
    pub fn success(&self) -> bool {
        !self.timed_out && !self.cancelled && self.code == Some(0)
    }

    /// Returns the platform exit code when available.
    pub fn code(&self) -> Option<i32> {
        self.code
    }

    /// Returns true when the process exceeded its deadline and was reaped.
    pub fn timed_out(&self) -> bool {
        self.timed_out
    }

    /// Returns true when external cancellation terminated and reaped the process tree.
    pub fn cancelled(&self) -> bool {
        self.cancelled
    }
}

/// Contains bounded buffered output from a workspace process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutput {
    status: ProcessStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl ProcessOutput {
    /// Returns the process exit status.
    pub fn status(&self) -> &ProcessStatus {
        &self.status
    }

    /// Returns bounded standard output.
    pub fn stdout(&self) -> &[u8] {
        &self.stdout
    }

    /// Returns bounded standard error.
    pub fn stderr(&self) -> &[u8] {
        &self.stderr
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FileIdentity {
    first: u64,
    second: u64,
}

impl FileIdentity {
    pub(super) fn from_metadata(metadata: &Metadata) -> Result<Self, ApiError> {
        identity_from_metadata(metadata)
    }
}

#[derive(Clone)]
pub(super) struct IdentityBoundProcessRoot {
    directory: Arc<Dir>,
    process_path: Arc<PathBuf>,
    identity: FileIdentity,
}

impl IdentityBoundProcessRoot {
    pub(super) fn new(directory: Arc<Dir>, process_path: PathBuf) -> Result<Self, ApiError> {
        let identity = FileIdentity::from_metadata(
            &directory
                .dir_metadata()
                .map_err(|_| unavailable("workspace identity cannot be read"))?,
        )?;
        #[cfg(unix)]
        let _ = process_path;
        Ok(Self {
            directory,
            process_path: Arc::new(process_path),
            identity,
        })
    }

    pub(super) fn identity(&self) -> FileIdentity {
        self.identity
    }

    pub(super) async fn run(
        &self,
        command: RootCommand,
        limits: ProcessLimits,
    ) -> Result<ProcessOutput, ApiError> {
        let root = self.clone();
        tokio::task::spawn_blocking(move || {
            run_process(
                root,
                command,
                limits,
                None,
                Arc::new(AtomicBool::new(false)),
                ProcessCancellation::new(),
            )
        })
        .await
        .map_err(|_| unavailable("workspace process worker failed"))?
    }

    pub(super) async fn stream(
        &self,
        command: RootCommand,
        limits: ProcessLimits,
        sink: &mut dyn ProcessChunkSink,
    ) -> Result<ProcessStatus, ApiError> {
        self.stream_with_cancellation(command, limits, sink, None)
            .await
    }

    pub(super) async fn stream_with_cancellation(
        &self,
        command: RootCommand,
        limits: ProcessLimits,
        sink: &mut dyn ProcessChunkSink,
        cancellation: Option<ProcessCancellation>,
    ) -> Result<ProcessStatus, ApiError> {
        let external_cancellation = cancellation.unwrap_or_default();
        let mut drop_guard = CancelProcessOnDrop::new(external_cancellation.clone());
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let root = self.clone();
        let worker_cancelled = cancelled.clone();
        let mut worker = tokio::task::spawn_blocking(move || {
            run_process(
                root,
                command,
                limits,
                Some(sender),
                worker_cancelled,
                external_cancellation,
            )
        });

        loop {
            tokio::select! {
                result = &mut worker => {
                    let output = result
                        .map_err(|_| unavailable("workspace process worker failed"))??;
                    while let Ok(chunk) = receiver.try_recv() {
                        sink.push(chunk).await?;
                    }
                    drop_guard.disarm();
                    return Ok(output.status);
                }
                chunk = receiver.recv() => {
                    if let Some(chunk) = chunk {
                        if let Err(error) = sink.push(chunk).await {
                            cancelled.store(true, Ordering::SeqCst);
                            let _ = worker.await;
                            return Err(error);
                        }
                    }
                }
            }
        }
    }

    pub(super) fn reported_root_is_self(&self, output: &ProcessOutput) -> bool {
        let Ok(value) = std::str::from_utf8(output.stdout()) else {
            return false;
        };
        let reported = value.trim_end_matches(['\r', '\n']);
        if reported.is_empty() || reported.bytes().any(|byte| byte.is_ascii_control()) {
            return false;
        }
        identity_from_ambient_path(Path::new(reported)) == Some(self.identity)
    }

    pub(super) fn verify(&self) -> Result<(), ApiError> {
        let current = FileIdentity::from_metadata(
            &self
                .directory
                .dir_metadata()
                .map_err(|_| unavailable("workspace identity cannot be read"))?,
        )?;
        if current != self.identity {
            return Err(unavailable("workspace identity changed"));
        }
        Ok(())
    }

    pub(super) fn verify_execution_path(&self) -> Result<(), ApiError> {
        self.verify()?;
        if identity_from_ambient_path(&self.process_path) != Some(self.identity) {
            return Err(unavailable("workspace path identity changed"));
        }
        Ok(())
    }
}

impl super::WorkspaceCapability {
    /// Streams a bounded command until it exits or external cancellation is requested.
    pub async fn stream_at_root_with_cancellation(
        &self,
        command: RootCommand,
        limits: ProcessLimits,
        sink: &mut dyn ProcessChunkSink,
        cancellation: Option<ProcessCancellation>,
    ) -> Result<ProcessStatus, ApiError> {
        self.process_root
            .stream_with_cancellation(command, limits, sink, cancellation)
            .await
    }
}

fn run_process(
    root: IdentityBoundProcessRoot,
    request: RootCommand,
    limits: ProcessLimits,
    chunks: Option<mpsc::UnboundedSender<ProcessChunk>>,
    cancelled: Arc<AtomicBool>,
    external_cancellation: ProcessCancellation,
) -> Result<ProcessOutput, ApiError> {
    root.verify()?;
    if external_cancellation.is_cancelled() {
        return Ok(cancelled_output());
    }
    let mut command = Command::new(&request.program);
    command
        .args(&request.arguments)
        .env_clear()
        .envs(&request.environment)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_platform_environment(&mut command);
    set_process_root(&mut command, &root)?;

    let mut child = command
        .spawn()
        .map_err(|_| unavailable("workspace process cannot be started"))?;
    let process_tree = ProcessTree::attach(&mut child)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| unavailable("workspace process stdout is unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| unavailable("workspace process stderr is unavailable"))?;
    let used = Arc::new(AtomicUsize::new(0));
    let exceeded = Arc::new(AtomicBool::new(false));
    let stdout_reader = read_pipe(
        stdout,
        ProcessStream::Stdout,
        limits.max_output_bytes,
        used.clone(),
        exceeded.clone(),
        cancelled.clone(),
        chunks.clone(),
    );
    let stderr_reader = read_pipe(
        stderr,
        ProcessStream::Stderr,
        limits.max_output_bytes,
        used,
        exceeded.clone(),
        cancelled.clone(),
        chunks,
    );

    let deadline = Instant::now() + limits.timeout;
    let (exit, timed_out, externally_cancelled) = loop {
        if external_cancellation.is_cancelled() {
            process_tree.terminate(&mut child);
            let exit = child
                .wait()
                .map_err(|_| unavailable("workspace process cannot be reaped"))?;
            break (exit, false, true);
        }
        if let Some(exit) = child
            .try_wait()
            .map_err(|_| unavailable("workspace process status is unavailable"))?
        {
            process_tree.terminate(&mut child);
            break (exit, false, false);
        }
        if exceeded.load(Ordering::SeqCst) || cancelled.load(Ordering::SeqCst) {
            process_tree.terminate(&mut child);
            let exit = child
                .wait()
                .map_err(|_| unavailable("workspace process cannot be reaped"))?;
            break (exit, false, false);
        }
        if Instant::now() >= deadline {
            process_tree.terminate(&mut child);
            let exit = child
                .wait()
                .map_err(|_| unavailable("workspace process cannot be reaped"))?;
            break (exit, true, false);
        }
        std::thread::sleep(POLL_INTERVAL);
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| unavailable("workspace stdout reader failed"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| unavailable("workspace stderr reader failed"))??;
    root.verify()?;
    if exceeded.load(Ordering::SeqCst) {
        return Err(api_error(
            ApiErrorCode::PayloadTooLarge,
            "workspace process output exceeded its limit",
        ));
    }
    if cancelled.load(Ordering::SeqCst) {
        return Err(unavailable("workspace process was cancelled"));
    }
    Ok(ProcessOutput {
        status: ProcessStatus {
            code: exit.code(),
            timed_out,
            cancelled: externally_cancelled,
        },
        stdout,
        stderr,
    })
}

fn cancelled_output() -> ProcessOutput {
    ProcessOutput {
        status: ProcessStatus {
            code: None,
            timed_out: false,
            cancelled: true,
        },
        stdout: Vec::new(),
        stderr: Vec::new(),
    }
}

#[allow(clippy::too_many_arguments)]
fn read_pipe<R: Read + Send + 'static>(
    mut pipe: R,
    stream: ProcessStream,
    limit: usize,
    used: Arc<AtomicUsize>,
    exceeded: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    chunks: Option<mpsc::UnboundedSender<ProcessChunk>>,
) -> std::thread::JoinHandle<Result<Vec<u8>, ApiError>> {
    std::thread::spawn(move || {
        let mut collected = Vec::new();
        let mut buffer = [0_u8; READ_CHUNK_BYTES];
        loop {
            let read = pipe
                .read(&mut buffer)
                .map_err(|_| unavailable("workspace process output cannot be read"))?;
            if read == 0 {
                return Ok(collected);
            }
            let previous = used.fetch_add(read, Ordering::SeqCst);
            let accepted = limit.saturating_sub(previous).min(read);
            if accepted < read {
                exceeded.store(true, Ordering::SeqCst);
            }
            if accepted > 0 {
                let bytes = buffer[..accepted].to_vec();
                if let Some(sender) = &chunks {
                    if sender.send(ProcessChunk { stream, bytes }).is_err() {
                        cancelled.store(true, Ordering::SeqCst);
                    }
                } else {
                    collected.extend_from_slice(&bytes);
                }
            }
        }
    })
}

#[cfg(unix)]
fn set_process_root(
    command: &mut Command,
    root: &IdentityBoundProcessRoot,
) -> Result<(), ApiError> {
    use std::os::unix::process::CommandExt;

    let directory = root
        .directory
        .try_clone()
        .map_err(|_| unavailable("workspace process root cannot be cloned"))?
        .into_std_file();
    // SAFETY: the closure only invokes async-signal-safe process setup operations.
    unsafe {
        command.pre_exec(move || {
            rustix::process::setpgid(None, None).map_err(io::Error::from)?;
            rustix::process::fchdir(&directory).map_err(io::Error::from)
        });
    }
    Ok(())
}

#[cfg(windows)]
fn set_process_root(
    command: &mut Command,
    root: &IdentityBoundProcessRoot,
) -> Result<(), ApiError> {
    use std::os::windows::process::CommandExt;

    command.current_dir(root.process_path.as_ref());
    command.creation_flags(windows_sys::Win32::System::Threading::CREATE_SUSPENDED);
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn set_process_root(
    command: &mut Command,
    root: &IdentityBoundProcessRoot,
) -> Result<(), ApiError> {
    command.current_dir(root.process_path.as_ref());
    Ok(())
}

fn apply_platform_environment(command: &mut Command) {
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    #[cfg(windows)]
    if let Some(system_root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", system_root);
    }
}

#[cfg(unix)]
struct ProcessTree;

#[cfg(unix)]
impl ProcessTree {
    fn attach(_child: &mut Child) -> Result<Self, ApiError> {
        Ok(Self)
    }

    fn terminate(&self, child: &mut Child) {
        if let Some(pid) = rustix::process::Pid::from_raw(child.id() as i32) {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        }
        let _ = child.kill();
    }
}

#[cfg(windows)]
struct ProcessTree(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl ProcessTree {
    fn attach(child: &mut Child) -> Result<Self, ApiError> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };

        // SAFETY: Windows initializes the job object and the zeroed limit structure is valid.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                reap_failed_attach(child);
                return Err(unavailable("workspace process job cannot be created"));
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                std::ptr::addr_of!(limits).cast(),
                std::mem::size_of_val(&limits) as u32,
            ) == 0
                || AssignProcessToJobObject(job, child.as_raw_handle()) == 0
            {
                windows_sys::Win32::Foundation::CloseHandle(job);
                reap_failed_attach(child);
                return Err(unavailable("workspace process job cannot be configured"));
            }
            if !resume_suspended_process(child.id()) {
                windows_sys::Win32::System::JobObjects::TerminateJobObject(job, 1);
                windows_sys::Win32::Foundation::CloseHandle(job);
                reap_failed_attach(child);
                return Err(unavailable("workspace process cannot be resumed"));
            }
            Ok(Self(job))
        }
    }

    fn terminate(&self, child: &mut Child) {
        // SAFETY: the handle is a live job object owned by this value.
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0, 1);
        }
        let _ = child.kill();
    }
}

#[cfg(windows)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        // SAFETY: the handle is owned by this value and closed exactly once.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(not(any(unix, windows)))]
struct ProcessTree;

#[cfg(not(any(unix, windows)))]
impl ProcessTree {
    fn attach(_child: &mut Child) -> Result<Self, ApiError> {
        Ok(Self)
    }

    fn terminate(&self, child: &mut Child) {
        let _ = child.kill();
    }
}

#[cfg(windows)]
fn reap_failed_attach(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(windows)]
fn resume_suspended_process(process_id: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

    // SAFETY: snapshot and thread handles are checked before use and closed exactly once.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut found = false;
        let mut has_entry = Thread32First(snapshot, &mut entry) != 0;
        while has_entry {
            if entry.th32OwnerProcessID == process_id {
                let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                if !thread.is_null() {
                    found = ResumeThread(thread) != u32::MAX;
                    CloseHandle(thread);
                    if found {
                        break;
                    }
                }
            }
            has_entry = Thread32Next(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
        found
    }
}

#[cfg(unix)]
fn identity_from_metadata(metadata: &Metadata) -> Result<FileIdentity, ApiError> {
    Ok(FileIdentity {
        first: metadata.dev(),
        second: metadata.ino(),
    })
}

#[cfg(windows)]
fn identity_from_metadata(metadata: &Metadata) -> Result<FileIdentity, ApiError> {
    let first = metadata
        .volume_serial_number()
        .ok_or_else(|| unavailable("workspace volume identity is unavailable"))?;
    let second = metadata
        .file_index()
        .ok_or_else(|| unavailable("workspace file identity is unavailable"))?;
    Ok(FileIdentity {
        first: u64::from(first),
        second,
    })
}

#[cfg(not(any(unix, windows)))]
fn identity_from_metadata(_metadata: &Metadata) -> Result<FileIdentity, ApiError> {
    Err(unavailable(
        "workspace identity is unsupported on this platform",
    ))
}

fn identity_from_ambient_path(path: &Path) -> Option<FileIdentity> {
    let directory = Dir::open_ambient_dir(path, cap_std::ambient_authority()).ok()?;
    FileIdentity::from_metadata(&directory.dir_metadata().ok()?).ok()
}

fn api_error(code: ApiErrorCode, message: &'static str) -> ApiError {
    ApiError::new(code, message, "platform-workspace-process")
}

fn unavailable(message: &'static str) -> ApiError {
    api_error(ApiErrorCode::WorkspaceUnavailable, message)
}

#[cfg(test)]
#[path = "workspace_process_tests.rs"]
mod tests;
