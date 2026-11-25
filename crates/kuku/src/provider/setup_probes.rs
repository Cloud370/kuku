use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::{ModelThinkingTest, ProviderConfig, ProviderFormat, resolve_request_url};
use crate::{Error, Result};

use super::capabilities::probe_scope;
use super::setup_api::{DECLARED_ORDER, check_setup_api};
use super::setup_reasoning::{check_reasoning, reply_accepted};
use super::trial::{TrialProblem, trial_with_route_evidence};
use super::types::ProviderFailureKind;
use super::{
    DiscoveredModel, ModelListTarget, ModelTrial, ReasoningCheck, ReasoningCheckResult, RouteQuote,
    SetupApiEvidence, TrialResult, discover_models, openrouter_quote_url, openrouter_quotes,
};
use tokio::task::{AbortHandle, JoinSet};

const IN_FLIGHT: usize = 2;
const MAX_MODEL_IN_FLIGHT: usize = 6;
const MAX_RATE_LIMIT_RETRIES: u8 = 2;

mod plan;
mod pricing;

pub use plan::{TrialSelection, plan_model_trials};

/// One request in a setup plan.
#[derive(PartialEq)]
pub enum Probe {
    Catalog(Box<ModelListTarget>),
    ApiType {
        connection: Arc<ProviderConfig>,
        model: String,
        candidates: Vec<ProviderFormat>,
    },
    Quotes {
        connection: Arc<ProviderConfig>,
        model: String,
        url: String,
        service_tier: Option<String>,
    },
    Thinking {
        connection: Arc<ProviderConfig>,
        model: String,
        check: ReasoningCheck,
        service_tier: Option<String>,
    },
    Trial {
        connection: Arc<ProviderConfig>,
        model: String,
        check: ModelTrial,
    },
}

impl From<ModelListTarget> for Probe {
    fn from(target: ModelListTarget) -> Self {
        Self::Catalog(Box::new(target))
    }
}

/// Structured response returned by one setup request.
pub enum ProbeResult {
    Catalog(Vec<DiscoveredModel>),
    ApiType(Box<SetupApiEvidence>),
    Quotes(Vec<RouteQuote>),
    Trial(Box<TrialResult>),
    Thinking(ReasoningCheckResult),
}

/// Lifecycle of one request, including its outcome and elapsed time.
pub enum ProbeProgress {
    Queued,
    Waiting(Instant),
    RetryWaiting { until: Instant, sent: bool },
    Finished(Duration, Result<ProbeResult>),
    Canceled { sent: bool },
}

/// A scheduled request and its current state.
pub struct ProbeRow {
    pub probe: Probe,
    pub progress: ProbeProgress,
    pub cached_models: Option<Arc<[DiscoveredModel]>>,
    failure_status: Option<u16>,
    route_missing: bool,
    task: Option<AbortHandle>,
    skipped: bool,
    attempts: u8,
}

/// Whether the model has completed a normal text request during setup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupTextCheck {
    NotTested,
    Testing,
    Ready,
    Failed {
        kind: Option<ProviderFailureKind>,
        status: Option<u16>,
    },
}

enum ProbeCompletion {
    Finished(Result<ProbeResult>, Option<u16>),
    Unavailable(Error, Option<u16>),
    RateLimited {
        retry_after: Option<Duration>,
        error: Error,
    },
}

fn model_completion<T>(
    result: std::result::Result<T, TrialProblem>,
    receipt: impl FnOnce(T) -> ProbeResult,
) -> ProbeCompletion {
    match result {
        Err(problem) if problem.rate_limited => ProbeCompletion::RateLimited {
            retry_after: problem.retry_after,
            error: *problem.error,
        },
        Err(problem) if problem.route_missing => {
            ProbeCompletion::Unavailable(*problem.error, problem.status)
        }
        Ok(value) => ProbeCompletion::Finished(Ok(receipt(value)), None),
        Err(problem) => ProbeCompletion::Finished(Err(*problem.error), problem.status),
    }
}

