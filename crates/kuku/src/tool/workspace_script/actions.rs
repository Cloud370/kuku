use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use super::{ScriptExecution, ScriptOperation};
use crate::tool::builtin;
use crate::tool::builtin::run_command::{output::OutputLimits, run_command_with_output};
use crate::tool::ToolResultEnvelope;
use crate::{Result, Session};

pub(crate) struct WorkspaceActions {
    pub(crate) session: Session,
    pub(crate) agent: String,
    pub(crate) workspace: PathBuf,
    pub(crate) preview_bytes: usize,
}

impl WorkspaceActions {
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
                let mut reservation = self.session.reserve_output(&self.agent, output_id)?;
                reservation.discard_on_drop();
                let output_dir = self
                    .session
                    .events_path()
                    .parent()
                    .expect("session parent")
                    .join("outputs");
                let result = run_command_with_output(
                    &args,
                    &self.workspace,
                    None,
                    Some(execution.cancellation()),
                    &output_dir,
                    output_id,
                    OutputLimits {
                        max_output_bytes: reservation.max_bytes(),
                        max_preview_bytes: self.preview_bytes,
                    },
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
                let value = execution
                    .outputs
                    .apply(op, &self.session, &self.agent, output_id, &args)
                    .await?;
                Ok(ToolResultEnvelope::ok(
                    op.name(),
                    serde_json::to_string(&value)?,
                    value,
                ))
            }
            Op::ReadOutput => unreachable!("the Session bounded reader handles read_output"),
            _ => {
                let workspace = self.workspace.clone();
                let local_id = local_id.to_owned();
                tokio::task::spawn_blocking(move || {
                    if execution.is_cancelled() {
                        return ToolResultEnvelope::cancelled(
                            "script stopped before file operation",
                        );
                    }
                    match op {
                        Op::FindFiles => builtin::find_files(&args, &workspace),
                        Op::SearchText => builtin::search_text(&args, &workspace),
                        Op::StatFile => builtin::stat_file(&args, &workspace),
                        Op::ReadFile => {
                            let (result, read) = builtin::read_file_for_executor(
                                &args,
                                &workspace,
                                execution.limits.file_bytes,
                            );
                            if let Some(read) = read {
                                execution.stage_read(&local_id, read);
                            }
                            result
                        }
                        Op::EditFile => {
                            builtin::edit_file_for_executor(&args, &workspace, &execution.reads())
                        }
                        Op::WriteFile => {
                            builtin::write_file_for_executor(&args, &workspace, &execution.reads())
                        }
                        Op::RemoveFile => {
                            builtin::remove_file_for_executor(&args, &workspace, &execution.reads())
                        }
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
