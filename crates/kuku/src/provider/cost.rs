use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{ModelPricing, PriceCatalog, ProviderOptions, TokenRates};

use super::capabilities::normalized_tier;
use super::openrouter_catalog::{is_official_openrouter, quote_url_for_request, same_endpoint};
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        basis: Option<EstimateBasis>,
    },
    #[default]
    Unknown,
}

/// The route and source attached to the price snapshot used by an estimate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum EstimateBasis {
    Configured {
        endpoint: Option<String>,
        service_tier: String,
    },
    Catalog {
        endpoint: String,
        service_tier: String,
        catalog_url: String,
    },
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

#[derive(Default)]
pub(crate) struct RequestPricing {
    model: String,
    requested_tier: Option<String>,
    pricing: ModelPricing,
    gateway: bool,
    catalog_url: Option<String>,
    catalog: Option<PriceCatalog>,
}

impl RequestPricing {
    pub(crate) fn new(
        options: &ProviderOptions,
        model: &str,
        request_url: &str,
        requested_tier: Option<&str>,
    ) -> Self {
        let official = url::Url::parse(request_url).is_ok_and(|url| is_official_openrouter(&url));
        let catalog_url = quote_url_for_request(request_url, model);
        let catalog = options
            .model_price_catalogs
            .get(model)
            .filter(|catalog| catalog_url.as_ref() == Some(&catalog.url))
            .cloned();
        Self {
            model: model.into(),
            requested_tier: requested_tier.map(str::to_owned),
            pricing: options
                .model_pricing
                .get(model)
                .cloned()
                .unwrap_or_default(),
            gateway: official || options.profile == crate::config::ProviderProfile::OpenRouter,
            catalog_url,
            catalog,
        }
    }

    pub(super) fn use_catalog(&mut self, catalog: &PriceCatalog) {
        if self.catalog_url.as_ref() == Some(&catalog.url) {
            self.catalog = Some(catalog.clone());
        }
    }

    pub(crate) fn cost(
        &self,
        usage: Option<&ProviderUsage>,
        report: Option<&ProviderReport>,
    ) -> RequestCost {
        if matches!(self.pricing, ModelPricing::UsageOnly) {
            return RequestCost::Unknown;
        }
        if let Some(nanodollars) = report.and_then(|report| nanos(report.cost_usd?)) {
            return RequestCost::Reported { nanodollars };
        }
        let estimated = usage.and_then(|usage| {
            let (rates, basis) = self.rates(report)?;
            if !rates.valid() {
                return None;
            }
            let raw = report.and_then(|report| report.raw_usage.as_ref());
            let nanodollars = nanos(estimate(rates, usage, raw)?)?;
            Some(RequestCost::Estimated {
                nanodollars,
                rates: rates.clone(),
                basis: Some(basis),
            })
        });
        estimated.unwrap_or_default()
    }

    fn rates(&self, report: Option<&ProviderReport>) -> Option<(&TokenRates, EstimateBasis)> {
        let reported_tier = report.and_then(|report| report.service_tier.as_deref());
        let settled_tier = if self.gateway {
            (report?.model.as_deref()? == self.model).then_some(())?;
            reported_tier?
        } else if let Some(tier) = reported_tier {
            tier
        } else {
            let requested = self.tier(self.requested_tier.as_deref());
            (requested == "default").then_some(requested)?
        };
        let tier = self.tier(Some(settled_tier));
        if tier == "auto" {
            return None;
        }
        let configured = |endpoint| EstimateBasis::Configured {
            endpoint,
            service_tier: tier.into(),
        };
        match &self.pricing {
            ModelPricing::Configured { rates } if !self.gateway && tier == "default" => {
                Some((rates, configured(None)))
            }
            ModelPricing::Routed { routes } => {
                let endpoint = if self.gateway {
                    let report = report?;
                    self.catalog_route(report, tier)
                        .map(|quote| quote.tag.as_str())
                        .or_else(|| {
                            report
                                .provider
                                .as_deref()
                                .filter(|provider| provider.contains('/'))
                        })
                } else {
                    report.and_then(|report| report.provider.as_deref())
                };
                let mut matches = routes.iter().filter(|route| {
                    self.tier(route.service_tier.as_deref()) == tier
                        && match (&route.endpoint, endpoint) {
                            (Some(expected), Some(actual)) => {
                                expected == actual
                                    || (self.gateway && same_endpoint(actual, expected))
                            }
                            (None, _) => !self.gateway,
                            _ => false,
                        }
                });
                let selected = matches.next().filter(|_| matches.next().is_none())?;
                Some((&selected.rates, configured(endpoint.map(str::to_owned))))
            }
            ModelPricing::Automatic => {
                let report = report?;
                if report.is_byok != Some(false) {
                    return None;
                }
                let quote = self.catalog_route(report, tier)?;
                Some((
                    &quote.rates,
                    EstimateBasis::Catalog {
                        endpoint: quote.tag.clone(),
                        service_tier: tier.into(),
                        catalog_url: self.catalog.as_ref()?.url.clone(),
                    },
                ))
            }
            _ => None,
        }
    }

