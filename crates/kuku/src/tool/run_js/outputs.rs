use std::collections::HashMap;
use std::path::Path;

use serde_json::{Value, json};
use tokio::sync::Mutex;

use super::{ScriptOperation, WorkspaceActions, WorkspaceScriptLimits};
use crate::session::runtime::OutputReservation;
use crate::tool::ToolDefinition;
use crate::tool::builtin::run_command::output::{MAX_FRAME_BYTES, OutputLimits, OutputWriter};
use crate::{Error, OutputStream, Result};

const COMMAND_SIGNATURE: &str = concat!(
    "tools.run_command({program,args?,cwd?}) ",
    "returns {exit_code,duration_ms,capture_complete,output_ref,stdout,stderr}. ",
    "It waits for the process inside the script. Set background:true on the outer run_js call to detach it. ",
    "Set title on the outer run_js call to label the script. ",
    "The host controls process deadlines. ",
    "A process exit resolves with its code. Launch errors, denial, timeout and cancellation throw. "
);

const STREAM_SIGNATURE: &str = concat!(
    "stdout and stderr are CommandStream readers. Use await r.stdout.text() for text, await r.stdout.json() for JSON, or for await to read r.stdout.lines(). ",
    "These readers require complete, decodable output. ",
);

const CONFIG_CLI_GUIDANCE: &str = include_str!("../../../prompts/tools/config-cli.md");

pub(super) fn describe_config_cli(
    command: &mut ToolDefinition,
    program: Option<&Path>,
    data_dir: &Path,
) {
    let mut paragraphs = CONFIG_CLI_GUIDANCE.lines().filter(|line| !line.is_empty());
    let managed = paragraphs.next().expect("managed CLI guidance");
    let fallback = paragraphs.next().expect("unmanaged CLI guidance");
    let common = paragraphs.next().expect("config command guidance");
    let invocation = program
        .filter(|path| path.is_absolute() && data_dir.is_absolute())
        .and_then(Path::to_str)
        .zip(data_dir.to_str())
        .map(|(program, data_dir)| {
            json!({
                "program": program,
                "args": ["--require-build", crate::BUILD_VERSION, "--data-dir", data_dir]
            })
        });
    let guidance = invocation.map_or_else(
        || fallback.to_owned(),
        |invocation| managed.replace("{{ invocation }}", &invocation.to_string()),
    );
    // The nested command adapter keeps this property, including in tools.schema().
    let property = &mut command.input_schema["properties"]["program"]["description"];
    let original = property.as_str().unwrap_or_default();
    *property = json!(format!("{original}\n{guidance} {common}"));
}

pub(super) fn describe_script_command(definition: &mut ToolDefinition, can_read: bool) {
    if let Some(properties) = definition.input_schema["properties"].as_object_mut() {
        properties.remove("background");
        properties.remove("title");
    }
    let access = if can_read {
        "For large or partial output, use tools.output_chunks({id:run.output_ref.id}). "
    } else {
        "Larger captures still return a reference. This Agent has no read_output access. "
    };
    let program = definition.input_schema["properties"]["program"]["description"]
        .as_str()
        .unwrap_or_default();
    definition.description = [COMMAND_SIGNATURE, STREAM_SIGNATURE, access, program, "\n"].concat();
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
                let reservation = actions.reserve_output()?;
                let id = reservation.id().to_owned();
                let dir = actions
                    .session
                    .events_path()
                    .parent()
                    .expect("session parent")
                    .join("outputs");
                let writer = OutputWriter::create(
                    &dir,
                    &id,
                    OutputLimits {
                        max_output_bytes: reservation.max_bytes(),
                        max_preview_bytes: 0,
                    },
                )
                .await?;
                drafts.open.insert(
                    id.clone(),
                    Draft {
                        writer,
                        reservation,
                    },
                );
                drafts.created += 1;
                Ok(json!({"id":id,"complete":false}))
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
