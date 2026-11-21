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
    pub(crate) fn valid(&self) -> bool {
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
    Routed {
        routes: Vec<ConfiguredRouteRates>,
    },
    UsageOnly,
}

impl ModelPricing {
    pub(super) fn valid(&self) -> bool {
        match self {
            Self::Configured { rates } => rates.valid(),
            Self::Routed { routes } => {
                !routes.is_empty()
                    && routes.iter().all(|route| {
                        route
                            .endpoint
                            .as_ref()
                            .is_none_or(|value| !value.trim().is_empty())
                            && route
                                .service_tier
                                .as_ref()
                                .is_none_or(|value| !value.trim().is_empty())
                            && route.rates.valid()
                    })
            }
            Self::Automatic | Self::UsageOnly => true,
        }
    }
}

/// Manually configured rates for a serving endpoint and native service tier.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfiguredRouteRates {
    /// The serving endpoint tag. Omit on a direct connection.
    pub endpoint: Option<String>,
    /// Omission denotes the standard service tier.
    pub service_tier: Option<String>,
    #[serde(flatten)]
    pub rates: TokenRates,
}

/// An advertised quote for an OpenRouter endpoint in USD per million tokens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteQuote {
    pub tag: String,
    pub provider: String,
    pub author_route: bool,
    pub service_tier: Option<String>,
    pub rates: TokenRates,
}

/// Public endpoint quotes saved during model setup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriceCatalog {
    pub url: String,
    pub quotes: Vec<RouteQuote>,
}
