use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::{ModelPricing, ProviderConfig};
use crate::Result;

use super::{
    discover_models, openrouter_quote_url, openrouter_quotes, trial_model, DiscoveredModel,
    ModelListTarget, ModelTrial, RouteQuote, TrialResult,
};
use tokio::task::JoinSet;

const IN_FLIGHT: usize = 2;

/// A selected model with the settings to check at its actual endpoint.
#[derive(Debug, Clone)]
pub struct TrialSelection {
    pub model: String,
    pub effort: Option<String>,
    pub service_tier: Option<String>,
}

/// Plan bounded trial and pricing requests. Calling this does not dispatch any requests.
pub fn plan_model_trials(
    connection: Arc<ProviderConfig>,
    models: impl IntoIterator<Item = TrialSelection>,
) -> Vec<Probe> {
    let mut requests = Vec::new();
    for TrialSelection {
        model,
        effort,
        service_tier,
    } in models
    {
        if matches!(
            connection.options.model_pricing.get(&model),
            None | Some(ModelPricing::Automatic)
        ) {
            if let Some(url) = openrouter_quote_url(&connection, &model) {
                requests.push(Probe::Quotes {
                    connection: Arc::clone(&connection),
                    model: model.clone(),
                    url,
                    service_tier: service_tier.clone(),
                });
            }
        }
        let image = crate::adapter::inspect_connection_model(&connection, &model)
            .ok()
            .and_then(|caps| caps.input_modalities)
            .is_none_or(|media| media.value.iter().any(|item| item == "image"));
        requests.push(Probe::Trial {
            connection: Arc::clone(&connection),
            model: model.clone(),
            check: ModelTrial::Text {
                effort,
                service_tier: service_tier.clone(),
            },
        });
        if image {
            requests.push(Probe::Trial {
                connection: Arc::clone(&connection),
                model,
                check: ModelTrial::ImageInput { service_tier },
            });
        }
    }
    requests
}

/// One request in a setup plan.
#[derive(PartialEq)]
pub enum Probe {
    Catalog(Box<ModelListTarget>),
    Quotes {
        connection: Arc<ProviderConfig>,
        model: String,
        url: String,
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
    Quotes(Vec<RouteQuote>),
    Trial(Box<TrialResult>),
}

/// Lifecycle of one request, including its outcome and elapsed time.
pub enum ProbeProgress {
    Queued,
    Waiting(Instant),
    Finished(Duration, Result<ProbeResult>),
    Canceled { sent: bool },
}

/// A scheduled request and its current state.
pub struct ProbeRow {
    pub probe: Probe,
    pub progress: ProbeProgress,
    pub cached_models: Option<Arc<[DiscoveredModel]>>,
}

/// Owns a setup batch. Dropping it cancels in-flight and queued work.
#[derive(Default)]
pub struct ProbeBatch {
    rows: Vec<ProbeRow>,
    tasks: JoinSet<(usize, Duration, Result<ProbeResult>)>,
}

impl ProbeBatch {
    pub fn start(probes: Vec<Probe>) -> Self {
        let mut batch = Self {
            rows: probes
                .into_iter()
                .map(|probe| ProbeRow {
                    cached_models: match &probe {
                        Probe::Catalog(target) => super::discovery::cached_models(target),
                        _ => None,
                    },
                    probe,
                    progress: ProbeProgress::Queued,
                })
                .collect(),
            ..Self::default()
        };
        batch.schedule();
        batch
    }

    fn schedule(&mut self) {
        let mut active = [0; 2];
        for row in &self.rows {
            if matches!(row.progress, ProbeProgress::Waiting(_)) {
                active[row.probe.lane()] += 1;
            }
        }
        for (index, row) in self.rows.iter_mut().enumerate() {
            let lane = row.probe.lane();
            if active[lane] < IN_FLIGHT && matches!(row.progress, ProbeProgress::Queued) {
                active[lane] += 1;
                let started = Instant::now();
                row.progress = ProbeProgress::Waiting(started);
                let future = row.probe.run();
                self.tasks.spawn(async move {
                    let result = future.await;
                    (index, started.elapsed(), result)
                });
            }
        }
    }

    pub fn rows(&self) -> &[ProbeRow] {
        &self.rows
    }

    pub fn pending(&self) -> bool {
        !self.tasks.is_empty()
    }

    /// Wait for a request to finish and immediately start any newly eligible request.
    pub async fn next(&mut self) -> bool {
        let Some(joined) = self.tasks.join_next().await else {
            return false;
        };
        self.finished(joined);
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
        if changed {
            self.schedule();
        }
        changed
    }

    fn finished(
        &mut self,
        joined: std::result::Result<(usize, Duration, Result<ProbeResult>), tokio::task::JoinError>,
    ) {
        if let Ok((index, elapsed, result)) = joined {
            self.rows[index].progress = ProbeProgress::Finished(elapsed, result);
        } else {
            self.tasks.abort_all();
            self.tasks = JoinSet::new();
            self.mark_canceled();
        }
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
        self.mark_canceled();
    }

    fn mark_canceled(&mut self) {
        for row in &mut self.rows {
            match row.progress {
                ProbeProgress::Queued => row.progress = ProbeProgress::Canceled { sent: false },
                ProbeProgress::Waiting(_) => row.progress = ProbeProgress::Canceled { sent: true },
                ProbeProgress::Finished(..) | ProbeProgress::Canceled { .. } => {}
            }
        }
    }
}

impl Probe {
    fn lane(&self) -> usize {
        usize::from(matches!(self, Self::Trial { .. }))
    }

    fn run(&self) -> impl std::future::Future<Output = Result<ProbeResult>> + Send + 'static {
        let probe = match self {
            Self::Catalog(target) => Self::Catalog(target.clone()),
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
        };
        async move {
            match probe {
                Self::Catalog(target) => discover_models(&target.connection)
                    .await
                    .map(ProbeResult::Catalog),
                Self::Quotes {
                    connection, model, ..
                } => openrouter_quotes(&connection, &model)
                    .await
                    .map(ProbeResult::Quotes),
                Self::Trial {
                    connection,
                    model,
                    check,
                } => trial_model(&connection, &model, check)
                    .await
                    .map(|result| ProbeResult::Trial(Box::new(result))),
            }
        }
    }
}
