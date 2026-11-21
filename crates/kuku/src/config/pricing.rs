use serde::{Deserialize, Serialize};

/// Rates in USD per million tokens for one connection and model.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TokenRates {
    pub input: Option<f64>,
    pub output: Option<f64>,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
    pub cache_write_5m: Option<f64>,
    pub cache_write_1h: Option<f64>,
}

impl TokenRates {
    pub(super) fn valid(&self) -> bool {
        [
            self.input,
            self.output,
            self.cache_read,
            self.cache_write,
            self.cache_write_5m,
            self.cache_write_1h,
        ]
        .into_iter()
        .flatten()
        .all(|rate| rate.is_finite() && rate >= 0.0)
    }
}

/// How to obtain cost information for an exact model on this connection.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "kebab-case")]
pub enum ModelPricing {
    #[default]
    Automatic,
    Configured {
        #[serde(flatten)]
        rates: TokenRates,
    },
    UsageOnly,
}
