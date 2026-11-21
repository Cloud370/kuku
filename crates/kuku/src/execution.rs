//! Typed facts observed while executing workspace actions.

use serde::{Deserialize, Serialize};

mod file;
mod receipt;

pub use receipt::{WorkspaceActionDetails, WorkspaceActionReceipt, WorkspaceActionState};

pub use file::{FileCommitState, FileMutationFacts, FileMutationKind};

/// An interpreter for platform-specific command syntax.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandShell {
    /// The POSIX shell selected by the host's executable search path.
    Sh,
    /// Bash without login or interactive startup flags.
    Bash,
    /// Zsh without login or interactive startup flags.
    Zsh,
    /// Windows command processor with automatic startup commands disabled.
    Cmd,
    /// Windows PowerShell without user profiles or interactive prompts.
    Powershell,
    /// PowerShell Core without user profiles or interactive prompts.
    Pwsh,
}

/// The requested native program or explicit interpreter invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum CommandInvocation {
    /// Launch a program with literal arguments and no shell expansion.
    Program {
        /// Executable name or path supplied by the caller.
        program: String,
        /// Literal argument values, including empty strings.
        args: Vec<String>,
    },
    /// Evaluate the original command text with one selected interpreter.
    Shell {
        /// Interpreter selected without fallback to another interpreter.
        shell: CommandShell,
        /// Original command text passed to the interpreter.
        command: String,
    },
}

/// The observed termination of a requested program invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandTermination {
    /// The program terminated and its available exit status was observed.
    Exited,
    /// The execution timeout requested process-tree cleanup.
    TimedOut,
    /// Cancellation prevented dispatch or requested process-tree cleanup.
    Cancelled,
    /// The executable could not be found.
    ProgramMissing,
    /// The executable was found or selected but could not be started.
    StartFailed,
}

/// A retained Session output and its payload extent in capture order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputReference {
    /// Stable output identifier used by the shared bounded reader.
    pub id: String,
    /// Starting payload byte offset across both streams.
    pub offset: u64,
    /// Number of retained payload bytes from the starting offset.
    pub bytes: u64,
}

/// The command pipe that supplied retained bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputStream {
    Stdout,
    Stderr,
}

/// A merged-coordinate span. Read only the indicated stream within the span.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandOutputRange {
    pub stream: OutputStream,
    pub offset: u64,
    pub end_offset: u64,
}

/// The retained evidence used by a bounded command output view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommandOutputSummary {
    Rule {
        rule: String,
        ranges: Vec<CommandOutputRange>,
    },
}

/// Recognized format of a test program's own result report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestReportFormat {
    /// Rust's built-in test harness summary.
    Rust,
    /// Pytest's final result summary.
    Pytest,
}

/// Counts explicitly reported by a test program, independent of process success.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportedTests {
    /// Recognized report syntax that supplied these counts.
    pub format: TestReportFormat,
    /// Number of recognized summary records aggregated into this report.
    pub suites: u64,
    /// Tests reported as passed.
    pub passed: u64,
    /// Tests reported as failed.
    pub failed: u64,
    /// Test execution or collection errors reported separately from failures.
    #[serde(default)]
    pub errors: u64,
    /// Tests reported as skipped or ignored.
    pub skipped: u64,
    /// Tests reported as measured benchmarks.
    pub measured: u64,
    /// Tests reported as filtered or deselected.
    pub filtered_out: u64,
    /// Tests reported as expected failures.
    #[serde(default)]
    pub expected_failures: u64,
    /// Tests reported as unexpected passes.
    #[serde(default)]
    pub unexpected_passes: u64,
    /// Warnings reported by the test runner.
    #[serde(default)]
    pub warnings: u64,
    /// First bounded failure identifier observed for this report format.
    pub first_failure: Option<String>,
    /// Bounded original summary lines that supplied the counts.
    pub evidence: Vec<String>,
    /// Additional recognized report evidence exceeded the retained sample limit.
    pub evidence_truncated: bool,
}

/// Host-observed command facts retained separately from bounded model evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandExecutionFacts {
    /// Stable structured result discriminator.
    pub kind: CommandResultKind,
    /// Exact requested invocation without a reconstructed shell string.
    pub invocation: CommandInvocation,
    /// Canonical working directory used for this invocation.
    pub cwd: String,
    /// Standard input policy for this execution.
    pub stdin: CommandStdin,
    /// Observed termination category.
    pub termination: CommandTermination,
    /// Whether the execution host successfully started the process.
    #[serde(default)]
    pub started: bool,
    /// Exit code observed from the launched process, when available.
    pub exit_code: Option<i32>,
    /// Signal that terminated the launched process, where reported by the OS.
    #[serde(default)]
    pub signal: Option<i32>,
    /// Launch failure detail when the program did not start.
    pub start_error: Option<String>,
    /// Wall-clock execution and output-finalization time in milliseconds.
    pub duration_ms: u64,
    /// Number of observed stdout lines, including a final partial line.
    pub stdout_lines: u64,
    /// Number of observed stderr lines, including a final partial line.
    pub stderr_lines: u64,
    /// Number of observed stdout bytes, including bytes drained after the quota.
    pub stdout_bytes: u64,
    /// Number of observed stderr bytes, including bytes drained after the quota.
    pub stderr_bytes: u64,
    /// Whether the timeout requested cleanup.
    pub timed_out: bool,
    /// Whether cancellation prevented dispatch or requested cleanup.
    pub cancelled: bool,
    /// Stable output identifier for the shared bounded reader.
    pub output_id: String,
    /// Retained raw output extent acknowledged after persistence.
    pub output_ref: OutputReference,
    /// Whether both streams were captured fully within the retention quota.
    pub capture_complete: bool,
    /// Whether the retention quota caused observed bytes to be discarded.
    pub quota_truncated: bool,
    /// Whether process or pipe cleanup could not be confirmed complete.
    pub cleanup_incomplete: bool,
    /// Whether the initial model evidence selected recognized report lines.
    pub output_filtered: bool,
    /// Rule and source ranges used for a specialized view, when selected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_summary: Option<CommandOutputSummary>,
    /// Whether the initial model evidence omitted output due to its byte bound.
    pub preview_truncated: bool,
    /// Bounded decoded excerpt of the observed stdout and stderr.
    #[serde(default)]
    pub output_preview: String,
    /// Whether additional retained output lies outside the decoded excerpt.
    #[serde(default)]
    pub output_preview_truncated: bool,
    /// Whether decoding the displayed preview replaced raw bytes.
    pub lossy_utf8: bool,
    /// Number of transient output updates dropped without affecting capture.
    pub dropped_events: u64,
    /// Bounded selected build or test lines, without added line numbering.
    pub summary_lines: Vec<String>,
    /// Recognized reports as stated by the program, without inferring test success.
    pub reported_tests: Vec<ReportedTests>,
    /// Whether arbitrary program side effects were independently inventoried.
    pub side_effects_observed: bool,
}

impl CommandExecutionFacts {
    /// Decode a structured command result produced by the SDK.
    pub fn from_structured(value: &serde_json::Value) -> serde_json::Result<Self> {
        serde_json::from_value(value.clone())
    }
}

/// Structured result discriminator for native command execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandResultKind {
    /// Native process execution through the common capture path.
    CommandResult,
}

/// Standard input supplied to a command by the execution host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandStdin {
    /// The program receives end-of-file immediately.
    Null,
}