    fn tier<'a>(&self, tier: Option<&'a str>) -> &'a str {
        if self.gateway {
            normalized_tier(tier)
        } else {
            match tier {
                None | Some("standard") => "default",
                Some(tier) => tier,
            }
        }
    }

    fn catalog_route(
        &self,
        report: &ProviderReport,
        tier: &str,
    ) -> Option<&crate::config::RouteQuote> {
        let provider = report.provider.as_deref()?;
        let mut quotes = self.catalog.as_ref()?.quotes.iter().filter(|quote| {
            normalized_tier(quote.service_tier.as_deref()) == tier
                && (same_endpoint(&quote.tag, provider)
                    || quote.provider.eq_ignore_ascii_case(provider))
        });
        quotes.next().filter(|_| quotes.next().is_none())
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
        EstimateBasis, ModelPricing, PriceCatalog, ProviderReport, ProviderUsage, RequestCost,
        RequestPricing, TokenRates,
    };
    use crate::config::{ConfiguredRouteRates, ProviderOptions, RouteQuote};

    #[test]
    fn estimates_uncached_tokens_and_separate_cache_write_durations() {
        let pricing = RequestPricing {
            pricing: ModelPricing::Configured {
                rates: TokenRates {
                    input: Some(4.0),
                    output: Some(20.0),
                    cache_read: Some(0.20),
                    cache_write_5m: Some(5.0),
                    cache_write_1h: Some(8.0),
                    ..TokenRates::default()
                },
            },
            ..RequestPricing::default()
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
        let cost = pricing.cost(Some(&usage), Some(&report));
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
            pricing.cost(Some(&usage), Some(&report))
        );
        report.cost_usd = Some(0.00012);
        assert_eq!(
            RequestCost::Reported {
                nanodollars: 120_000
            },
            pricing.cost(Some(&usage), Some(&report))
        );
    }

    #[test]
    fn automatic_estimates_follow_the_serving_provider_and_tier() {
        let model = "google/gemini-4-neon";
        let url = "https://openrouter.ai/api/v1/responses";
        let rates = |input, output| TokenRates {
            input: Some(input),
            output: Some(output),
            ..Default::default()
        };
        let quote = |tag: &str, provider: &str, tier: Option<&str>, input, output| RouteQuote {
            tag: tag.into(),
            provider: provider.into(),
            author_route: true,
            service_tier: tier.map(str::to_owned),
            rates: rates(input, output),
        };
        let mut options = ProviderOptions::default();
        options.model_price_catalogs.insert(
            model.into(),
            PriceCatalog {
                url: "https://openrouter.ai/api/v1/models/google/gemini-4-neon/endpoints".into(),
                quotes: vec![
                    quote(
                        "google-ai-studio/flex",
                        "Google AI Studio",
                        Some("flex"),
                        1.5,
                        7.5,
                    ),
                    quote("google-ai-studio", "Google AI Studio", None, 3.0, 15.0),
                    quote("google-vertex/global", "Google Vertex", None, 3.0, 15.0),
                    quote("google-vertex/us", "Google Vertex", None, 4.0, 16.0),
                ],
            },
        );
        let pricing = RequestPricing::new(&options, model, url, Some("flex"));
        let usage = ProviderUsage {
            input_tokens: Some(100),
            output_tokens: Some(10),
            ..Default::default()
        };
        let mut report = ProviderReport {
            model: Some(model.into()),
            provider: Some("Google AI Studio".into()),
            service_tier: Some("flex".into()),
            is_byok: Some(false),
            ..Default::default()
        };
        assert!(
            matches!(pricing.cost(Some(&usage), Some(&report)), RequestCost::Estimated {
            nanodollars: 225_000,
                basis: Some(EstimateBasis::Catalog { endpoint, .. }),
            ..
        } if endpoint == "google-ai-studio/flex")
        );
        report.service_tier = Some("default".into());
        assert!(matches!(
            pricing.cost(Some(&usage), Some(&report)),
            RequestCost::Estimated {
                nanodollars: 450_000,
                ..
            }
        ));
        report.provider = Some("Google Vertex".into());
        assert_eq!(
            RequestCost::Unknown,
            pricing.cost(Some(&usage), Some(&report))
        );
        report.provider = Some("google-vertex/global".into());
        assert!(matches!(
            pricing.cost(Some(&usage), Some(&report)),
            RequestCost::Estimated { .. }
        ));
        report.is_byok = Some(true);
        assert_eq!(
            RequestCost::Unknown,
            pricing.cost(Some(&usage), Some(&report))
        );
        report.cost_usd = Some(0.0);
        assert_eq!(
            RequestCost::Reported { nanodollars: 0 },
            pricing.cost(Some(&usage), Some(&report))
        );
    }

    #[test]
    fn configured_route_rates_need_the_matching_tier_and_preserve_legacy_costs() {
        let rates = TokenRates {
            input: Some(2.0),
            output: Some(4.0),
            ..Default::default()
        };
        let pricing = RequestPricing {
            requested_tier: Some("priority".into()),
            pricing: ModelPricing::Routed {
                routes: vec![ConfiguredRouteRates {
                    endpoint: None,
                    service_tier: Some("priority".into()),
                    rates: rates.clone(),
                }],
            },
            ..Default::default()
        };
        let usage = ProviderUsage {
            input_tokens: Some(50),
            output_tokens: Some(10),
            ..Default::default()
        };
        assert_eq!(RequestCost::Unknown, pricing.cost(Some(&usage), None));
        let confirmed = ProviderReport {
            service_tier: Some("priority".into()),
            ..Default::default()
        };
        assert!(
            matches!(pricing.cost(Some(&usage), Some(&confirmed)), RequestCost::Estimated {
            nanodollars: 140_000,
            basis: Some(EstimateBasis::Configured { service_tier, .. }),
            ..
        } if service_tier == "priority")
        );
        let fallback = ProviderReport {
            service_tier: Some("standard".into()),
            ..Default::default()
        };
        assert_eq!(
            RequestCost::Unknown,
            pricing.cost(Some(&usage), Some(&fallback))
        );
        let legacy = serde_json::from_value::<RequestCost>(json!({
            "source": "estimated", "nanodollars": 140000, "rates": rates
        }))
        .unwrap();
        assert!(matches!(legacy, RequestCost::Estimated { basis: None, .. }));
    }
}