/// Owns a setup batch. Dropping it cancels in-flight and queued work.
#[derive(Default)]
pub struct ProbeBatch {
    rows: Vec<ProbeRow>,
    tasks: JoinSet<(usize, Duration, ProbeCompletion)>,
    cooldowns: Vec<(Arc<ProviderConfig>, Instant)>,
}

impl ProbeBatch {
    pub fn start(probes: Vec<Probe>) -> Self {
        let mut batch = Self::default();
        batch.append(probes);
        batch
    }

    /// Check the eligible API protocols concurrently and retain the preferred usable result.
    pub fn start_api_check(
        connection: &ProviderConfig,
        model: &str,
        candidates: &[ProviderFormat],
    ) -> Self {
        Self::start(
            DECLARED_ORDER
                .into_iter()
                .filter(|format| candidates.contains(format))
                .map(|format| {
                    let mut route = connection.clone();
                    route.format = format;
                    Probe::ApiType {
                        connection: Arc::new(route),
                        model: model.to_owned(),
                        candidates: vec![format],
                    }
                })
                .collect(),
        )
    }

    /// Finalize once protocol priority determines the outcome, and move it to the first row.
    pub fn finish_api_check(&mut self) -> bool {
        if self.rows.is_empty()
            || self
                .rows
                .iter()
                .any(|row| !matches!(row.probe, Probe::ApiType { .. }))
        {
            return false;
        }
        let index = self
            .rows
            .iter()
            .position(|row| !row.route_missing)
            .unwrap_or(0);
        if !matches!(self.rows[index].progress, ProbeProgress::Finished(..)) {
            return false;
        }
        self.stop();
        self.rows.swap(0, index);
        for row in &mut self.rows[1..] {
            row.skipped = true;
        }
        true
    }

    /// Continue with more work while keeping progress and results of earlier requests.
    pub fn append(&mut self, probes: Vec<Probe>) {
        self.add_rows(probes);
        self.schedule();
    }

    fn add_rows(&mut self, probes: Vec<Probe>) {
        self.rows.extend(probes.into_iter().map(|probe| ProbeRow {
            cached_models: match &probe {
                Probe::Catalog(target) => super::discovery::cached_models(target),
                _ => None,
            },
            probe,
            progress: ProbeProgress::Queued,
            failure_status: None,
            route_missing: false,
            task: None,
            skipped: false,
            attempts: 0,
        }));
    }

