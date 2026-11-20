use std::sync::Arc;

use serde_json::{json, Value};
use tokio::sync::Notify;

use crate::tool::{ToolDefinition, ToolResultEnvelope};
use crate::{Error, Result, Session};

fn job_schema() -> Value {
    json!({"type":"object","properties":{"job_id":{"type":"string","minLength":1}},
           "required":["job_id"],"additionalProperties":false})
}

pub(super) fn list_definition() -> ToolDefinition {
    let mut tool = super::definition("list_jobs",
        "List your Session Jobs, newest first. Returns id, tool, status, summary and output_id. For an older page pass the returned next_before.",
        json!({"type":"object","properties":{"before":{"type":"integer","minimum":1}},"additionalProperties":false}));
    tool.read_only = true;
    tool
}

pub(super) fn wait_definition() -> ToolDefinition {
    let mut tool = super::definition("wait_job",
        "Wait for one of your Session Jobs when your next action needs its result. Returns final status, result and output_id. Completions are also delivered automatically.",
        job_schema());
    tool.read_only = true;
    tool
}

pub(super) fn cancel_definition() -> ToolDefinition {
    super::definition("cancel_job",
        "Stop a specific Session Job by job_id. Returns its terminal status and any captured result.",
        job_schema())
}

pub(super) fn list(session: &Session, agent: &str, args: &Value) -> Result<ToolResultEnvelope> {
    let before = args
        .get("before")
        .map(|value| {
            value.as_u64().filter(|cursor| *cursor > 0).ok_or_else(|| {
                Error::InvalidArgument("before must be a positive Job cursor".into())
            })
        })
        .transpose()?;
    const PAGE_SIZE: usize = 25;
    let mut jobs = session.jobs_before(agent, before, PAGE_SIZE + 1)?;
    let has_more = jobs.len() > PAGE_SIZE;
    jobs.truncate(PAGE_SIZE);
    let next_before = jobs.last().filter(|_| has_more).map(|job| job.started_at);
    let value = json!({"jobs":jobs, "next_before":next_before});
    Ok(ToolResultEnvelope::ok(
        "Session Jobs",
        value.to_string(),
        value,
    ))
}

pub(in crate::query) async fn control(
    session: Session,
    owner: String,
    args: Value,
    cancel: Arc<Notify>,
    stop_job: bool,
) -> ToolResultEnvelope {
    let result = async {
        let id = super::required(&args, "job_id")?;
        let job = if stop_job {
            session.cancel_job(&owner, id).await?
        } else {
            tokio::select! {
                biased;
                _ = cancel.notified() => return Ok(ToolResultEnvelope::cancelled("Job wait cancelled")),
                result = session.wait_job(&owner, id) => result?,
            }
        };
        let mut value = serde_json::to_value(&job)?;
        if !stop_job {
            value["kind"] = json!("job_wait_result");
        }
        Ok(ToolResultEnvelope::ok(&job.summary, value.to_string(), value))
    }.await;
    result.unwrap_or_else(|err: Error| {
        ToolResultEnvelope::error("Job operation failed", err.to_string())
    })
}
