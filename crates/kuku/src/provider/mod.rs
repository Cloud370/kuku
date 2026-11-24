pub(crate) mod anthropic;
pub(crate) mod capabilities;
pub(crate) mod chunk;
pub(crate) mod config;
pub(crate) mod cost;
mod discovery;
pub(crate) mod error;
pub(crate) mod http_client;
pub(crate) mod openai_compat;
pub(crate) mod openai_responses;
mod openrouter_catalog;
pub(crate) mod prepare;
pub(crate) mod replay;
pub(crate) mod report;
mod request_fields;
mod setup_api;
mod setup_probes;
mod setup_reasoning;
pub(crate) mod sse;
pub(crate) mod subscription;
pub(crate) mod trace;
pub(crate) mod transport;
mod trial;
pub(crate) mod types;

pub use capabilities::setup_requires_responses_for_tools;
pub use cost::{EstimateBasis, RequestCost};
pub use discovery::{
    discover_models, model_list_targets, setup_model_list_targets, DiscoveredModel, ModelListTarget,
};
pub use openrouter_catalog::{
    openrouter_quote_url, openrouter_quotes, route_quote_for_display, RouteQuote,
};
pub use setup_api::{setup_api_candidates, setup_endpoint_format, SetupApiEvidence};
pub use setup_probes::{
    plan_model_trials, Probe, ProbeBatch, ProbeProgress, ProbeResult, ProbeRow, SetupTextCheck,
    TrialSelection,
};
pub use setup_reasoning::{ReasoningCheck, ReasoningCheckResult};
pub use trial::{
    trial_model, trial_model_with_route_request, ModelTrial, TrialResult, TRIAL_OUTPUT_TOKENS,
};
pub use types::{
    CapabilitySource, DeclaredCapability, Provider, ProviderCapabilities, ProviderReport,
    ProviderSettings, ProviderUsage,
};

use std::pin::Pin;

use futures_core::Stream;

pub(crate) use error::classify_stream_error;
use trace::ProviderTraceMetadata;
use types::{
    PreparedProviderRequest, ProviderCorrelation, ProviderFailure, ProviderKind, ProviderRequest,
    ResolvedProvider,
};

pub(crate) type ProviderChunkStream =
    Pin<Box<dyn Stream<Item = Result<chunk::ProviderChunk, ProviderFailure>> + Send>>;

pub(crate) async fn stream_provider(
    config: &ResolvedProvider,
    request: &ProviderRequest<'_>,
    correlation: Option<&ProviderCorrelation>,
    trace: Option<ProviderTraceMetadata>,
) -> Result<ProviderChunkStream, ProviderFailure> {
    let prepared = match correlation {
        Some(correlation) => prepare::prepare_provider_request(config, request, Some(correlation)),
        None => prepare::prepare_single_request(config, request),
    }?;
    stream_prepared_provider(config, &prepared, trace).await
}

pub(crate) async fn stream_prepared_provider(
    config: &ResolvedProvider,
    request: &PreparedProviderRequest,
    trace: Option<ProviderTraceMetadata>,
) -> Result<ProviderChunkStream, ProviderFailure> {
    match request.kind {
        ProviderKind::Anthropic => anthropic::stream_prepared(config, request, trace).await,
        ProviderKind::OpenAiCompatible => {
            openai_compat::stream_prepared(config, request, trace).await
        }
        ProviderKind::OpenAiResponses => {
            openai_responses::stream_prepared(config, request, trace).await
        }
    }
}
