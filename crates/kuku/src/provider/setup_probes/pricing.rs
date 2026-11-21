use crate::config::{resolve_request_url, PriceCatalog, ProviderConfig, RouteQuote};
use crate::provider::cost::RequestPricing;
use crate::provider::openrouter_catalog::service_tiers;

use super::{
    openrouter_quote_url, ModelTrial, Probe, ProbeBatch, ProbeProgress, ProbeResult, ProbeRow,
    ReasoningCheckResult,
};

impl ProbeBatch {
    /// Public prices captured for the connection and model in this opted-in setup.
    pub fn price_catalog(&self, connection: &ProviderConfig, model: &str) -> Option<PriceCatalog> {
        let url = openrouter_quote_url(connection, model)?;
        let quotes = self.catalog_quotes(model, &url)?.to_vec();
        Some(PriceCatalog { url, quotes })
    }

    /// Service tiers advertised by the public catalog for the selected model.
    pub fn advertised_service_tiers(
        &self,
        connection: &ProviderConfig,
        model: &str,
    ) -> Option<Vec<String>> {
        let url = openrouter_quote_url(connection, model)?;
        service_tiers(
            self.catalog_quotes(model, &url)?,
            connection.options.routing.as_ref(),
        )
    }

    fn catalog_quotes(&self, model: &str, url: &str) -> Option<&[RouteQuote]> {
        self.rows
            .iter()
            .rev()
            .filter(|row| !row.skipped)
            .find_map(|row| match quote_result(row)? {
                (quoted_model, quoted_url, quotes)
                    if quoted_model == model && quoted_url == url =>
                {
                    Some(quotes)
                }
                _ => None,
            })
    }

    pub(super) fn update_price_estimates(&mut self, index: usize) {
        if let Some((model, url, quotes)) = quote_result(&self.rows[index]) {
            let model = model.to_owned();
            let catalog = PriceCatalog {
                url: url.into(),
                quotes: quotes.to_vec(),
            };
            for row in &mut self.rows {
                if row.probe.model() == Some(model.as_str()) {
                    reprice(row, &catalog);
                }
            }
        } else if let Some(catalog) = self.rows[index]
            .probe
            .connection()
            .and_then(|connection| self.price_catalog(connection, self.rows[index].probe.model()?))
        {
            reprice(&mut self.rows[index], &catalog);
        }
    }
}

fn quote_result(row: &ProbeRow) -> Option<(&str, &str, &[RouteQuote])> {
    match (&row.probe, &row.progress) {
        (
            Probe::Quotes { model, url, .. },
            ProbeProgress::Finished(_, Ok(ProbeResult::Quotes(quotes))),
        ) => Some((model, url, quotes)),
        _ => None,
    }
}

