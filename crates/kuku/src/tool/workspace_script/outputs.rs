use std::collections::HashMap;

use serde_json::{json, Value};
use tokio::sync::Mutex;

use super::{ScriptOperation, WorkspaceActions, WorkspaceScriptLimits};
use crate::session::runtime::OutputReservation;
use crate::tool::builtin::run_command::output::{OutputLimits, OutputWriter, MAX_FRAME_BYTES};
use crate::tool::ToolDefinition;
use crate::{Error, OutputStream, Result};

const COMMAND_SIGNATURE: &str = concat!(
    "tools.run_command({program,args?,cwd?,timeout?} or {command,shell?,cwd?,timeout?}) ",
    "returns {exit_code,duration_ms,capture_complete,output_ref,stdout,stderr}. ",
    "A process exit resolves with its code. Launch errors, denial, timeout and cancellation throw. "
);

const STREAM_SIGNATURE: &str = concat!(
    "CommandStream.text() returns Promise<string>. Its .json() returns Promise<JSONValue>. Both read up to 1 MiB per stream. ",
    ".lines() returns AsyncIterable<string> (up to 128 KiB per line). Both pipes work the same way. ",
    "Readers require complete, decodable output. Small complete output is included within command authorization. ",
);

pub(super) fn describe_script_command(definition: &mut ToolDefinition, can_read: bool) {
    if let Some(properties) = definition.input_schema["properties"].as_object_mut() {
        properties.remove("background");
    }
    let access = if can_read {
        "Larger reads require host authorization for read_output. For raw or partial bytes, use tools.output_chunks({id:run.output_ref.id}). "
    } else {
        "Larger captures still return a reference. This Agent has no read_output access. "
    };
    definition.description = [COMMAND_SIGNATURE, STREAM_SIGNATURE, access].concat();
}

struct Draft {
    writer: OutputWriter,
    reservation: OutputReservation,
}

pub(super) struct DerivedOutputs {
    state: Mutex<Drafts>,
    limits: WorkspaceScriptLimits,
}

#[derive(Default)]
struct Drafts {
    open: HashMap<String, Draft>,
    created: usize,
    bytes: u64,
}

impl DerivedOutputs {
    pub(super) fn new(limits: WorkspaceScriptLimits) -> Self {
        Self {
            state: Mutex::new(Drafts::default()),
            limits,
        }
    }

    pub(super) async fn apply(
        &self,
        op: ScriptOperation,
        actions: &WorkspaceActions,
        action_output_id: &str,
        args: &Value,
    ) -> Result<Value> {
        let mut drafts = self.state.lock().await;
        match op {
            ScriptOperation::CreateOutput => {
                if drafts.created >= self.limits.derived_outputs {
                    return Err(Error::InvalidArgument(
                        "script derived output count exceeded".into(),
                    ));
                }
                let mut reservation = actions.reserve_output(action_output_id)?;
                reservation.discard_on_drop();
                let dir = actions
                    .session
                    .events_path()
                    .parent()
                    .expect("session parent")
                    .join("outputs");
                let writer = OutputWriter::create(
                    &dir,
                    action_output_id,
                    OutputLimits {
                        max_output_bytes: reservation.max_bytes(),
                        max_preview_bytes: 0,
                    },
                )
                .await?;
                drafts.open.insert(
                    action_output_id.into(),
                    Draft {
                        writer,
                        reservation,
                    },
                );
                drafts.created += 1;
                Ok(json!({"id":action_output_id,"complete":false}))
            }
            ScriptOperation::AppendOutput => {
                let (id, text) = (string_arg(args, "id")?, string_arg(args, "text")?);
                let total = drafts.bytes.saturating_add(text.len() as u64);
                if total > self.limits.derived_bytes {
                    return Err(Error::InvalidArgument(
                        "script derived output byte limit exceeded".into(),
                    ));
                }
                let draft = drafts.open.get_mut(id).ok_or_else(missing)?;
                for chunk in text.as_bytes().chunks(MAX_FRAME_BYTES) {
                    if let Err(error) = draft.writer.write(OutputStream::Stdout, chunk).await {
                        drafts.open.remove(id);
                        return Err(error.into());
                    }
                }
                drafts.bytes = total;
                Ok(json!({"id":id, "submitted_bytes":text.len()}))
            }
            ScriptOperation::FinishOutput => {
                let id = string_arg(args, "id")?;
                let mut draft = drafts.open.remove(id).ok_or_else(missing)?;
                let metadata = draft.writer.finish(false).await?;
                draft.reservation.keep();
                Ok(
                    json!({"id":id, "output_ref":{"id":id, "offset":0, "bytes":metadata.stored_bytes},
                    "capture_complete":metadata.complete && !metadata.quota_truncated,
                    "quota_truncated":metadata.quota_truncated}),
                )
            }
            _ => Err(Error::InvalidArgument("unknown output operation".into())),
        }
    }
}

fn missing() -> Error {
    Error::InvalidArgument("output is not an open draft from this script".into())
}

fn string_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args[key]
        .as_str()
        .ok_or_else(|| Error::InvalidArgument(format!("{key} must be a string")))
}

pub(crate) fn definitions() -> Vec<ToolDefinition> {
    [
        ("create_output", "Open a new Session output. Returns its id.", json!({}), json!([])),
        ("append_output", "Append UTF-8 text to an open script output.", json!({"id":{"type":"string"},"text":{"type":"string"}}), json!(["id","text"])),
        ("finish_output", "Finish a derived output and return its immutable output reference and capture status.", json!({"id":{"type":"string"}}), json!(["id"])),
    ].into_iter().map(|(name, description, properties, required)| ToolDefinition {
        name:name.into(), description:description.into(),
        input_schema:json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),
        read_only:false, max_result_chars:1024,risk:"session".into(),
    }).collect()
}
