use std::sync::atomic::Ordering;

use crate::{Error, Result};

use super::Query;

pub(super) fn exhausted(query: &Query) -> bool {
    query
        .model_request_budget
        .as_ref()
        .is_some_and(|remaining| remaining.load(Ordering::Relaxed) == 0)
}

pub(super) fn try_reserve_compaction(query: &Query, compact_only: bool) -> bool {
    query.model_request_budget.as_ref().is_none_or(|remaining| {
        remaining
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value
                    .checked_sub(1)
                    .filter(|remaining| compact_only || *remaining > 0)
            })
            .is_ok()
    })
}

pub(super) fn reserve(query: &Query) -> Result<()> {
    if let Some(remaining) = &query.model_request_budget {
        remaining
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_sub(1)
            })
            .map_err(|_| {
                Error::InvalidArgument(
                    "model request limit exhausted for this open session; no request was dispatched"
                        .into(),
                )
            })?;
    }
    Ok(())
}
