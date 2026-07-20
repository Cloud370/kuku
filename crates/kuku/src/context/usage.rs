use std::collections::BTreeMap;

use crate::event::{
    CurrencyCode, DecimalCost, ProviderUsage, RequestCompleted, RequestFailed, RequestId,
    TaskEvent, JSON_SAFE_INTEGER_MAX,
};

/// Failure to reduce request lifecycle usage from a Task ledger.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UsageReductionError {
    /// One request has multiple non-identical terminal lifecycle facts.
    #[error("request {request_id} has conflicting terminal lifecycle facts")]
    ConflictingTerminal { request_id: RequestId },
    /// A reduced metric cannot be represented by the JSON-safe wire contract.
    #[error("reduced {metric} exceeds the JSON-safe integer maximum")]
    MetricOverflow { metric: &'static str },
}

/// Provider-reported usage reduced across unique terminal request facts.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageAggregateSummary {
    /// Sum of reported total provider input tokens.
    pub input_tokens: Option<u64>,
    /// Sum of reported provider output tokens.
    pub output_tokens: Option<u64>,
    /// Sum of reported cache-read input tokens.
    pub cached_input_tokens: Option<u64>,
    /// Sum of reported cache-creation input tokens.
    pub cache_creation_input_tokens: Option<u64>,
    /// Number of unique terminal provider requests.
    pub request_count: u64,
    /// Sum of reported request elapsed times.
    pub elapsed_ms: Option<u64>,
    /// Sum of reported request costs.
    pub cost: Option<DecimalCost>,
    /// Cached-input tokens divided by total input tokens when both are available.
    pub cached_input_ratio: Option<f64>,
}

/// Idempotent reducer over the single request lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UsageAggregate {
    terminals: BTreeMap<RequestId, RequestTerminal>,
}

impl UsageAggregate {
    /// Reduces terminal lifecycle facts, ignoring starts and unrelated Task activity.
    pub fn from_lifecycle(
        events: impl IntoIterator<Item = TaskEvent>,
    ) -> Result<Self, UsageReductionError> {
        let mut terminals = BTreeMap::new();
        for event in events {
            let terminal = if let TaskEvent::RequestCompleted(value) = event {
                RequestTerminal::Completed(value)
            } else if let TaskEvent::RequestFailed(value) = event {
                RequestTerminal::Failed(value)
            } else {
                continue;
            };
            let request_id = terminal.request_id().clone();
            if let Some(existing) = terminals.get(&request_id) {
                if existing != &terminal {
                    return Err(UsageReductionError::ConflictingTerminal { request_id });
                }
            } else {
                terminals.insert(request_id, terminal);
            }
        }
        Ok(Self { terminals })
    }

    /// Summarizes all unique terminal requests in this aggregate.
    pub fn summary(&self) -> Result<UsageAggregateSummary, UsageReductionError> {
        summarize(self.terminals.values())
    }