fn reprice(row: &mut ProbeRow, catalog: &PriceCatalog) {
    let (Some(connection), Some(model)) = (row.probe.connection(), row.probe.model()) else {
        return;
    };
    let ProbeProgress::Finished(_, Ok(result)) = &mut row.progress else {
        return;
    };
    let (format, base, tier, trial) = match (&row.probe, result) {
        (Probe::Trial { check, .. }, ProbeResult::Trial(trial)) => {
            let tier = match check {
                ModelTrial::Text { service_tier, .. }
                | ModelTrial::ImageInput { service_tier }
                | ModelTrial::Tools { service_tier } => service_tier.as_deref(),
            };
            (
                connection.format,
                connection.base_url.as_str(),
                tier,
                trial.as_mut(),
            )
        }
        (
            Probe::Thinking { service_tier, .. },
            ProbeResult::Thinking(
                ReasoningCheckResult::Accepted(trial)
                | ReasoningCheckResult::Candidates {
                    trial: Some(trial), ..
                },
            ),
        ) => (
            connection.format,
            connection.base_url.as_str(),
            service_tier.as_deref(),
            trial.as_mut(),
        ),
        (Probe::ApiType { .. }, ProbeResult::ApiType(evidence)) => (
            evidence.format,
            evidence.base_url.as_str(),
            None,
            &mut evidence.trial,
        ),
        _ => return,
    };
    let Ok(url) = resolve_request_url(format, base, connection.options.endpoint_url.as_deref())
    else {
        return;
    };
    let mut pricing = RequestPricing::new(&connection.options, model, &url, tier);
    pricing.use_catalog(catalog);
    trial.cost = pricing.cost(trial.usage.as_ref(), trial.provider_report.as_ref());
    if trial.continuation_cost.is_some() {
        trial.continuation_cost = Some(pricing.cost(
            trial.continuation_usage.as_ref(),
            trial.continuation_report.as_ref(),
        ));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use crate::config::{ApiKey, ProviderFormat, TokenRates};
    use crate::provider::{ProviderReport, ProviderUsage, RequestCost, TrialResult};

    use super::{
        openrouter_quote_url, ModelTrial, Probe, ProbeBatch, ProbeProgress, ProbeResult,
        ProviderConfig, RouteQuote,
    };

    #[test]
    fn arriving_catalog_reprices_a_completed_trial_and_continuation() {
        let model = "anthropic/claude-opus-5.5";
        let connection = Arc::new(ProviderConfig {
            format: ProviderFormat::OpenAiResponses,
            base_url: "https://openrouter.ai/api/v1".into(),
            api_key: ApiKey::Plaintext("unused".into()),
            options: Default::default(),
        });
        let url = openrouter_quote_url(&connection, model).unwrap();
        let report = ProviderReport {
            model: Some(model.into()),
            provider: Some("Anthropic".into()),
            service_tier: Some("priority".into()),
            is_byok: Some(false),
            ..Default::default()
        };
        let usage = ProviderUsage {
            input_tokens: Some(100),
            output_tokens: Some(10),
            ..Default::default()
        };
        let mut batch = ProbeBatch::default();
        batch.add_rows(vec![
            Probe::Trial {
                connection: Arc::clone(&connection),
                model: model.into(),
                check: ModelTrial::Tools {
                    service_tier: Some("priority".into()),
                },
            },
            Probe::Quotes {
                connection: Arc::clone(&connection),
                model: model.into(),
                url,
                service_tier: Some("priority".into()),
            },
        ]);
        batch.rows[0].progress = ProbeProgress::Finished(
            Duration::ZERO,
            Ok(ProbeResult::Trial(Box::new(TrialResult {
                text_returned: true,
                observed_reasoning: false,
                reported_effort: None,
                stop_reason: None,
                image_match: None,
                expected_colors: None,
                observed_colors: Vec::new(),
                usage: Some(usage.clone()),
                provider_report: Some(report.clone()),
                cost: RequestCost::Unknown,
                tool_round_trip: Some(true),
                continuation_cost: Some(RequestCost::Unknown),
                continuation_usage: Some(usage),
                continuation_report: Some(report),
            }))),
        );
        batch.rows[1].progress = ProbeProgress::Finished(
            Duration::ZERO,
            Ok(ProbeResult::Quotes(vec![RouteQuote {
                tag: "anthropic/fast".into(),
                provider: "Anthropic".into(),
                author_route: true,
                service_tier: Some("priority".into()),
                rates: TokenRates {
                    input: Some(8.0),
                    output: Some(40.0),
                    ..Default::default()
                },
            }])),
        );
        for finished in [1, 0] {
            batch.update_price_estimates(finished);
            let ProbeProgress::Finished(_, Ok(ProbeResult::Trial(trial))) = &batch.rows[0].progress
            else {
                panic!("missing trial");
            };
            assert!(matches!(
                trial.cost,
                RequestCost::Estimated {
                    nanodollars: 1_200_000,
                    ..
                }
            ));
            assert_eq!(Some(&trial.cost), trial.continuation_cost.as_ref());
        }
        assert_eq!(
            1,
            batch
                .price_catalog(&connection, model)
                .unwrap()
                .quotes
                .len()
        );
    }
}
