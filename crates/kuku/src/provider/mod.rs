pub(crate) mod anthropic;
pub(crate) mod chunk;
pub(crate) mod config;
pub(crate) mod error;
pub(crate) mod openai_compat;
pub(crate) mod types;

pub use types::Provider;

use futures_core::Stream;
use std::pin::Pin;
use types::{ProviderFailure, ProviderKind, ProviderRequest, ProviderResponse, ResolvedProvider};

pub(crate) type ProviderChunkStream =
    Pin<Box<dyn Stream<Item = Result<chunk::ProviderChunk, ProviderFailure>> + Send>>;

pub(crate) async fn call_provider(
    config: &ResolvedProvider,
    request: &ProviderRequest,
) -> Result<ProviderResponse, ProviderFailure> {
    match config.kind {
        ProviderKind::Anthropic => anthropic::call(config, request).await,
        ProviderKind::OpenAiCompatible => openai_compat::call(config, request).await,
    }
}

pub(crate) async fn stream_provider(
    config: &ResolvedProvider,
    request: &ProviderRequest,
) -> Result<ProviderChunkStream, ProviderFailure> {
    match config.kind {
        ProviderKind::Anthropic => anthropic::stream(config, request).await,
        ProviderKind::OpenAiCompatible => openai_compat::stream(config, request).await,
    }
}