    /// Summarizes one request when its terminal fact exists.
    pub fn request_summary(
        &self,
        request_id: &RequestId,
    ) -> Result<Option<UsageAggregateSummary>, UsageReductionError> {
        self.terminals
            .get(request_id)
            .map(|terminal| summarize(std::iter::once(terminal)))
            .transpose()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RequestTerminal {
    Completed(RequestCompleted),
    Failed(RequestFailed),
}

impl RequestTerminal {
    fn request_id(&self) -> &RequestId {
        match self {
            Self::Completed(value) => &value.scope.request_id,
            Self::Failed(value) => &value.scope.request_id,
        }
    }

    fn usage(&self) -> Option<&ProviderUsage> {
        match self {
            Self::Completed(value) => Some(&value.usage),
            Self::Failed(value) => value.usage.as_ref(),
        }
    }

    fn elapsed_ms(&self) -> Option<u64> {
        match self {
            Self::Completed(value) => value.elapsed_ms,
            Self::Failed(value) => value.elapsed_ms,
        }
    }

    fn cost(&self) -> Option<&DecimalCost> {
        match self {
            Self::Completed(value) => value.cost.as_ref(),
            Self::Failed(value) => value.cost.as_ref(),
        }
    }
}

#[derive(Default)]
struct SummaryBuilder {
    input_tokens: MetricTotal,
    output_tokens: MetricTotal,
    cached_input_tokens: MetricTotal,
    cache_creation_input_tokens: MetricTotal,
    request_count: u64,
    elapsed_ms: MetricTotal,
    cost_micros: MetricTotal,
}

impl SummaryBuilder {
    fn record(&mut self, terminal: &RequestTerminal) -> Result<(), UsageReductionError> {
        self.request_count = checked_sum(self.request_count, 1, "request count")?;
        let usage = terminal.usage();
        self.input_tokens.record(
            usage.and_then(|value| value.input_tokens),
            "input token total",
        )?;
        self.output_tokens.record(
            usage.and_then(|value| value.output_tokens),
            "output token total",
        )?;
        self.cached_input_tokens.record(
            usage.and_then(|value| value.cached_input_tokens),
            "cached input token total",
        )?;
        self.cache_creation_input_tokens.record(
            usage.and_then(|value| value.cache_creation_input_tokens),
            "cache creation input token total",
        )?;
        self.elapsed_ms
            .record(terminal.elapsed_ms(), "elapsed time total")?;
        self.cost_micros
            .record(terminal.cost().map(|cost| cost.micros), "cost total")?;
        Ok(())
    }

    fn finish(self) -> UsageAggregateSummary {
        let input_tokens = self.input_tokens.finish(self.request_count);
        let output_tokens = self.output_tokens.finish(self.request_count);
        let cached_input_tokens = self.cached_input_tokens.finish(self.request_count);
        let cache_creation_input_tokens =
            self.cache_creation_input_tokens.finish(self.request_count);
        let elapsed_ms = self.elapsed_ms.finish(self.request_count);
        let cost_micros = self.cost_micros.finish(self.request_count);
        let cached_input_ratio = match (cached_input_tokens, input_tokens) {
            (Some(cached), Some(input)) if input > 0 => Some(cached as f64 / input as f64),
            (Some(_), Some(_)) | (Some(_), None) | (None, Some(_)) | (None, None) => None,
        };
        UsageAggregateSummary {
            input_tokens,
            output_tokens,
            cached_input_tokens,
            cache_creation_input_tokens,
            request_count: self.request_count,
            elapsed_ms,
            cost: cost_micros.map(|micros| DecimalCost {
                currency: CurrencyCode::Usd,
                micros,
            }),
            cached_input_ratio,
        }
    }
}

#[derive(Default)]
struct MetricTotal {
    sum: u64,
    reported_count: u64,
}

impl MetricTotal {
    fn record(
        &mut self,
        reported: Option<u64>,
        metric: &'static str,
    ) -> Result<(), UsageReductionError> {
        if let Some(reported) = reported {
            self.sum = checked_sum(self.sum, reported, metric)?;
            self.reported_count = checked_sum(self.reported_count, 1, "metric report count")?;
        }
        Ok(())
    }

    fn finish(self, request_count: u64) -> Option<u64> {
        (request_count > 0 && self.reported_count == request_count).then_some(self.sum)
    }
}

fn summarize<'a>(
    terminals: impl IntoIterator<Item = &'a RequestTerminal>,
) -> Result<UsageAggregateSummary, UsageReductionError> {
    let mut builder = SummaryBuilder::default();
    for terminal in terminals {
        builder.record(terminal)?;
    }
    Ok(builder.finish())
}

fn checked_sum(
    current: u64,
    reported: u64,
    metric: &'static str,
) -> Result<u64, UsageReductionError> {
    let total = current
        .checked_add(reported)
        .filter(|value| *value <= JSON_SAFE_INTEGER_MAX)
        .ok_or(UsageReductionError::MetricOverflow { metric })?;
    Ok(total)
}
