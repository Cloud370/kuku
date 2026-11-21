pub(crate) mod anthropic;
pub(crate) mod capabilities;
pub(crate) mod chunk;
pub(crate) mod config;
mod discovery;
pub(crate) mod error;
pub(crate) mod http_client;
pub(crate) mod openai_compat;
pub(crate) mod openai_responses;
pub(crate) mod prepare;
pub(crate) mod replay;
pub(crate) mod report;
pub(crate) mod sse;
pub(crate) mod trace;
pub(crate) mod transport;
pub(crate) mod types;

pub use discovery::{discover_models, DiscoveredModel};
pub use types::{
    CapabilitySource, DeclaredCapability, Provider, ProviderCapabilities, ProviderReport,
    ProviderSettings, ProviderUsage,
};

use std::pin::Pin;

use futures_core::Stream;

pub(crate) use error::classify_stream_error;
pub(crate) use prepare::prepare_provider_request;
use trace::ProviderTraceMetadata;
use types::{
    PreparedProviderRequest, ProviderFailure, ProviderKind, ProviderRequest, ResolvedProvider,
};

pub(crate) type ProviderChunkStream =
    Pin<Box<dyn Stream<Item = Result<chunk::ProviderChunk, ProviderFailure>> + Send>>;

pub(crate) async fn stream_provider(
    config: &ResolvedProvider,
    request: &ProviderRequest<'_>,
    trace: Option<ProviderTraceMetadata>,
) -> Result<ProviderChunkStream, ProviderFailure> {
    match config.kind {
        ProviderKind::Anthropic => anthropic::stream(config, request, trace).await,
        ProviderKind::OpenAiCompatible => openai_compat::stream(config, request, trace).await,
        ProviderKind::OpenAiResponses => openai_responses::stream(config, request, trace).await,
    }
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
