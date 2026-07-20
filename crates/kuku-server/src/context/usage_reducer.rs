use kuku::context::{UsageAggregate, UsageAggregateSummary, UsageReductionError};
use kuku::event::{RequestId, TaskEvent};

use crate::api::{ContextUsage, UsageSummary};

#[derive(Debug, Clone)]
pub(crate) struct UsageReducer {
    aggregate: UsageAggregate,
}

impl UsageReducer {
    pub(crate) fn from_lifecycle(
        events: impl IntoIterator<Item = TaskEvent>,
    ) -> Result<Self, UsageReductionError> {
        Ok(Self {
            aggregate: UsageAggregate::from_lifecycle(events)?,
        })
    }

    pub(crate) fn for_request(
        &self,
        request_id: &RequestId,
    ) -> Result<Option<UsageSummary>, UsageReductionError> {
        Ok(self
            .aggregate
            .request_summary(request_id)?
            .map(usage_summary))
    }

    pub(crate) fn for_task(&self) -> Result<UsageSummary, UsageReductionError> {
        Ok(usage_summary(self.aggregate.summary()?))
    }

    pub(crate) fn for_context(
        &self,
        selected: Option<&RequestId>,
    ) -> Result<ContextUsage, UsageReductionError> {
        Ok(ContextUsage {
            this_request: selected
                .map(|request_id| self.for_request(request_id))
                .transpose()?
                .flatten(),
            this_task: self.for_task()?,
        })
    }
}

fn usage_summary(value: UsageAggregateSummary) -> UsageSummary {
    UsageSummary {
        input_tokens: value.input_tokens,
        output_tokens: value.output_tokens,
        cached_input_tokens: value.cached_input_tokens,
        cache_creation_input_tokens: value.cache_creation_input_tokens,
        request_count: value.request_count,
        elapsed_ms: value.elapsed_ms,
        cost: value.cost,
        cached_input_ratio: value.cached_input_ratio,
    }
}
