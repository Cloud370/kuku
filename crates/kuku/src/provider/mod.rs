pub(crate) mod anthropic;
pub(crate) mod chunk;
pub(crate) mod config;
pub(crate) mod error;
pub(crate) mod openai_compat;
pub(crate) mod openai_responses;
pub(crate) mod types;

pub use types::{Provider, ProviderUsage};

use std::pin::Pin;

use futures_core::Stream;

use types::{ProviderFailure, ProviderKind, ProviderRequest, ResolvedProvider};

pub(crate) type ProviderChunkStream =
    Pin<Box<dyn Stream<Item = Result<chunk::ProviderChunk, ProviderFailure>> + Send + Sync>>;

pub(crate) async fn stream_provider(
    config: &ResolvedProvider,
    request: &ProviderRequest,
) -> Result<ProviderChunkStream, ProviderFailure> {
    match config.kind {
        ProviderKind::Anthropic => anthropic::stream(config, request).await,
        ProviderKind::OpenAiCompatible => openai_compat::stream(config, request).await,
        ProviderKind::OpenAiResponses => openai_responses::stream(config, request).await,
    }
}
