use std::time::Instant;

use serde_json::json;
use sha2::{Digest, Sha256};
use tokio_stream::StreamExt;

use crate::Result;
use crate::context::{CanonicalMessage, MessageBlock};
use crate::event::store::MAX_RECORD_BYTES;
use crate::event::{EventPayload, ModelStopReason};
use crate::provider::ProviderChunkStream;
use crate::provider::chunk::ProviderChunk;
use crate::provider::cost::RequestPricing;
use crate::provider::types::{
    PreparedProviderRequest, ProviderCorrelation, ProviderReport, ProviderUsage, ResolvedProvider,
};
use crate::query::frozen::ContextCommit;
use crate::query::helpers::now_timestamp;
use crate::query::request::OwnedRequestBase;
use crate::query::types::PendingRun;

use super::dispatch::prepare_request;

const COMPACTION_RELIEF_TOKENS: u32 = 512;
const MIN_RETRY_MATERIAL_TOKENS: u32 = 512;

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
    RequestBytes,
    ProviderOverflow { rejected_request: &'a str },
    Manual { closed_turn: bool },
}

pub(super) fn needed(
    pending: &PendingRun,
    context: &ContextCommit,
    resolved: &ResolvedProvider,
) -> bool {
    let handoff = pending.config.handoff();
    let budget = pending
        .request_base
        .as_ref()
        .expect("prepared task request")
        .context_headroom(resolved)
        .input_budget();
    handoff.enabled
        && budget > 0
        && context.estimated_input_tokens >= pressure_threshold(budget, handoff.threshold)
}

fn pressure_threshold(budget: u32, threshold: f64) -> u32 {
    (f64::from(budget) * threshold).ceil() as u32
}