    fn schedule(&mut self) {
        let now = Instant::now();
        self.cooldowns.retain(|(_, until)| *until > now);
        let models = self
            .rows
            .iter()
            .filter(|row| !row.skipped)
            .filter_map(|row| row.probe.connection().and_then(|_| row.probe.model()))
            .collect::<HashSet<_>>()
            .len();
        let model_limit = models
            .saturating_mul(IN_FLIGHT)
            .clamp(IN_FLIGHT, MAX_MODEL_IN_FLIGHT);
        let protocol_checks = self
            .rows
            .iter()
            .filter(|row| {
                matches!(row.probe, Probe::ApiType { .. })
                    && !matches!(
                        row.progress,
                        ProbeProgress::Finished(..) | ProbeProgress::Canceled { .. }
                    )
            })
            .count()
            .min(DECLARED_ORDER.len());
        let model_limit = model_limit.max(protocol_checks);
        let limits = [IN_FLIGHT, model_limit];
        let mut active = [0; 2];
        for row in &self.rows {
            if matches!(row.progress, ProbeProgress::Waiting(_)) {
                active[row.probe.lane()] += 1;
            }
        }
        let mut visits = std::collections::HashMap::new();
        let mut queued = Vec::new();
        for (index, row) in self.rows.iter().enumerate() {
            if let Probe::Thinking {
                model,
                connection,
                check: ReasoningCheck::Level(_),
                ..
            } = &row.probe
            {
                *visits
                    .entry((Arc::as_ptr(connection), model))
                    .or_insert(0u16) += u16::from(row.attempts > 0);
            }
            if matches!(
                row.progress,
                ProbeProgress::Queued | ProbeProgress::RetryWaiting { .. }
            ) {
                queued.push(index);
            }
        }
        queued.sort_by_key(|index| match &self.rows[*index].probe {
            Probe::Thinking {
                check: ReasoningCheck::Discover,
                ..
            } => (1, 0),
            Probe::Thinking {
                check: ReasoningCheck::Level(_),
                model,
                connection,
                ..
            } => (2, visits[&(Arc::as_ptr(connection), model)]),
            Probe::Trial {
                check: ModelTrial::ImageInput { .. },
                ..
            } => (2, 0),
            _ => (0, 0),
        });
        for index in queued {
            let baseline = self.baseline(index);
            let row = &mut self.rows[index];
            if let Some((_, until)) = row.probe.connection().and_then(|connection| {
                self.cooldowns
                    .iter()
                    .find(|(paused, _)| same_connection(paused, connection))
            }) {
                row.progress = ProbeProgress::RetryWaiting {
                    until: *until,
                    sent: row.attempts > 0,
                };
                continue;
            }
            row.progress = ProbeProgress::Queued;
            match baseline {
                Some(false) => {
                    row.progress = ProbeProgress::Canceled { sent: false };
                    continue;
                }
                None => continue,
                Some(true) => {}
            }
            let lane = row.probe.lane();
            if active[lane] < limits[lane] {
                active[lane] += 1;
                let started = Instant::now();
                row.progress = ProbeProgress::Waiting(started);
                row.attempts += 1;
                let future = row.probe.run();
                row.task = Some(self.tasks.spawn(async move {
                    let result = future.await;
                    (index, started.elapsed(), result)
                }));
            }
        }
    }

    fn baseline(&self, index: usize) -> Option<bool> {
        let Probe::Thinking {
            model, connection, ..
        } = &self.rows[index].probe
        else {
            return Some(true);
        };
        let baseline = self.rows[..index]
            .iter()
            .filter(|row| !row.skipped)
            .find(|row| match &row.probe {
                Probe::Trial {
                    model: baseline,
                    connection: route,
                    check: ModelTrial::Text { .. },
                } => baseline == model && same_connection(connection, route),
                Probe::ApiType {
                    model: baseline,
                    connection: before,
                    ..
                } => {
                    baseline == model
                        && match &row.progress {
                            ProbeProgress::Finished(_, Ok(ProbeResult::ApiType(evidence))) => {
                                let mut verified = before.as_ref().clone();
                                verified.format = evidence.format;
                                verified.base_url.clone_from(&evidence.base_url);
                                same_connection(connection, &verified)
                            }
                            _ => same_connection(connection, before),
                        }
                }
                _ => false,
            });
        let Some(baseline) = baseline else {
            return Some(true);
        };
        match &baseline.progress {
            ProbeProgress::Finished(_, Ok(ProbeResult::Trial(trial))) => {
                Some(reply_accepted(trial))
            }
            ProbeProgress::Finished(_, Ok(ProbeResult::ApiType(evidence))) => {
                Some(reply_accepted(&evidence.trial))
            }
            ProbeProgress::Finished(..) | ProbeProgress::Canceled { .. } => Some(false),
            _ if matches!(baseline.probe, Probe::ApiType { .. }) => None,
            _ => Some(true),
        }
    }

    pub fn rows(&self) -> &[ProbeRow] {
        &self.rows
    }

    /// Requests for models that are still selected in this batch.
    pub fn selected_rows(&self) -> impl Iterator<Item = &ProbeRow> {
        self.rows.iter().filter(|row| !row.skipped)
    }

