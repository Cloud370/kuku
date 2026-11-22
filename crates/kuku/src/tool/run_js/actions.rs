use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use super::{ScriptExecution, ScriptOperation};
use crate::session::runtime::{command_output_id, OutputReservation};
use crate::tool::builtin;
use crate::tool::builtin::run_command::output::{OutputLimits, OutputWriter, MAX_FRAME_BYTES};
use crate::tool::builtin::run_command::{run_command_to, CommandCapture};
use crate::tool::ToolResultEnvelope;
use crate::{Error, OutputReference, OutputStream, Result, Session};

pub(crate) struct WorkspaceActions {
    pub(crate) session: Session,
    pub(crate) agent: String,
    pub(crate) original_turn: u64,
    pub(crate) tool_call_id: String,
    pub(crate) workspace: PathBuf,
    pub(crate) preview_bytes: usize,
    pub(crate) max_command_seconds: Option<std::num::NonZeroU32>,
}

impl WorkspaceActions {
    pub(super) fn reserve_output(&self, id: &str) -> Result<OutputReservation> {
        self.session
            .reserve_output(&self.agent, self.original_turn, &self.tool_call_id, id)
    }

    pub(crate) async fn retain_script_return(&self, json: String) -> Result<OutputReference> {
        let id = command_output_id(
            &self.agent,
            self.original_turn,
            &format!("{}:return", self.tool_call_id),
        );
        let mut reservation = self.reserve_output(&id)?;
        reservation.discard_on_drop();
        if json.len() as u64 > reservation.max_bytes() {
            return Err(Error::InvalidArgument(
                "the complete JSON exceeds available retained output capacity".into(),
            ));
        }
        let dir = self
            .session
            .events_path()
            .parent()
            .expect("session parent")
            .join("outputs");
        let mut writer = OutputWriter::create(
            &dir,
            &id,
            OutputLimits {
                max_output_bytes: reservation.max_bytes(),
                max_preview_bytes: 0,
            },
        )
        .await?;
        for frame in json.as_bytes().chunks(MAX_FRAME_BYTES) {
            writer.write(OutputStream::Stdout, frame).await?;
        }
        let metadata = writer.finish(false).await?;
        if !metadata.complete || metadata.stored_bytes != json.len() as u64 {
            return Err(Error::InvalidArgument(
                "the complete JSON could not be retained".into(),
            ));
        }
        reservation.keep();
        Ok(OutputReference {
            id,
            offset: 0,
            bytes: metadata.stored_bytes,
        })
    }

    pub(crate) async fn execute(
        &self,
        op: ScriptOperation,
        local_id: &str,
        args: Value,
        output_id: &str,
        execution: Arc<ScriptExecution>,
    ) -> Result<ToolResultEnvelope> {
        use ScriptOperation as Op;
        match op {
            Op::RunCommand => {
                let mut reservation = self.reserve_output(output_id)?;
                reservation.discard_on_drop();
                let output_dir = self
                    .session
                    .events_path()
                    .parent()
                    .expect("session parent")
                    .join("outputs");
                let result = run_command_to(
                    &args,
                    &self.workspace,
                    None,
                    Some(execution.cancellation()),
                    CommandCapture::script(
                        &output_dir,
                        output_id,
                        OutputLimits {
                            max_output_bytes: reservation.max_bytes(),
                            max_preview_bytes: self.preview_bytes,
                        },
                        execution.limits.host_bytes,
                    )
                    .with_time_limit(self.max_command_seconds),
                )
                .await;
                if result
                    .structured
                    .as_ref()
                    .is_some_and(|value| value["output_ref"].is_object())
                {
                    reservation.keep();
                }
                Ok(result)
            }
            Op::CreateOutput | Op::AppendOutput | Op::FinishOutput => {
                let value = execution.outputs.apply(op, self, output_id, &args).await?;
                Ok(ToolResultEnvelope::ok(
                    op.name(),
                    serde_json::to_string(&value)?,
                    value,
                ))
            }
            Op::ReadOutput | Op::SearchOutput | Op::ListValues | Op::SaveValue => {
                unreachable!("the Session handles output reads and named values")
            }
            _ => {
                let workspace = self.workspace.clone();
                let local_id = local_id.to_owned();
                tokio::task::spawn_blocking(move || {
                    if execution.is_cancelled() {
                        return ToolResultEnvelope::cancelled(
                            "script stopped before file operation",
                        );
                    }
                    let cancelled = || execution.is_cancelled();
                    match op {
                        Op::FindFiles => builtin::find_files(&args, &workspace, &cancelled),
                        Op::SearchText => {
                            builtin::search_text_for_executor(&args, &workspace, &cancelled)
                        }
                        Op::StatFile => builtin::stat_file(&args, &workspace, &cancelled),
                        Op::ReadFile => {
                            let (result, read) = builtin::read_file_for_executor(
                                &args,
                                &workspace,
                                execution.limits.file_bytes,
                                &cancelled,
                            );
                            if let Some(read) = read {
                                execution.stage_observation(&local_id, read);
                            }
                            result
                        }
                        Op::EditFile => builtin::edit_file_for_executor(
                            &args,
                            &workspace,
                            &execution.observations(),
                            &cancelled,
                        ),
                        Op::WriteFile => builtin::write_file_for_executor(
                            &args,
                            &workspace,
                            &execution.observations(),
                            &cancelled,
                        ),
                        Op::RemoveFile => builtin::remove_file_for_executor(
                            &args,
                            &workspace,
                            &execution.observations(),
                            &cancelled,
                        ),
                        _ => unreachable!("only file operations reach the blocking executor"),
                    }
                })
                .await
                .map_err(|err| {
                    crate::Error::InvalidEventStream(format!("workspace file worker failed: {err}"))
                })
            }
        }
    }
}
