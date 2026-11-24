use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use url::Url;

use super::{
    resolve_models_url, resolve_request_url, ProviderAuth, ProviderFormat, ProviderOptions,
    ProviderProfile, ProviderProxy,
};
use crate::{Error, Result};

pub(crate) const CHATGPT_RESOURCE: &str = "https://api.openai.com/v1";
pub(crate) const CHATGPT_MODELS: &str = "https://api.openai.com/v1/models";
pub(crate) const CHATGPT_RESPONSES: &str = "https://api.openai.com/v1/responses";

/// A provider's subscription authorization and inference channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubscriptionChannel {
    /// Sign in with ChatGPT for eligible OpenAI Responses requests.
    ChatGpt,
}

impl SubscriptionChannel {
    /// Return the channel name used in configuration and account paths.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ChatGpt => "chatgpt",
        }
    }

    /// Return this channel's official browser page for managing plan usage.
    pub fn usage_url(self) -> &'static str {
        match self {
            Self::ChatGpt => "https://chatgpt.com/settings/usage",
        }
    }

    /// Validate network settings for a subscription channel.
    pub fn validate_proxy(self, proxy: &ProviderProxy) -> Result<()> {
        proxy.validate()?;
        match self {
            Self::ChatGpt if matches!(proxy, ProviderProxy::Explicit { url, .. } if Url::parse(url).is_ok_and(|url| url.scheme() == "socks5")) => {
                Err(Error::ConfigLoad(
                    "Use socks5h:// for proxy-side DNS on ChatGPT connections.".into(),
                ))
            }
            Self::ChatGpt => Ok(()),
        }
    }
}

/// Reference to one named registration in the runtime's private subscription store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionAccount {
    channel: SubscriptionChannel,
    credential_file: PathBuf,
}

impl SubscriptionAccount {
    /// Construct a reference to a named account beneath an absolute service data directory.
    pub fn new(data_dir: &Path, channel: SubscriptionChannel, name: &str) -> Result<Self> {
        if !data_dir.is_absolute() || !valid_name(name) {
            return Err(Error::InvalidArgument(
                "use an absolute data directory and an account name of 1–48 lowercase letters, digits, hyphens, or underscores".into(),
            ));
        }
        Ok(Self {
            channel,
            credential_file: data_dir
                .join("credentials")
                .join("subscriptions")
                .join(channel.as_str())
                .join(format!("{name}.json")),
        })
    }

    /// Return this account's subscription channel.
    pub fn channel(&self) -> SubscriptionChannel {
        self.channel
    }

    /// Return the name used to select this account.
    pub fn name(&self) -> Result<&str> {
        self.validate_path()?;
        self.credential_file
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(invalid_reference)
    }

    /// Whether this account uses the identifier generated during sign-in.
    pub fn has_generated_name(&self) -> bool {
        self.name().is_ok_and(|name| {
            let mut parts = name.split('-');
            let prefix = parts.next();
            let hex = |part: Option<&str>, len| {
                part.is_some_and(|part| {
                    part.len() == len && part.bytes().all(|ch| ch.is_ascii_hexdigit())
                })
            };
            matches!(prefix, Some("email" | "account"))
                && hex(parts.next(), 16)
                && hex(parts.next(), 12)
                && parts.next().is_none()
        })
    }

    /// Return the absolute reference to this account's private file.
    pub fn credential_file(&self) -> &Path {
        &self.credential_file
    }

    /// Validate the route, authentication style, and proxy for this subscription channel.
    pub fn validate_connection(
        &self,
        format: ProviderFormat,
        base: &str,
        options: &ProviderOptions,
    ) -> Result<()> {
        self.validate_path()?;
        self.channel.validate_proxy(&options.proxy)?;
        match self.channel {
            SubscriptionChannel::ChatGpt => {
                if format != ProviderFormat::OpenAiResponses
                    || resolve_request_url(format, base, options.endpoint_url.as_deref())?
                        != CHATGPT_RESPONSES
                    || resolve_models_url(format, base, options)? != CHATGPT_MODELS
                    || options.auth == Some(ProviderAuth::ApiKey)
                    || !matches!(
                        options.profile,
                        ProviderProfile::Auto | ProviderProfile::OpenAi
                    )
                {
                    return Err(Error::ConfigLoad(
                        "ChatGPT subscriptions require the official OpenAI Responses and models endpoints with Bearer authentication".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn validate_path(&self) -> Result<()> {
        let path = &self.credential_file;
        if !path.is_absolute()
            || path.extension().and_then(|value| value.to_str()) != Some("json")
            || !path
                .file_stem()
                .and_then(|value| value.to_str())
                .is_some_and(valid_name)
            || !path.parent().is_some_and(|dir| {
                dir.ends_with(
                    Path::new("credentials")
                        .join("subscriptions")
                        .join(self.channel.as_str()),
                )
            })
        {
            return Err(invalid_reference());
        }
        Ok(())
    }
}

fn valid_name(name: &str) -> bool {
    (1..=48).contains(&name.len())
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
        && !matches!(name, "con" | "prn" | "aux" | "nul")
        && !(name.len() == 4
            && (name.starts_with("com") || name.starts_with("lpt"))
            && matches!(name.as_bytes()[3], b'1'..=b'9'))
}

fn invalid_reference() -> Error {
    Error::ConfigLoad("invalid subscription credential file reference".into())
}