    pub fn text_check(&self, model: &str) -> SetupTextCheck {
        let Some(row) = self.rows.iter().rev().find(|row| is_text_row(row, model)) else {
            return SetupTextCheck::NotTested;
        };
        match &row.progress {
            ProbeProgress::Queued
            | ProbeProgress::Waiting(_)
            | ProbeProgress::RetryWaiting { .. } => SetupTextCheck::Testing,
            ProbeProgress::Finished(_, Ok(ProbeResult::Trial(trial))) if trial.text_returned => {
                SetupTextCheck::Ready
            }
            ProbeProgress::Finished(_, Ok(ProbeResult::ApiType(evidence)))
                if evidence.trial.text_returned =>
            {
                SetupTextCheck::Ready
            }
            ProbeProgress::Finished(_, Err(Error::Provider { kind, .. })) => {
                SetupTextCheck::Failed {
                    kind: Some(*kind),
                    status: row.failure_status,
                }
            }
            ProbeProgress::Finished(..) => SetupTextCheck::Failed {
                kind: None,
                status: row.failure_status,
            },
            ProbeProgress::Canceled { .. } => SetupTextCheck::NotTested,
        }
    }

    /// Retry the failed text request and any unsent checks for it, preserving other models.
    pub fn retry_text(&mut self, model: &str, connection: &ProviderConfig) -> bool {
        if !matches!(self.text_check(model), SetupTextCheck::Failed { .. }) {
            return false;
        }
        let Some(row) = self
            .rows
            .iter_mut()
            .rev()
            .find(|row| is_text_row(row, model))
        else {
            return false;
        };
        if matches!(row.probe, Probe::ApiType { .. }) {
            if !matches!(
                row.progress,
                ProbeProgress::Finished(_, Ok(ProbeResult::ApiType(_)))
            ) {
                return false;
            }
            row.probe = Probe::Trial {
                connection: Arc::new(connection.clone()),
                model: model.to_owned(),
                check: ModelTrial::Text {
                    effort: None,
                    service_tier: None,
                },
            };
        }
        row.progress = ProbeProgress::Queued;
        row.failure_status = None;
        row.attempts = 0;
        for row in self
            .rows
            .iter_mut()
            .filter(|row| !row.skipped && row.probe.model() == Some(model))
        {
            if matches!(row.progress, ProbeProgress::Canceled { sent: false }) {
                row.progress = ProbeProgress::Queued;
            }
        }
        self.schedule();
        true
    }

    /// Drop this model's queued checks and stop waiting for its in-flight checks.
    pub fn skip_model(&mut self, model: &str) {
        for row in self
            .rows
            .iter_mut()
            .filter(|row| row.probe.model() == Some(model))
        {
            row.skipped = true;
            if let Some(task) = row.task.take() {
                task.abort();
            }
            match row.progress {
                ProbeProgress::Queued => row.progress = ProbeProgress::Canceled { sent: false },
                ProbeProgress::Waiting(_) => row.progress = ProbeProgress::Canceled { sent: true },
                ProbeProgress::RetryWaiting { sent, .. } => {
                    row.progress = ProbeProgress::Canceled { sent }
                }
                _ => {}
            }
        }
        self.schedule();
    }

    /// Return scoped evidence only when the image challenge succeeded on this route.
    pub fn image_test_scope(&self, connection: &ProviderConfig, model: &str) -> Option<String> {
        let verified = self.selected_rows().any(|row| {
            matches!((&row.probe, &row.progress), (
                Probe::Trial { connection: route, model: selected, check: ModelTrial::ImageInput { .. } },
                ProbeProgress::Finished(_, Ok(ProbeResult::Trial(result)))
            ) if selected == model && result.image_match == Some(true)
                && same_connection(connection, route)
                && connection.options.subscription == route.options.subscription)
        });
        if !verified {
            return None;
        }
        let endpoint = resolve_request_url(
            connection.format,
            &connection.base_url,
            connection.options.endpoint_url.as_deref(),
        )
        .ok()?;
        Some(probe_scope(
            &endpoint,
            connection.format,
            &connection.options,
        ))
    }

