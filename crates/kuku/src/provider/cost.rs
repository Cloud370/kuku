use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{ModelPricing, TokenRates};

use super::{ProviderReport, ProviderUsage};

/// Cost evidence for one physical model request.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum RequestCost {
    Reported {
        nanodollars: u64,
    },
    Estimated {
        nanodollars: u64,
        rates: TokenRates,
    },
    #[default]
    Unknown,
}

impl RequestCost {
    pub fn usd(&self) -> Option<f64> {
        match self {
            Self::Reported { nanodollars } | Self::Estimated { nanodollars, .. } => {
                Some(*nanodollars as f64 / 1_000_000_000.0)
            }
            Self::Unknown => None,
        }
    }
}

pub(crate) fn request_cost(
    pricing: Option<&ModelPricing>,
    usage: Option<&ProviderUsage>,
    report: Option<&ProviderReport>,
) -> RequestCost {
    if matches!(pricing, Some(ModelPricing::UsageOnly)) {
        return RequestCost::Unknown;
    }
    if let Some(nanodollars) = report.and_then(|report| nanos(report.cost_usd?)) {
        return RequestCost::Reported { nanodollars };
    }
    let Some(ModelPricing::Configured { rates }) = pricing else {
        return RequestCost::Unknown;
    };
    let Some(nanodollars) = usage.and_then(|usage| {
        let raw = report.and_then(|report| report.raw_usage.as_ref());
        estimate(rates, usage, raw).and_then(nanos)
    }) else {
        return RequestCost::Unknown;
    };
    RequestCost::Estimated {
        nanodollars,
        rates: rates.clone(),
    }
}

fn nanos(usd: f64) -> Option<u64> {
    let nanos = (usd * 1_000_000_000.0).round();
    (nanos.is_finite() && nanos >= 0.0 && nanos < u64::MAX as f64).then_some(nanos as u64)
}

fn estimate(rates: &TokenRates, usage: &ProviderUsage, raw: Option<&Value>) -> Option<f64> {
    let input_price = rates.input?;
    let read = usage.cache_read_input_tokens.or(rates
        .cache_read
        .is_none_or(|rate| rate == input_price)
        .then_some(0))?;
    let write = usage.cache_creation_input_tokens.or((rates
        .cache_write
        .is_none_or(|rate| rate == input_price)
        && rates.cache_write_5m.is_none()
        && rates.cache_write_1h.is_none())
    .then_some(0))?;
    let uncached = usage.input_tokens?.checked_sub(read)?.checked_sub(write)?;
    let write_cost = cache_write_amount(rates, raw, write)?;
    Some(
        (uncached as f64 * input_price
            + priced_tokens(read, rates.cache_read)?
            + write_cost
            + usage.output_tokens? as f64 * rates.output?)
            / 1_000_000.0,
    )
}

fn cache_write_amount(rates: &TokenRates, raw: Option<&Value>, writes: u64) -> Option<f64> {
    if writes == 0 || (rates.cache_write_5m.is_none() && rates.cache_write_1h.is_none()) {
        return priced_tokens(writes, rates.cache_write);
    }
    let parts = raw?.get("cache_creation")?;
    let five = parts.get("ephemeral_5m_input_tokens")?.as_u64()?;
    let hour = parts.get("ephemeral_1h_input_tokens")?.as_u64()?;
    if five.checked_add(hour)? != writes {
        return None;
    }
    Some(
        priced_tokens(five, rates.cache_write_5m.or(rates.cache_write))?
            + priced_tokens(hour, rates.cache_write_1h.or(rates.cache_write))?,
    )
}

fn priced_tokens(tokens: u64, rate: Option<f64>) -> Option<f64> {
    if tokens == 0 {
        Some(0.0)
    } else {
        Some(tokens as f64 * rate?)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        request_cost, ModelPricing, ProviderReport, ProviderUsage, RequestCost, TokenRates,
    };

    #[test]
    fn estimates_uncached_tokens_and_separate_cache_write_durations() {
        let pricing = ModelPricing::Configured {
            rates: TokenRates {
                input: Some(4.0),
                output: Some(20.0),
                cache_read: Some(0.20),
                cache_write_5m: Some(5.0),
                cache_write_1h: Some(8.0),
                ..TokenRates::default()
            },
        };
        let usage = ProviderUsage {
            input_tokens: Some(100),
            output_tokens: Some(10),
            cache_read_input_tokens: Some(60),
            cache_creation_input_tokens: Some(10),
        };
        let mut report = ProviderReport {
            raw_usage: Some(json!({"cache_creation": {
                "ephemeral_5m_input_tokens": 4,
                "ephemeral_1h_input_tokens": 6,
            }})),
            ..ProviderReport::default()
        };
        let cost = request_cost(Some(&pricing), Some(&usage), Some(&report));
        assert!(matches!(
            cost,
            RequestCost::Estimated {
                nanodollars: 400_000,
                ..
            }
        ));
        report.raw_usage = Some(json!({"cache_creation": {"ephemeral_5m_input_tokens": 4}}));
        assert_eq!(
            RequestCost::Unknown,
            request_cost(Some(&pricing), Some(&usage), Some(&report))
        );
        report.cost_usd = Some(0.00012);
        assert_eq!(
            RequestCost::Reported {
                nanodollars: 120_000
            },
            request_cost(Some(&pricing), Some(&usage), Some(&report))
        );
    }
}
