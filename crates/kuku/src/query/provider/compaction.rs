use std::time::Instant;

use serde_json::json;
use sha2::{Digest, Sha256};
use tokio_stream::StreamExt;

use crate::context::{CanonicalMessage, MessageBlock};
use crate::event::{EventPayload, ModelStopReason};
use crate::provider::chunk::ProviderChunk;
use crate::provider::cost::RequestPricing;
use crate::provider::types::{
    PreparedProviderRequest, ProviderCorrelation, ProviderReport, ProviderUsage, ResolvedProvider,
};
use crate::provider::ProviderChunkStream;
use crate::query::frozen::ContextCommit;
use crate::query::helpers::now_timestamp;
use crate::query::types::PendingRun;
use crate::Result;

use super::dispatch::prepare_request;

const SUMMARY_OUTPUT_LIMIT: u32 = 4096;
const SUMMARY_BYTES: usize = 16 * 1024;
const CHECKPOINT_HEADROOM_BYTES: usize = 2048;

pub(super) enum Outcome {
    Keep,
    Failed,
    Replace(Box<PreparedProviderRequest>),
    Cancelled,
    NewInput,
}

#[derive(Clone, Copy)]
pub(super) enum Trigger<'a> {
    Pressure,
    ProviderOverflow { rejected_request: &'a str },
    Manual { closed_turn: bool },
}

pub(super) fn needed(pending: &PendingRun, context: &ContextCommit, request_bytes: usize) -> bool {
    let limit = pending.query.runtime_limits.max_request_bytes;
    pending.config.handoff().enabled
        && (context.token_pressure || request_bytes >= limit - limit / 8)
}