    /// Accepted and rejected values for this model on the tested connection.
    pub fn thinking_test(
        &self,
        connection: &ProviderConfig,
        model: &str,
    ) -> Option<ModelThinkingTest> {
        let endpoint = resolve_request_url(
            connection.format,
            &connection.base_url,
            connection.options.endpoint_url.as_deref(),
        )
        .ok()?;
        let mut test = ModelThinkingTest {
            scope: probe_scope(&endpoint, connection.format, &connection.options),
            ..ModelThinkingTest::default()
        };
        for row in self.selected_rows() {
            let (route, selected, level, accepted) = match (&row.probe, &row.progress) {
                (
                    Probe::Thinking {
                        connection,
                        model,
                        check: ReasoningCheck::Level(level),
                        ..
                    },
                    ProbeProgress::Finished(_, Ok(ProbeResult::Thinking(result))),
                ) => (
                    connection,
                    model,
                    level,
                    matches!(result, ReasoningCheckResult::Accepted(_)),
                ),
                (
                    Probe::Trial {
                        connection,
                        model,
                        check:
                            ModelTrial::Text {
                                effort: Some(level),
                                ..
                            },
                    },
                    ProbeProgress::Finished(_, Ok(ProbeResult::Trial(result))),
                ) if reply_accepted(result) => (connection, model, level, true),
                _ => continue,
            };
            if selected == model && same_connection(connection, route) {
                let levels = if accepted {
                    &mut test.accepted
                } else {
                    &mut test.rejected
                };
                if !levels.contains(level) {
                    levels.push(level.clone());
                }
            }
        }
        (!test.accepted.is_empty() || !test.rejected.is_empty()).then_some(test)
    }

    pub fn pending(&self) -> bool {
        !self.tasks.is_empty() || self.next_retry().is_some()
    }

    fn next_retry(&self) -> Option<Instant> {
        self.rows
            .iter()
            .filter_map(|row| match row.progress {
                ProbeProgress::RetryWaiting { until, .. } => Some(until),
                _ => None,
            })
            .min()
    }

    /// Wait for a response or retry timer and start any newly eligible request.
    pub async fn next(&mut self) -> bool {
        self.schedule();
        match (self.next_retry(), self.tasks.is_empty()) {
            (Some(until), true) => tokio::time::sleep_until(until.into()).await,
            (Some(until), false) => tokio::select! {
                Some(joined) = self.tasks.join_next() => self.finished(joined),
                _ = tokio::time::sleep_until(until.into()) => {},
            },
            (None, false) => {
                if let Some(joined) = self.tasks.join_next().await {
                    self.finished(joined);
                }
            }
            (None, true) => return false,
        }
        self.schedule();
        true
    }