pub(super) async fn compact(
    pending: &mut PendingRun,
    context: &mut ContextCommit,
    resolved: &ResolvedProvider,
    correlation: &ProviderCorrelation,
    before_bytes: Option<usize>,
    trigger: Trigger<'_>,
) -> Result<Outcome> {
    let manual = matches!(trigger, Trigger::Manual { .. });
    let closed_turn = matches!(trigger, Trigger::Manual { closed_turn: true });
    let rejected_request = match trigger {
        Trigger::ProviderOverflow { rejected_request } => Some(rejected_request),
        _ => None,
    };
    let wire_limit = pending.query.runtime_limits.max_request_bytes;
    let task = pending
        .request_base
        .as_ref()
        .expect("prepared task request");
    let can_admit_input = manual || !task.has_current_images();
    let headroom = task.context_headroom(resolved);
    let context_limit = headroom.total_budget();
    let budget = headroom.input_budget();
    let before_tokens = task.estimated_input_tokens();
    let recent_tokens = budget.min(context.estimated_input_tokens) / 4;
    let Some(cut) = context.checkpoint_cut(pending.turn, recent_tokens, closed_turn) else {
        return Ok(Outcome::Keep);
    };
    let pressure = matches!(trigger, Trigger::Pressure);
    let mut minimum_savings = 1;
    let material_tokens = if pressure {
        let tokens = context.new_checkpoint_material_tokens(cut, pending.turn);
        if pending.failed_compaction.as_ref().is_some_and(|failure| {
            context.matches_failed_compaction(failure)
                && tokens
                    < failure
                        .material_tokens
                        .saturating_add(MIN_RETRY_MATERIAL_TOKENS)
        }) {
            return Ok(Outcome::Keep);
        }
        let threshold = pressure_threshold(budget, pending.config.handoff().threshold);
        if threshold > 0 && context.estimated_input_tokens < budget {
            let relief = COMPACTION_RELIEF_TOKENS.min(threshold / 20);
            minimum_savings = context
                .estimated_input_tokens
                .saturating_sub(threshold.saturating_sub(relief))
                .max(1);
        }
        tokens
    } else {
        0
    };
    if pressure && material_tokens < minimum_savings.max(COMPACTION_RELIEF_TOKENS) {
        return Ok(Outcome::Keep);
    }
    let Some(instruction) = task.checkpoint_instruction()? else {
        return Ok(Outcome::Keep);
    };
    let mut candidate = context.clone();
    let mut next = task.clone();
    candidate.record_checkpoint_files(cut, pending, closed_turn)?;
    let empty_checkpoint = task.render_checkpoint(&candidate.summary_with_files(""))?;
    candidate.apply_checkpoint(&mut next, cut, pending.turn, empty_checkpoint, closed_turn);
    if before_tokens.saturating_sub(next.estimated_input_tokens()) < minimum_savings {
        return Ok(Outcome::Keep);
    }

    let prepared = {
        let mut source = context.checkpoint_source(cut, pending.turn);
        let max_summary_input = if rejected_request.is_some() {
            context_limit.min(before_tokens.saturating_sub((before_tokens / 32).max(1)))
        } else {
            context_limit
        };
        let prepare_summary = |mut checkpoint: OwnedRequestBase| {
            let estimated_input = checkpoint.estimated_input_tokens();
            if estimated_input >= max_summary_input {
                return None;
            }
            checkpoint.limit_checkpoint_output(context_limit - estimated_input);
            let prepared = prepare_request(pending, resolved, correlation, &checkpoint).ok()?;
            wire_limit
                .is_none_or(|limit| prepared.body.as_bytes().len() <= limit)
                .then_some(prepared)
        };
        let warm = task.checkpoint_request(source.clone(), instruction.clone(), true);
        match prepare_summary(warm) {
            Some(prepared) => prepared,
            None => {
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
                );
                let Some(prepared) = prepare_summary(isolated) else {
                    return Ok(Outcome::Keep);
                };
                prepared
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
        "trigger":match trigger {
            Trigger::Pressure => "token_budget",
            Trigger::RequestBytes => "request_byte_budget",
            Trigger::ProviderOverflow { .. } => "provider_overflow",
            Trigger::Manual { .. } => "manual",
        },
        "estimated_input_tokens":context.estimated_input_tokens, "input_token_budget":budget,
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
    let waiting_for_input = completed && can_admit_input && session.has_step_input(&agent)?;
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
        "estimated_input_tokens_before":before_tokens,
        "stop_reason":summary.stop.as_ref(),
    });
    let mut record_too_large = false;
    if completed && !waiting_for_input && summary.usable() {
        let summary_with_files = candidate.summary_with_files(summary.text.trim());
        let text = next.render_checkpoint(&summary_with_files)?;
        candidate.fill_checkpoint(&mut next, pending.turn, text);
        if let Ok(body) = prepare_request(pending, resolved, correlation, &next) {
            let bytes = body.body.as_bytes().len();
            let after_tokens = next.estimated_input_tokens();
            if wire_limit.is_none_or(|limit| bytes <= limit)
                && after_tokens <= budget
                && before_tokens.saturating_sub(after_tokens) >= minimum_savings
            {
                let mut confirmation = finish.clone();
                confirmation["elapsed_ms"] = u64::try_from(started.elapsed().as_millis())
                    .unwrap_or(u64::MAX)
                    .into();
                confirmation["status"] = "completed".into();
                confirmation["after_bytes"] = bytes.into();
                confirmation["estimated_input_tokens_after"] = after_tokens.into();
                if let Some(tokens) = summary.usage.as_ref().and_then(|usage| usage.output_tokens) {
                    confirmation["summary_output_tokens"] = tokens.into();
                }
                confirmation["checkpointed_messages"] = candidate.checkpointed_messages().into();
                confirmation["summary"] = summary_with_files.into();
                match candidate.record_checkpoint(pending, confirmation) {
                    Ok(true) => {
                        *context = candidate;
                        pending.failed_compaction = None;
                        pending.request_base = Some(next);
                        return Ok(Outcome::Replace(Box::new(body)));
                    }
                    Ok(false) => record_too_large = true,
                    Err(crate::Error::ExecutionNotActive(current)) if current == execution => {
                        finish["status"] = "cancelled".into();
                        session.commit_compaction_accounting(
                            &agent,
                            execution,
                            EventPayload::Unknown(finish),
                        )?;
                        return Ok(Outcome::Cancelled);
                    }
                    Err(error) => return Err(error),
                }
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
        let reason = if record_too_large {
            "The checkpoint exceeds the Session journal record limit".into()
        } else {
            summary.failure()
        };
        (Outcome::Failed, "failed", Some(reason))
    };
    finish["elapsed_ms"] = u64::try_from(started.elapsed().as_millis())
        .unwrap_or(u64::MAX)
        .into();
    finish["status"] = status.into();
    finish["reason"] = reason.into();
    if status == "failed" {
        if pressure {
            pending.failed_compaction = Some(context.failed_compaction(material_tokens));
        }
        finish["continues_with_previous_context"] = (matches!(trigger, Trigger::Pressure)
            && wire_limit.is_none_or(|limit| before_bytes.is_some_and(|bytes| bytes <= limit)))
        .into();
    }
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
                    if text.len() > MAX_RECORD_BYTES.saturating_sub(self.text.len()) {
                        self.problem =
                            Some("The checkpoint exceeds the Session journal record limit".into());
                        break;
                    }
                    if self.problem.is_none() {
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
        } else if let Some(problem) = self.problem {
            problem
        } else if self.stop == Some(ModelStopReason::Length) {
            "The model reached its output limit before completing the checkpoint".into()
        } else {
            "The model did not finish a usable checkpoint".into()
        }
    }
}