pub(super) async fn compact(
    pending: &mut PendingRun,
    context: &mut ContextCommit,
    resolved: &ResolvedProvider,
    correlation: &ProviderCorrelation,
    before_bytes: usize,
    trigger: Trigger<'_>,
) -> Result<Outcome> {
    let manual = matches!(trigger, Trigger::Manual { .. });
    let closed_turn = matches!(trigger, Trigger::Manual { closed_turn: true });
    let rejected_request = match trigger {
        Trigger::ProviderOverflow { rejected_request } => Some(rejected_request),
        _ => None,
    };
    let limit = pending.query.runtime_limits.max_request_bytes;
    let recent_bytes = if rejected_request.is_some() {
        limit.min(before_bytes) / 4
    } else {
        limit / 4
    };
    let Some(cut) = context.checkpoint_cut(pending.turn, recent_bytes, closed_turn)? else {
        return Ok(Outcome::Keep);
    };
    let must_fit = before_bytes > limit || rejected_request.is_some();
    let minimum_savings = if manual || must_fit {
        before_bytes.saturating_sub(limit).max(1)
    } else {
        (before_bytes / 10).max(2048)
    };
    if !manual
        && !must_fit
        && context.new_checkpoint_material_bytes(cut, pending.turn)? < minimum_savings
    {
        return Ok(Outcome::Keep);
    }
    let task = pending
        .request_base
        .as_ref()
        .expect("prepared task request");
    let Some(instruction) = task.checkpoint_instruction().map(str::to_owned) else {
        return Ok(Outcome::Keep);
    };
    let mut candidate = context.clone();
    let mut next = task.clone();
    let empty_checkpoint = task.render_checkpoint("")?;
    candidate.apply_checkpoint(&mut next, cut, pending.turn, empty_checkpoint, closed_turn);
    let Ok(skeleton) = prepare_request(pending, resolved, correlation, &next) else {
        return Ok(Outcome::Keep);
    };
    let reserve = if manual || must_fit {
        0
    } else {
        CHECKPOINT_HEADROOM_BYTES
    };
    if before_bytes.saturating_sub(skeleton.body.as_bytes().len()) < minimum_savings + reserve {
        return Ok(Outcome::Keep);
    }
    let mut source = context.checkpoint_source(cut, pending.turn);
    let summary_limit = if rejected_request.is_some() {
        limit.min(before_bytes.saturating_sub((before_bytes / 32).max(1)))
    } else {
        limit
    };
    let warm = task.checkpoint_request(
        source.clone(),
        instruction.clone(),
        true,
        SUMMARY_OUTPUT_LIMIT,
    );
    let prepared = match prepare_request(pending, resolved, correlation, &warm) {
        Ok(prepared) if prepared.body.as_bytes().len() <= summary_limit => prepared,
        _ => {
            for message in &mut source {
                message.blocks.retain(|block| {
                    !matches!(
                        block,
                        MessageBlock::Thinking(_) | MessageBlock::ProviderReplay { .. }
                    )
                });
            }
            let text = format!(
                "<checkpoint_source>\n{}\n</checkpoint_source>",
                serde_json::to_string(&source)?
            );
            let isolated = task.checkpoint_request(
                vec![CanonicalMessage::user_text(text)],
                instruction,
                false,
                SUMMARY_OUTPUT_LIMIT,
            );
            match prepare_request(pending, resolved, correlation, &isolated) {
                Ok(prepared) if prepared.body.as_bytes().len() <= summary_limit => prepared,
                _ => return Ok(Outcome::Keep),
            }
        }
    };

    let execution = pending
        .query
        .runtime_execution
        .expect("compaction belongs to an execution");
    let session = super::runtime_session(pending, execution)?;
    let agent = pending.conversation.as_str().to_owned();
    let request_id = if manual {
        format!("compact_{execution}")
    } else {
        format!(
            "compact_{}_{}",
            pending.turn,
            context.checkpoint_source_head()
        )
    };
    super::require_runtime_execution(pending)?;
    if !crate::query::budget::try_reserve_compaction(&pending.query, manual) {
        return Ok(Outcome::Keep);
    }
    let started = Instant::now();
    session.commit_model_facts(&agent, execution, [EventPayload::Unknown(json!({
        "kind":"runtime.compaction_started", "ts":now_timestamp()?, "agent":agent,
        "turn":pending.turn, "request_id":request_id, "before_bytes":before_bytes,
        "request_bytes":prepared.body.as_bytes().len(), "source_head":context.checkpoint_source_head(),
        "connection":prepared.connection_name, "model":prepared.model, "manual":manual,
        "rejected_request_id":rejected_request,
        "request_hash":format!("{:x}", Sha256::digest(prepared.body.as_bytes())),
    }))])?;
    let mut summary = Summary::default();
    let cancellation = pending.cancel_token.clone();
    let completed = tokio::select! {
        biased;
        _ = cancellation.notified() => false,
        _ = async {
            let stream = crate::query::adapters::model_stream(pending, &request_id, resolved, &prepared, None).await;
            match stream {
                Ok(stream) => summary.read(stream).await,
                Err(error) => summary.provider_failure(error.kind),
            }
        } => true,
    };
    let waiting_for_input = completed && session.has_step_input(&agent)?;
    pending.model_request_count += 1;
    let elapsed_us = started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
    let pricing = RequestPricing::new(
        &resolved.options,
        &prepared.model,
        &prepared.url,
        prepared.settings.service_tier.as_deref(),
    );
    let cost = pricing.cost(summary.usage.as_ref(), summary.report.as_ref());
    session.commit_compaction_accounting(&agent, execution, EventPayload::Unknown(json!({
            "kind":"runtime.model_usage", "ts":now_timestamp()?, "agent":agent, "turn":pending.turn,
            "request_id":request_id, "attempt":1, "usage":summary.usage, "elapsed_us":elapsed_us, "cost":cost,
        })))?;
    if let Some(usage) = &summary.usage {
        crate::query::step::accumulate_usage(&mut pending.cumulative, usage)?;
    }
    let mut finish = json!({
        "kind":"runtime.compaction_finished", "ts":now_timestamp()?, "agent":agent, "turn":pending.turn,
        "request_id":request_id, "elapsed_ms":elapsed_us/1000, "before_bytes":before_bytes,
    });
    if completed && !waiting_for_input && summary.usable() {
        let text = next.render_checkpoint(summary.text.trim())?;
        candidate.fill_checkpoint(&mut next, pending.turn, text);
        if let Ok(body) = prepare_request(pending, resolved, correlation, &next) {
            let bytes = body.body.as_bytes().len();
            if bytes <= limit && before_bytes.saturating_sub(bytes) >= minimum_savings {
                finish["elapsed_ms"] = u64::try_from(started.elapsed().as_millis())
                    .unwrap_or(u64::MAX)
                    .into();
                finish["status"] = "completed".into();
                finish["after_bytes"] = bytes.into();
                finish["checkpointed_messages"] = candidate.checkpointed_messages().into();
                finish["summary"] = summary.text.trim().into();
                if let Err(error) = candidate.record_checkpoint(pending, finish.clone()) {
                    if matches!(error, crate::Error::ExecutionNotActive(current) if current == execution)
                    {
                        finish["status"] = "cancelled".into();
                        finish["summary"] = serde_json::Value::Null;
                        session.commit_compaction_accounting(
                            &agent,
                            execution,
                            EventPayload::Unknown(finish),
                        )?;
                        return Ok(Outcome::Cancelled);
                    }
                    return Err(error);
                }
                *context = candidate;
                pending.request_base = Some(next);
                return Ok(Outcome::Replace(Box::new(body)));
            }
        }
    }
    let (outcome, status, reason) = if !completed {
        (Outcome::Cancelled, "cancelled", None)
    } else if waiting_for_input {
        (
            Outcome::NewInput,
            "interrupted",
            Some("New input arrived".into()),
        )
    } else {
        (Outcome::Failed, "failed", Some(summary.failure()))
    };
    finish["elapsed_ms"] = u64::try_from(started.elapsed().as_millis())
        .unwrap_or(u64::MAX)
        .into();
    finish["status"] = status.into();
    finish["reason"] = reason.into();
    session.commit_compaction_accounting(&agent, execution, EventPayload::Unknown(finish))?;
    Ok(outcome)
}