    /// Apply results already available without waiting for the network.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Some(joined) = self.tasks.try_join_next() {
            self.finished(joined);
            changed = true;
        }
        if changed
            || self
                .next_retry()
                .is_some_and(|until| until <= Instant::now())
        {
            self.schedule();
            return true;
        }
        changed
    }

    fn finished(
        &mut self,
        joined: std::result::Result<(usize, Duration, ProbeCompletion), tokio::task::JoinError>,
    ) {
        if let Ok((index, elapsed, result)) = joined {
            self.rows[index].task = None;
            if self.rows[index].skipped {
                return;
            }
            let (result, status) = match result {
                ProbeCompletion::Finished(result, status) => (result, status),
                ProbeCompletion::Unavailable(error, status) => {
                    self.rows[index].route_missing = true;
                    (Err(error), status)
                }
                ProbeCompletion::RateLimited { retry_after, error } => {
                    if self.rate_limit(index, retry_after) {
                        return;
                    }
                    (Err(error), Some(429))
                }
            };
            let checks = self.follow_up_thinking(index, &result);
            self.rows[index].progress = ProbeProgress::Finished(elapsed, result);
            self.rows[index].failure_status = status;
            self.update_price_estimates(index);
            self.add_rows(checks);
        } else if joined.is_err_and(|error| !error.is_cancelled()) {
            self.tasks.abort_all();
            self.tasks = JoinSet::new();
            self.mark_canceled();
        }
    }

    fn rate_limit(&mut self, index: usize, retry_after: Option<Duration>) -> bool {
        let row = &mut self.rows[index];
        let Some(connection) = row.probe.connection().cloned() else {
            return false;
        };
        let delay = retry_after.unwrap_or_else(|| Duration::from_secs(1 << row.attempts));
        let Some(until) = Instant::now().checked_add(delay) else {
            return false;
        };
        let until = if let Some((_, cooldown)) = self
            .cooldowns
            .iter_mut()
            .find(|(paused, _)| same_connection(paused, &connection))
        {
            *cooldown = (*cooldown).max(until);
            *cooldown
        } else {
            self.cooldowns.push((connection, until));
            until
        };
        if row.attempts > MAX_RATE_LIMIT_RETRIES {
            return false;
        }
        row.progress = ProbeProgress::RetryWaiting { until, sent: true };
        true
    }

    fn follow_up_thinking(&self, index: usize, result: &Result<ProbeResult>) -> Vec<Probe> {
        let Probe::Thinking {
            connection,
            model,
            check: ReasoningCheck::Discover,
            service_tier,
        } = &self.rows[index].probe
        else {
            return Vec::new();
        };
        let Ok(ProbeResult::Thinking(ReasoningCheckResult::Candidates { values, .. })) = result
        else {
            return Vec::new();
        };
        values
            .iter()
            .filter(|level| {
                !self.rows.iter().any(|row| match &row.probe {
                    Probe::Trial {
                        connection: route,
                        model: other,
                        check:
                            ModelTrial::Text {
                                effort: Some(effort),
                                ..
                            },
                    }
                    | Probe::Thinking {
                        connection: route,
                        model: other,
                        check: ReasoningCheck::Level(effort),
                        ..
                    } => other == model && effort == *level && same_connection(connection, route),
                    _ => false,
                })
            })
            .map(|level| Probe::Thinking {
                connection: Arc::clone(connection),
                model: model.clone(),
                check: ReasoningCheck::Level(level.clone()),
                service_tier: service_tier.clone(),
            })
            .collect()
    }

    pub fn useful_catalog(&self) -> Option<(&ModelListTarget, &[DiscoveredModel])> {
        self.rows
            .iter()
            .find_map(|row| match (&row.probe, &row.progress) {
                (
                    Probe::Catalog(target),
                    ProbeProgress::Finished(_, Ok(ProbeResult::Catalog(items))),
                ) if !items.is_empty() => Some((target.as_ref(), items.as_slice())),
                _ => None,
            })
            .or_else(|| {
                self.rows
                    .iter()
                    .find_map(|row| match (&row.probe, row.cached_models.as_deref()) {
                        (Probe::Catalog(target), Some(items)) if !items.is_empty() => {
                            Some((target.as_ref(), items))
                        }
                        _ => None,
                    })
            })
    }

    pub fn stop(&mut self) {
        while let Some(joined) = self.tasks.try_join_next() {
            self.finished(joined);
        }
        self.tasks.abort_all();
        self.tasks = JoinSet::new();
        self.cooldowns.clear();
        self.mark_canceled();
    }

    fn mark_canceled(&mut self) {
        for row in &mut self.rows {
            row.task = None;
            match row.progress {
                ProbeProgress::Queued => row.progress = ProbeProgress::Canceled { sent: false },
                ProbeProgress::Waiting(_) => row.progress = ProbeProgress::Canceled { sent: true },
                ProbeProgress::RetryWaiting { sent, .. } => {
                    row.progress = ProbeProgress::Canceled { sent }
                }
                ProbeProgress::Finished(..) | ProbeProgress::Canceled { .. } => {}
            }
        }
    }
}

