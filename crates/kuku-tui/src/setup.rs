mod config;
mod state;
mod ui;

use std::io::{self, IsTerminal};

use kuku_client::ConnectionOptions;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SetupMode {
    Init,
    FirstRun,
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

    pub(crate) fn default_env(self) -> &'static str {
        match self {
            Self::Anthropic => "ANTHROPIC_API_KEY",
            Self::OpenAi => "OPENAI_API_KEY",
            Self::DeepSeek => "DEEPSEEK_API_KEY",
            Self::OpenRouter => "OPENROUTER_API_KEY",
        }
    }

    pub(crate) fn format(self) -> &'static str {
        match self {
            Self::Anthropic | Self::DeepSeek => "anthropic",
            Self::OpenAi => "openai-responses",
            Self::OpenRouter => "openai-chat",
        }
    }

    pub(crate) fn base_url(self) -> &'static str {
        match self {
            Self::Anthropic => "https://api.anthropic.com",
            Self::OpenAi => "https://api.openai.com/v1",
            Self::DeepSeek => "https://api.deepseek.com/anthropic",
            Self::OpenRouter => "https://openrouter.ai/api/v1",
        }
    }

    pub(crate) fn profile(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::DeepSeek => "deepseek",
            Self::OpenRouter => "openrouter",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConnectionChoice {
    Existing(String),
    Preset(ProviderPreset),
}

pub(crate) async fn run(options: ConnectionOptions, mode: SetupMode) -> io::Result<bool> {
    let settings = match mode {
        SetupMode::Init => {
            validate_entry(
                &options,
                io::stdin().is_terminal() && io::stdout().is_terminal(),
                "Interactive settings require a terminal; edit config.toml or use `kuku config set`.",
            )?;
            config::Settings::load(options.clone()).await?
        }
        SetupMode::FirstRun => {
            let Some(settings) = config::Settings::load_for_first_run(options.clone()).await?
            else {
                return Ok(true);
            };
            validate_entry(
                &options,
                io::stdin().is_terminal() && io::stdout().is_terminal(),
                "Interactive settings require a terminal; edit config.toml or use `kuku config set`.",
            )?;
            settings
        }
    };
    let mut state = state::SetupState::new(&settings);
    let finish = ui::run(&settings, &mut state, mode)?;
    if finish == Finish::Cancel {
        println!("Setup canceled. No files were changed.");
        return Ok(false);
    }
    if mode == SetupMode::FirstRun && !first_run_service_is_idle(&options).await {
        println!("Local service is active or unavailable; setup was not saved.");
        return Ok(true);
    }
    config::save(&settings, &state)?;
    println!("Saved settings to {}.", settings.config_path.display());
    match state.credential_source {
        CredentialSource::Environment => {
            if let Some(name) = state.environment_name() {
                println!(
                    "Set {name} in the shell that launches kuku. The credential was not validated."
                );
            }
        }
        CredentialSource::StoredKey => println!(
            "Saved locally as plaintext, not encrypted, under {}.",
            settings.credentials_dir.display()
        ),
    }
    println!("Setup made no model request.");
    Ok(finish == Finish::SaveAndContinue)
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

pub(crate) fn validate_entry(
    options: &ConnectionOptions,
    terminal: bool,
    guidance: &str,
) -> io::Result<()> {
    if !terminal {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("{guidance} No files were changed."),
        ));
    }
    if options.remote.is_some() || options.token.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Provider settings are local-only; omit --remote and --token.",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "setup_tests.rs"]
mod tests;