#[derive(Default)]
struct Summary {
    text: String,
    usage: Option<ProviderUsage>,
    report: Option<ProviderReport>,
    end: bool,
    stop: Option<ModelStopReason>,
    problem: Option<String>,
}

impl Summary {
    async fn read(&mut self, mut stream: ProviderChunkStream) {
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(ProviderChunk::TextDelta { text }) => {
                    if self.text.len().saturating_add(text.len()) > SUMMARY_BYTES {
                        self.problem = Some("The checkpoint exceeded its output size limit".into());
                    } else if self.problem.is_none() {
                        self.text.push_str(&text);
                    }
                }
                Ok(ProviderChunk::StreamUsage {
                    input_tokens,
                    output_tokens,
                    cache_read_input_tokens,
                    cache_creation_input_tokens,
                }) => {
                    self.usage = Some(ProviderUsage {
                        input_tokens,
                        output_tokens,
                        cache_read_input_tokens,
                        cache_creation_input_tokens,
                    });
                }
                Ok(ProviderChunk::Report { report }) => self.report = Some(report),
                Ok(ProviderChunk::StopReason { reason }) => self.stop = Some(reason),
                Ok(ProviderChunk::StreamEnd) => {
                    self.end = true;
                    break;
                }
                Ok(
                    ProviderChunk::ToolCallStart { .. }
                    | ProviderChunk::ToolCallArgDelta { .. }
                    | ProviderChunk::ToolCallStop { .. }
                    | ProviderChunk::InvalidToolStream,
                ) => {
                    self.problem = Some("The summary response attempted a tool call".into());
                }
                Err(error) => {
                    self.provider_failure(error.kind);
                    break;
                }
                Ok(ProviderChunk::ServerError { .. }) => {
                    self.problem = Some("The provider stopped the summary with an error".into());
                    break;
                }
                Ok(_) => {}
            }
        }
    }

    fn usable(&self) -> bool {
        self.end
            && self.problem.is_none()
            && self.stop == Some(ModelStopReason::EndTurn)
            && !self.text.trim().is_empty()
    }

    fn provider_failure(&mut self, kind: crate::provider::types::ProviderFailureKind) {
        self.problem = Some(format!(
            "Provider error · {}",
            kind.as_event_kind().replace('_', " ")
        ));
    }

    fn failure(self) -> String {
        if self.usable() {
            "The checkpoint did not make the context small enough".into()
        } else {
            self.problem
                .unwrap_or_else(|| "The model did not finish a usable checkpoint".into())
        }
    }
}
