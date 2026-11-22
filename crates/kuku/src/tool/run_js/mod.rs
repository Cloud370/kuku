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
pub(crate) use runtime::execute;

const MAX_SCRIPT_ACTIONS: usize = 2048;
const MAX_SCRIPT_RESULT_BYTES: usize = 64 * 1024;
const SCRIPT_DESCRIPTION: &str = concat!(
    "Compose dependent tool calls in JavaScript with top-level await and return a small JSON value. ",
    "Call tools.<name>(args). tools.methods lists the available host methods and their input schemas. ",
    "Standard JavaScript built-ins are available. Node and browser APIs are unavailable. Slow scripts become Session Jobs."
);

/// Bounds one disposable workspace JavaScript worker and its host operations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceScriptLimits {
    /// Maximum bytes in the QuickJS heap.
    pub heap_bytes: usize,
    /// Maximum bytes in the QuickJS stack.
    pub stack_bytes: usize,
    /// Maximum elapsed JavaScript execution time, excluding worker admission and host waits.
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
        name: "run_js".into(),
        description: SCRIPT_DESCRIPTION.into(),
        input_schema: json!({
            "type":"object",
            "properties":{
                "code":{"type":"string","minLength":1,"description":"JavaScript async function body with a JSON return value."},
                "background":{"type":"boolean","default":false,"description":"Return a Job handle if the script is still running after one second."},
                "title":crate::tool::registry::action_title_schema()
            },
            "required":["code"],
            "additionalProperties":false
        }),
        read_only: false,
        max_result_chars: 16 * 1024,
        risk: "execute".into(),
    }
}

pub(crate) fn method_definitions(
    mut lookup: impl FnMut(ScriptOperation) -> Option<ToolDefinition>,
) -> Vec<ToolDefinition> {
    let derived = outputs::definitions();
    let mut methods: Vec<_> = ScriptOperation::ALL
        .into_iter()
        .filter(|op| *op != ScriptOperation::SearchOutput)
        .filter(|op| !derived.iter().any(|method| method.name == op.name()))
        .filter_map(&mut lookup)
        .collect();
    methods.extend(derived);
    let read = methods.iter().find(|method| method.name == "read_output");
    let can_read = read.is_some();
    if let Some(read) = read {
        methods.push(outputs::search_definition(read));
    }
    if let Some(command) = methods
        .iter_mut()
        .find(|method| method.name == "run_command")
    {
        outputs::describe_script_command(command, can_read);
    }
    methods
}

pub(crate) fn describe_selected_tools(tools: &mut [ToolDefinition]) {
    let Some(index) = tools.iter().position(|tool| tool.name == "run_js") else {
        return;
    };
    let methods =
        method_definitions(|op| tools.iter().find(|tool| tool.name == op.name()).cloned());
    let mut description = format!("{SCRIPT_DESCRIPTION}\n");
    if let Some(command) = methods.iter().find(|method| method.name == "run_command") {
        description.push_str(&command.description);
        description.push_str("Example: `const r = await tools.run_command({program:'git',args:['status','--porcelain']}); return {exit_code:r.exit_code, output_ref:r.output_ref, lines:r.exit_code === 0 && r.capture_complete ? (await r.stdout.text()).split('\\n').filter(Boolean).length : null};`.\n");
    }
    description.push_str("Other host methods return {status,result,truncated,error?}. Check status === 'ok' before reading result. ");
    for op in ScriptOperation::ALL {
        if methods.iter().any(|method| method.name == op.name()) {
            description.push_str(method_result(op));
        }
    }
    tools[index].description = description;
}

fn method_result(operation: ScriptOperation) -> &'static str {
    use ScriptOperation as Op;
    match operation {
        Op::FindFiles => "find_files({path?,pattern?,max_depth?}) lists immediate entries or finds files recursively with a pattern. Read result.entries and result.entry_count. ",
        Op::ReadFile => "read_file({path,offset?,limit?}) returns result.raw_text, content_hash, start_byte, end_byte and is_full_file_snapshot. ",
        Op::SearchText => "search_text({pattern,path?,include?,view?,offset?,limit?,context?}) returns result.matches, has_more and next_offset. ",
        Op::EditFile => "edit_file({path,old_text,new_text,replace_all?,expected_version?,brief?}) returns result.replacement_count and content_hash_after. ",
        Op::WriteFile => "write_file({path,content,expected_version?,brief?}) returns result.content_hash_after and bytes_written. ",
        Op::StatFile => "stat_file({path,version?}) returns result.exists, entry_kind and optional content_hash. ",
        Op::RemoveFile => "remove_file({path,expected_version?,brief?}) returns result.bytes_removed. ",
        Op::RunCommand => "",
        Op::ReadOutput => "read_output({id or name,offset?,limit?,search?}) returns result.segments, next_offset and has_more. output_chunks(args) yields pages. output_lines({id or name,stream?,offset?,limit?}) yields line records with text and completeness flags. ",
        Op::SearchOutput => "search_output({id or name,text,stream?,offset?,limit?}) finds exact UTF-8 bytes in each stream and returns result.matches with stream, offset and end_offset. Continue with result.next_offset while result.has_more is true, even if a page has no matches. ",
        Op::SaveValue => "save_value({name,output_id or value,replace?}) returns result.reference. ",
        Op::ListValues => "list_values({prefix?,after?,limit?}) returns result.values and next_after. ",
        Op::CreateOutput => "create_output({}) returns result.id. ",
        Op::AppendOutput => "append_output({id,text}) returns result.submitted_bytes. ",
        Op::FinishOutput => "finish_output({id}) returns result.output_ref and capture_complete. ",
    }
}
