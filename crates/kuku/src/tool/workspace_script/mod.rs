mod actions;
mod facts;
mod host;
mod outputs;
mod runtime;
#[cfg(test)]
mod tests;
mod vm;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::tool::ToolDefinition;

pub(crate) use actions::WorkspaceActions;
pub(crate) use facts::WorkspaceScriptOperation as ScriptOperation;
pub use facts::{
    WorkspaceScriptActionFacts, WorkspaceScriptActionStatus, WorkspaceScriptFacts,
    WorkspaceScriptOperation, WorkspaceScriptResultKind,
};
pub(crate) use host::{ScriptCall, ScriptExecution, ScriptFuture, ScriptHost};
pub(crate) use outputs::definitions as derived_output_definitions;
pub(crate) use runtime::execute;

const MAX_SCRIPT_ACTIONS: usize = 2048;
const MAX_SCRIPT_RESULT_BYTES: usize = 64 * 1024;

/// Bounds one disposable workspace JavaScript worker and its host operations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceScriptLimits {
    /// Maximum bytes in the QuickJS heap.
    pub heap_bytes: usize,
    /// Maximum bytes in the QuickJS stack.
    pub stack_bytes: usize,
    /// Maximum elapsed execution time, including host operations.
    pub timeout_ms: u64,
    /// Maximum UTF-8 bytes in one script function body.
    pub source_bytes: usize,
    /// Maximum serialized bytes in the returned model evidence.
    pub result_bytes: usize,
    /// Maximum serialized bytes in one host request or response.
    pub host_bytes: usize,
    /// Maximum serialized request bytes waiting for or using host capacity.
    pub queued_bytes: usize,
    /// Maximum bytes in a workspace file read by one host operation.
    pub file_bytes: usize,
    /// Maximum simultaneously executing host operations within one worker.
    pub active_operations: usize,
    /// Maximum host operations accepted but not yet completed within one worker.
    pub queued_operations: usize,
    /// Maximum host operations accepted during one worker's lifetime.
    pub actions: usize,
    /// Maximum derived outputs created by one worker.
    pub derived_outputs: usize,
    /// Maximum captured bytes across derived outputs created by one worker.
    pub derived_bytes: u64,
}

impl Default for WorkspaceScriptLimits {
    fn default() -> Self {
        Self {
            heap_bytes: 32 * 1024 * 1024,
            stack_bytes: 512 * 1024,
            timeout_ms: 30_000,
            source_bytes: 256 * 1024,
            result_bytes: 16 * 1024,
            host_bytes: 4 * 1024 * 1024,
            queued_bytes: 8 * 1024 * 1024,
            file_bytes: 4 * 1024 * 1024,
            active_operations: 4,
            queued_operations: 64,
            actions: 1024,
            derived_outputs: 8,
            derived_bytes: 8 * 1024 * 1024,
        }
    }
}

impl WorkspaceScriptLimits {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.heap_bytes < 1024 * 1024
            || self.stack_bytes < 64 * 1024
            || self.stack_bytes > 8 * 1024 * 1024
            || self.timeout_ms == 0
            || self.source_bytes == 0
            || self.result_bytes < 128
            || self.result_bytes > MAX_SCRIPT_RESULT_BYTES
            || self.host_bytes < 1024
            || self.host_bytes > tokio::sync::Semaphore::MAX_PERMITS.min(u32::MAX as usize)
            || self.queued_bytes < self.host_bytes
            || self.file_bytes == 0
            || self.active_operations == 0
            || self.queued_operations < self.active_operations
            || self.queued_operations > tokio::sync::Semaphore::MAX_PERMITS
            || self.actions == 0
            || self.actions > MAX_SCRIPT_ACTIONS
            || self.derived_outputs == 0
            || self.derived_bytes < 4096
        {
            return Err(
                "invalid workspace script heap, stack, time, byte or operation limits".into(),
            );
        }
        Ok(())
    }
}

pub(crate) fn definition() -> ToolDefinition {
    ToolDefinition {
        name: "workspace_script".into(),
        description: concat!(
            "Execute a JavaScript async function body with top-level await. Return a small JSON value. ",
            "Use workspace.<method>({arguments}) for host actions. workspace.methods lists the available ",
            "method definitions. Host methods use the direct tool names and arguments and return ",
            "{status, summary, truncated, result, model_content}. Check status and read structured data ",
            "from result, for example result.raw_text from read_file. Use Promise.all for independent ",
            "actions. The host authorizes each final action. Example: ",
            "let count = 0; for await (const {text} of workspace.output_lines({id: 'OUTPUT_ID', stream: 'stdout'})) ",
            "{ if (text.includes('error')) count++; } ",
            "const report = await workspace.write_file({path: 'summary.json', content: JSON.stringify({count})}); ",
            "return {count, status: report.status}; ",
            "Get an output id from run_command result.output_ref.id. For raw bytes, use ",
            "workspace.output_chunks({id}) and its segments. Use create_output, append_output and ",
            "finish_output to retain large derived results. When available, save durable names with ",
            "await workspace.save_value({name, value}) or await workspace.save_value({name, output_id}). ",
            "Set replace to true to change an existing binding. Use workspace.list_values({prefix, after, limit}) ",
            "to discover names and their references, and workspace.read_output({name}) to read a value. ",
            "JavaScript runs in QuickJS without Node.js APIs."
        ).into(),
        input_schema: json!({
            "type":"object",
            "properties":{
                "code":{"type":"string","minLength":1,"description":"JavaScript async function body with a JSON return value."}
            },
            "required":["code"],
            "additionalProperties":false
        }),
        read_only: false,
        max_result_chars: 16 * 1024,
        risk: "execute".into(),
    }
}
