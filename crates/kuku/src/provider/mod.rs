#![allow(dead_code)]

pub(crate) mod anthropic;
pub(crate) mod config;
pub(crate) mod error;
pub(crate) mod openai_compat;
pub(crate) mod types;

pub use types::Provider;

use types::{ProviderFailure, ProviderKind, ProviderRequest, ProviderResponse, ResolvedProvider};

#[allow(unused_imports)]
pub(crate) use config::{resolve_config, ResolveConfigInput};

pub(crate) async fn call_provider(
    config: &ResolvedProvider,
    request: &ProviderRequest,
) -> Result<ProviderResponse, ProviderFailure> {
    match config.kind {
        ProviderKind::Anthropic => anthropic::call(config, request).await,
        ProviderKind::OpenAiCompatible => openai_compat::call(config, request).await,
    }
}
