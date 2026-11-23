use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A named host action available to workspace JavaScript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceScriptOperation {
    /// List matching workspace paths.
    FindFiles,
    /// Read workspace file contents.
    ReadFile,
    /// Search workspace file contents.
    SearchText,
    /// Replace exact text in a workspace file.
    EditFile,
    /// Write a workspace file.
    WriteFile,
    /// Inspect a workspace path.
    StatFile,
    /// Remove a workspace file.
    RemoveFile,
    /// Start a native program or selected shell.
    RunCommand,
    /// Read a bounded portion of a retained output.
    ReadOutput,
    /// Find literal bytes in a retained output.
    SearchOutput,
    /// Bind a literal value or retained output to an Agent's exact name.
    SaveValue,
    /// List an Agent's named value metadata and immutable references.
    ListValues,
    /// Start a retained output derived by a script.
    CreateOutput,
    /// Append data to a derived output.
    AppendOutput,
    /// Finalize a derived output and obtain its reference.
    FinishOutput,
}

impl WorkspaceScriptOperation {
    /// The method name used by the script and the authorization boundary.
    pub fn name(self) -> &'static str {
        match self {
            Self::FindFiles => "find_files",
            Self::ReadFile => "read_file",
            Self::SearchText => "search_text",
            Self::EditFile => "edit_file",
            Self::WriteFile => "write_file",
            Self::StatFile => "stat_file",
            Self::RemoveFile => "remove_file",
            Self::RunCommand => "run_command",
            Self::ReadOutput => "read_output",
            Self::SearchOutput => "search_output",
            Self::SaveValue => "save_value",
            Self::ListValues => "list_values",
            Self::CreateOutput => "create_output",
            Self::AppendOutput => "append_output",
            Self::FinishOutput => "finish_output",
        }
    }

    pub(crate) fn permission_name(self) -> &'static str {
        match self {
            Self::SearchOutput => Self::ReadOutput.name(),
            _ => self.name(),
        }
    }

    pub(crate) const ALL: [Self; 15] = [
        Self::FindFiles,
        Self::ReadFile,
        Self::SearchText,
        Self::EditFile,
        Self::WriteFile,
        Self::StatFile,
        Self::RemoveFile,
        Self::RunCommand,
        Self::ReadOutput,
        Self::SearchOutput,
        Self::SaveValue,
        Self::ListValues,
        Self::CreateOutput,
        Self::AppendOutput,
        Self::FinishOutput,
    ];
}

/// A host operation's observed outcome within a workspace script.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceScriptActionStatus {
    /// The host operation completed and supplied a result.
    Ok,
    /// The host operation returned an error.
    Error,
    /// The host refused the action before execution.
    Blocked,
    /// The host acknowledged cancellation.
    Cancelled,
    /// The host did not confirm a terminal outcome before cleanup ended.
    Unknown,
}

/// Facts about one host action, in the order the script requested it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceScriptActionFacts {
    /// Action identifier local to this script invocation.
    pub id: String,
    /// The named host operation.
    pub tool: WorkspaceScriptOperation,
    /// The outcome observed by the host.
    pub status: WorkspaceScriptActionStatus,
    /// Short diagnostic or summary from the host.
    pub summary: String,
    /// Whether the underlying operation reported truncation.
    pub truncated: bool,
    /// Structured host facts, with large payloads omitted.
    pub result: Value,
}

/// Observed workspace script results and its bounded JSON return value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceScriptFacts {
    /// Stable structured result discriminator.
    pub kind: WorkspaceScriptResultKind,
    /// The script's JSON return value, or null if it supplied no value.
    pub value: Value,
    /// Complete JSON return saved as retained output when it exceeded the inline limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_ref: Option<crate::OutputReference>,
    /// Host actions that reached the host dispatch boundary.
    pub actions: Vec<WorkspaceScriptActionFacts>,
    /// Whether the JavaScript execution budget was exhausted.
    pub timed_out: bool,
    /// Whether the host could not confirm completion of resource cleanup.
    pub cleanup_incomplete: bool,
    /// Accepted operations that stopped before reaching the host dispatch boundary.
    pub not_dispatched: usize,
}

impl WorkspaceScriptFacts {
    /// Decode a structured workspace script result produced by the SDK.
    pub fn from_structured(value: &Value) -> serde_json::Result<Self> {
        serde_json::from_value(value.clone())
    }
}

/// Structured result discriminator for an embedded workspace script.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceScriptResultKind {
    /// A workspace script worker's outcome and observed actions.
    WorkspaceScriptResult,
}
