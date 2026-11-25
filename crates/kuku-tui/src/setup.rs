mod config;
mod connection_name;
mod dashboard;
mod editor;
mod models;
mod probes;
mod recovery;
mod repair;
mod repair_backend;
pub(crate) mod service_tier;
mod state;
mod subscription;
mod ui;

use std::io;

use kuku::config::{ProviderFormat, ProviderProfile, SubscriptionChannel};
use kuku_client::ConnectionOptions;

pub(crate) use config::{model_entry, set_default_model};
pub(crate) use editor::{
    ModelEditor, ModelEditorAction, ThinkingDefaults, ThinkingSaveTarget, ThinkingSetting,
    save_setting, thinking_defaults,
};
pub(crate) use repair::{CredentialRepair, CredentialRepairAction};
pub(crate) use repair_backend::{
    CredentialRepairApplied, CredentialRepairPlan, CredentialRepairRequest,
    PreparedCredentialRepair, SavedCredentialRepair,
};
pub(crate) use repair_backend::{
    apply as apply_credential_repair, prepare as prepare_credential_repair,
    save as save_credential_repair,
};
pub(crate) use state::SetupIntent;
pub use subscription::SubscriptionSettings;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SetupMode {
    FirstRun,
    Session,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Finish {
    SaveAndContinue,
    SaveAndExit,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CredentialSource {
    Environment,
    StoredKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderPreset {
    Anthropic,
    OpenAi,
    DeepSeek,
    OpenRouter,
}

impl ProviderPreset {
    pub(crate) const ALL: [Self; 4] = [
        Self::Anthropic,
        Self::OpenAi,
        Self::DeepSeek,
        Self::OpenRouter,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Anthropic => "Anthropic",
            Self::OpenAi => "OpenAI",
            Self::DeepSeek => "DeepSeek official",
            Self::OpenRouter => "OpenRouter",
        }
    }

    pub(crate) fn connection(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::DeepSeek => "deepseek",
            Self::OpenRouter => "openrouter",
        }
    }

    pub(crate) fn default_model(self) -> &'static str {
        match self {
            Self::Anthropic => "claude-sonnet-4-6",
            Self::OpenAi => "gpt-5.4-mini",
            Self::DeepSeek => "deepseek-flash",
            Self::OpenRouter => "openai/gpt-6-luna",
        }
    }

    pub(crate) fn format(self) -> ProviderFormat {
        match self {
            Self::Anthropic => ProviderFormat::Anthropic,
            Self::OpenAi => ProviderFormat::OpenAiResponses,
            Self::DeepSeek | Self::OpenRouter => ProviderFormat::OpenAiChat,
        }
    }

    pub(crate) fn base_url(self, format: ProviderFormat) -> &'static str {
        match self {
            Self::DeepSeek if format == ProviderFormat::Anthropic => {
                "https://api.deepseek.com/anthropic"
            }
            Self::DeepSeek => "https://api.deepseek.com",
            Self::OpenRouter => "https://openrouter.ai/api/v1",
            Self::OpenAi | Self::Anthropic => format.default_base_url(),
        }
    }

    pub(crate) fn profile(self) -> ProviderProfile {
        match self {
            Self::Anthropic => ProviderProfile::Anthropic,
            Self::OpenAi => ProviderProfile::OpenAi,
            Self::DeepSeek => ProviderProfile::DeepSeek,
            Self::OpenRouter => ProviderProfile::OpenRouter,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConnectionChoice {
    Existing(String),
    Preset(ProviderPreset),
    Subscription(SubscriptionChannel),
    Custom,
}

pub(crate) async fn first_run_required(options: ConnectionOptions) -> io::Result<bool> {
    Ok(config::Settings::load_for_first_run(options)
        .await?
        .is_some())
}

async fn first_run_service_is_idle(options: &ConnectionOptions) -> bool {
    let Ok(service) = options.local_service() else {
        return false;
    };
    matches!(service.status().await, Ok(None))
}

pub(super) fn should_offer_setup(
    status: Result<bool, ()>,
    existing: bool,
    credential_ready: bool,
) -> bool {
    service_is_idle(status) && (!existing || !credential_ready)
}

pub(super) fn service_is_idle(status: Result<bool, ()>) -> bool {
    matches!(status, Ok(false))
}

#[cfg(test)]
#[path = "setup_tests.rs"]
mod tests;
