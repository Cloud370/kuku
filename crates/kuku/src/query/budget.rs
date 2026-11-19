use std::sync::atomic::Ordering;

use crate::{Error, Result};

use super::Query;

pub(super) fn exhausted(query: &Query) -> bool {
    query
        .model_request_budget
        .as_ref()
        .is_some_and(|remaining| remaining.load(Ordering::Relaxed) == 0)
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
