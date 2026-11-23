use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use super::{ScriptExecution, ScriptOperation};
use crate::session::runtime::OutputReservation;
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
    pub(crate) io_limits: crate::WorkspaceIoLimits,
    pub(crate) max_command_seconds: Option<std::num::NonZeroU32>,
    pub(crate) max_command_memory_bytes: Option<std::num::NonZeroU64>,
}

impl WorkspaceActions {
    pub(super) fn reserve_output(&self) -> Result<OutputReservation> {
        self.session
            .reserve_output(&self.agent, self.original_turn, &self.tool_call_id)
    }

    pub(crate) async fn retain_script_return(&self, json: String) -> Result<OutputReference> {
        let mut reservation = self.reserve_output()?;
        let id = reservation.id().to_owned();
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
        execution: Arc<ScriptExecution>,
    ) -> Result<ToolResultEnvelope> {
        use ScriptOperation as Op;
        match op {
            Op::RunCommand => {
                let mut reservation = self.reserve_output()?;
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
                        reservation.id(),
                        OutputLimits {
                            max_output_bytes: reservation.max_bytes(),
                            max_preview_bytes: self.preview_bytes,
                        },
                        execution.limits.host_bytes,
                    )
                    .with_process_limits(self.max_command_seconds, self.max_command_memory_bytes),
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
                let value = execution.outputs.apply(op, self, &args).await?;
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
                let limits = self.io_limits;
                let local_id = local_id.to_owned();
                tokio::task::spawn_blocking(move || {
                    if execution.is_cancelled() {
                        return ToolResultEnvelope::cancelled(
                            "script stopped before file operation",
                        );
                    }
                    let cancelled = || execution.is_cancelled();
                    match op {
                        Op::FindFiles => builtin::find_files(&args, &workspace, limits, &cancelled),
                        Op::SearchText => {
                            builtin::search_text_for_executor(&args, &workspace, limits, &cancelled)
                        }
                        Op::StatFile => {
                            builtin::stat_file(&args, &workspace, limits.scan_bytes, &cancelled)
                        }
                        Op::ReadFile => {
                            let (result, read) = builtin::read_file_for_executor(
                                &args,
                                &workspace,
                                execution.limits.file_bytes,
                                limits.scan_bytes,
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
                            limits.text_bytes,
                            &cancelled,
                        ),
                        Op::WriteFile => builtin::write_file_for_executor(
                            &args,
                            &workspace,
                            &execution.observations(),
                            limits,
                            &cancelled,
                        ),
                        Op::RemoveFile => builtin::remove_file_for_executor(
                            &args,
                            &workspace,
                            &execution.observations(),
                            limits.scan_bytes,
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