impl Probe {
    /// Model ID for a model-scoped operation.
    pub fn model(&self) -> Option<&str> {
        match self {
            Self::ApiType { model, .. }
            | Self::Quotes { model, .. }
            | Self::Thinking { model, .. }
            | Self::Trial { model, .. } => Some(model),
            Self::Catalog(_) => None,
        }
    }

    fn connection(&self) -> Option<&Arc<ProviderConfig>> {
        match self {
            Self::Thinking { connection, .. }
            | Self::Trial { connection, .. }
            | Self::ApiType { connection, .. } => Some(connection),
            _ => None,
        }
    }

    fn lane(&self) -> usize {
        usize::from(self.connection().is_some())
    }

    fn run(&self) -> impl std::future::Future<Output = ProbeCompletion> + Send + 'static {
        let probe = match self {
            Self::Catalog(target) => Self::Catalog(target.clone()),
            Self::ApiType {
                connection,
                model,
                candidates,
            } => Self::ApiType {
                connection: Arc::clone(connection),
                model: model.clone(),
                candidates: candidates.clone(),
            },
            Self::Quotes {
                connection,
                model,
                url,
                service_tier,
            } => Self::Quotes {
                connection: Arc::clone(connection),
                model: model.clone(),
                url: url.clone(),
                service_tier: service_tier.clone(),
            },
            Self::Trial {
                connection,
                model,
                check,
            } => Self::Trial {
                connection: Arc::clone(connection),
                model: model.clone(),
                check: check.clone(),
            },
            Self::Thinking {
                connection,
                model,
                check,
                service_tier,
            } => Self::Thinking {
                connection: Arc::clone(connection),
                model: model.clone(),
                check: check.clone(),
                service_tier: service_tier.clone(),
            },
        };
        async move {
            match probe {
                Self::Catalog(target) => ProbeCompletion::Finished(
                    discover_models(&target.connection)
                        .await
                        .map(ProbeResult::Catalog),
                    None,
                ),
                Self::ApiType {
                    connection,
                    model,
                    candidates,
                } => model_completion(
                    check_setup_api(&connection, &model, &candidates).await,
                    |evidence| ProbeResult::ApiType(Box::new(evidence)),
                ),
                Self::Quotes {
                    connection, model, ..
                } => ProbeCompletion::Finished(
                    openrouter_quotes(&connection, &model)
                        .await
                        .map(ProbeResult::Quotes),
                    None,
                ),
                Self::Trial {
                    connection,
                    model,
                    check,
                } => model_completion(
                    trial_with_route_evidence(&connection, &model, check).await,
                    |result| ProbeResult::Trial(Box::new(result)),
                ),
                Self::Thinking {
                    connection,
                    model,
                    check,
                    service_tier,
                } => model_completion(
                    check_reasoning(&connection, &model, service_tier, check).await,
                    ProbeResult::Thinking,
                ),
            }
        }
    }
}

fn is_text_row(row: &ProbeRow, model: &str) -> bool {
    !row.skipped
        && row.probe.model() == Some(model)
        && matches!(
            row.probe,
            Probe::ApiType { .. }
                | Probe::Trial {
                    check: ModelTrial::Text { .. },
                    ..
                }
        )
}

fn same_connection(left: &ProviderConfig, right: &ProviderConfig) -> bool {
    left.format == right.format
        && left.base_url == right.base_url
        && left.api_key == right.api_key
        && left.options.endpoint_url == right.options.endpoint_url
        && left.options.profile == right.options.profile
        && left.options.proxy == right.options.proxy
        && left.options.routing == right.options.routing
}
