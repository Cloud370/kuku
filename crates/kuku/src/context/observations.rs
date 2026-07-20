//! Typed workspace observations and request-time retention values.

use crate::event::{
    ObservationFact, ObservationKind, ObservationRetention, ObservedRange, RequestScope,
    WorkspaceRelativePath,
};

const MAX_SUMMARY_CHARS: usize = 4_096;

/// Failure to convert a typed Tool result into a durable observation fact.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ObservationBuildError {
    /// A required Tool identity or summary is invalid.
    #[error("observation {0} is invalid")]
    Invalid(String),
    /// A line range overflows the durable integer boundary.
    #[error("observation line range overflows")]
    RangeOverflow,
}

/// Typed result data supplied by a built-in Tool adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolObservationData {
    /// A file content read with an optional content hash and line range.
    FileRead {
        /// Workspace-relative path returned by the host.
        path: String,
        /// Hash of the bytes observed by the Tool.
        observed_hash: Option<String>,
        /// One-based first observed line.
        start_line: u64,
        /// Number of observed lines.
        line_count: u64,
    },
    /// A file or directory listing.
    FileList {
        /// Workspace-relative listing root, or `None` for workspace root.
        path: Option<String>,
    },
    /// A text or path search.
    Search {
        /// Exact search query.
        query: String,
        /// Workspace-relative search root when scoped.
        path: Option<String>,
    },
    /// A command result, which is not a file tree observation.
    Command {
        /// Exact command invocation.
        command: String,
        /// Process exit code when available.
        exit_code: Option<i32>,
    },
    /// A named Tool result with optional path/hash metadata.
    Tool {
        /// Registered Tool name.
        name: String,
        /// Workspace-relative path when the Tool returned one.
        path: Option<String>,
        /// Hash of the observed value when available.
        observed_hash: Option<String>,
    },
}

/// Typed Tool result metadata needed for observation reduction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolObservation {
    /// Safe compact result summary.
    pub summary: String,
    /// Whether model-visible content was truncated.
    pub truncated: bool,
    /// Whether model-visible content was replaced by a summary.
    pub summarized: bool,
    /// Structured Tool result data.
    pub data: ToolObservationData,
}

/// Derived current state of one observed workspace value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationState {
    /// The path exists and matches the observed hash, or has no hash to compare.
    Present,
    /// The path exists but its current hash differs from the observed hash.
    ChangedSinceObservation,
    /// The path no longer exists.
    NoLongerPresent,
    /// The path exists but the current capability cannot inspect it.
    Inaccessible,
    /// The observation has no path and therefore has no file drift state.
    NotApplicable,
}

/// Compares an immutable observation against a current hash lookup.
pub struct ObservationTracker<'a> {
    fact: &'a ObservationFact,
}

impl<'a> ObservationTracker<'a> {
    /// Creates a derived tracker without changing the durable fact.
    pub fn new(fact: &'a ObservationFact) -> Self {
        Self { fact }
    }

    /// Compares the recorded hash against the current optional file hash.
    pub fn compare(&self, current_hash: Option<&str>) -> ObservationState {
        if self.fact.relative_path.is_none() {
            return ObservationState::NotApplicable;
        }
        let Some(current_hash) = current_hash else {
            return ObservationState::NoLongerPresent;
        };
        if self
            .fact
            .observed_hash
            .as_deref()
            .is_some_and(|observed| observed != current_hash)
        {
            ObservationState::ChangedSinceObservation
        } else {
            ObservationState::Present
        }
    }
}

/// Builds durable observations from typed built-in Tool results.
pub struct ObservationBuilder;

impl ObservationBuilder {
    /// Converts one Tool result into an immutable observation fact.
    pub fn from_tool(
        scope: RequestScope,
        tool_call_id: impl Into<String>,
        tool: ToolObservation,
    ) -> Result<ObservationFact, ObservationBuildError> {
        let tool_call_id = tool_call_id.into();
        validate_text("tool call ID", &tool_call_id, 256)?;
        validate_text("summary", &tool.summary, MAX_SUMMARY_CHARS)?;
        let retention = if tool.truncated {
            ObservationRetention::Truncated
        } else if tool.summarized {
            ObservationRetention::Summarized
        } else {
            ObservationRetention::Retained
        };
        let (kind, relative_path, observed_hash, range) = match tool.data {
            ToolObservationData::FileRead {
                path,
                observed_hash,
                start_line,
                line_count,
            } => (
                ObservationKind::FileRead,
                Some(parse_path(&path)?),
                observed_hash,
                line_range(start_line, line_count)?,
            ),
            ToolObservationData::FileList { path } => (
                ObservationKind::FileList,
                optional_path(path.as_deref())?,
                None,
                None,
            ),
            ToolObservationData::Search { query, path } => {
                validate_text("search query", &query, MAX_SUMMARY_CHARS)?;
                (
                    ObservationKind::Search { query },
                    optional_path(path.as_deref())?,
                    None,
                    None,
                )
            }
            ToolObservationData::Command { command, exit_code } => {
                validate_text("command", &command, MAX_SUMMARY_CHARS)?;
                (
                    ObservationKind::Command { command, exit_code },
                    None,
                    None,
                    None,
                )
            }
            ToolObservationData::Tool {
                name,
                path,
                observed_hash,
            } => {
                validate_text("Tool name", &name, 256)?;
                (
                    ObservationKind::Tool { name },
                    optional_path(path.as_deref())?,
                    observed_hash,
                    None,
                )
            }
        };
        Ok(ObservationFact {
            scope,
            tool_call_id,
            kind,
            relative_path,
            observed_hash,
            range,
            retention,
            summary: tool.summary,
        })
    }
}

fn line_range(
    start_line: u64,
    line_count: u64,
) -> Result<Option<ObservedRange>, ObservationBuildError> {
    if line_count == 0 {
        return Ok(None);
    }
    if start_line == 0 {
        return Err(ObservationBuildError::Invalid(
            "line range must be one-based".to_owned(),
        ));
    }
    let end_line = start_line
        .checked_add(line_count - 1)
        .ok_or(ObservationBuildError::RangeOverflow)?;
    Ok(Some(ObservedRange {
        start_line,
        end_line,
    }))
}

fn optional_path(
    path: Option<&str>,
) -> Result<Option<WorkspaceRelativePath>, ObservationBuildError> {
    path.map(parse_path).transpose()
}

fn parse_path(path: &str) -> Result<WorkspaceRelativePath, ObservationBuildError> {
    WorkspaceRelativePath::parse(path)
        .map_err(|_| ObservationBuildError::Invalid("workspace-relative path".to_owned()))
}

fn validate_text(label: &str, value: &str, max_chars: usize) -> Result<(), ObservationBuildError> {
    if value.is_empty() || value.chars().count() > max_chars {
        return Err(ObservationBuildError::Invalid(label.to_owned()));
    }
    Ok(())
}
