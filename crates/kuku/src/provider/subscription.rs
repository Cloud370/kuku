mod chatgpt;
mod store;

use serde_json::{json, Value};
use uuid::Uuid;

use crate::config::{ProviderProxy, SubscriptionAccount, SubscriptionChannel};
use crate::{Error, Result};

use self::store::{now, AccountRecord, AccountStore};

pub use self::chatgpt::{SubscriptionLogin, SubscriptionLoginProgress, SubscriptionLoginResult};

const REFRESH_WINDOW_SECS: i64 = 30;

/// Whether an account is registered and can authorize subscription requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionStatus {
    /// The registration has a renewable subscription session.
    SignedIn,
    /// Sign-in succeeded without subscription inference permission.
    PlanDisabled,
    /// Sign in or reauthorize before using this account.
    SignedOut,
}

/// Non-secret account information for local settings and the CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionAccountInfo {
    /// Masked email derived from the authorized identity, when available.
    pub masked_email: Option<String>,
    /// State of the selected local registration.
    pub status: SubscriptionStatus,
}

/// Read a local account's state without refreshing or making a network request.
pub fn subscription_account_info(account: &SubscriptionAccount) -> Result<SubscriptionAccountInfo> {
    let record = AccountStore::new(account)?.read()?;
    Ok(account_info(record.as_ref()))
}

/// Display a human-readable account label using an optional alias and masked email.
pub fn subscription_display_label(
    account: &SubscriptionAccount,
    info: Option<&SubscriptionAccountInfo>,
) -> String {
    let channel = match account.channel() {
        crate::config::SubscriptionChannel::ChatGpt => "ChatGPT",
    };
    let mut label = channel.to_owned();
    if let Some(alias) = account
        .name()
        .ok()
        .filter(|_| !account.has_generated_name())
    {
        label.push_str(" · ");
        label.push_str(alias);
    }
    if let Some(email) = info
        .and_then(|info| info.masked_email.as_deref())
        .filter(|email| is_masked_email(email))
    {
        label.push_str(" · ");
        label.push_str(email);
    }
    label
}

fn is_email_suffix(suffix: &str) -> bool {
    (2..=24).contains(&suffix.len()) && suffix.bytes().all(|ch| ch.is_ascii_alphabetic())
}

fn is_masked_email(email: &str) -> bool {
    let masked_part = |part: &str, suffix_length| {
        part == "****"
            || part.split_once("***").is_some_and(|(beginning, ending)| {
                beginning.len() <= 3
                    && ending.len() <= suffix_length
                    && beginning
                        .bytes()
                        .chain(ending.bytes())
                        .all(|ch| ch.is_ascii_alphanumeric())
            })
    };
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    let domain_mask = if let Some((name, suffix)) = domain.split_once('.') {
        is_email_suffix(suffix) && masked_part(name, 0)
    } else {
        masked_part(domain, 0)
    };
    masked_part(local, 1) && domain_mask
}

/// Clear the selected account's local tokens and report whether remote revocation was confirmed.
pub async fn sign_out_subscription(
    account: &SubscriptionAccount,
    proxy: &ProviderProxy,
) -> Result<bool> {
    let store = AccountStore::new(account)?;
    let _lock = store.lock().await?;
    let Some(mut record) = store.read()? else {
        return Ok(true);
    };
    let Some(tokens) = record.tokens.take() else {
        return Ok(true);
    };
    let revoked = match (account.channel(), tokens.refresh.as_deref()) {
        (SubscriptionChannel::ChatGpt, Some(refresh_token)) => {
            chatgpt::revoke(proxy, &record.client_id, refresh_token).await
        }
        (_, None) => tokens.access.is_none(),
    };
    record.revision = Uuid::new_v4().to_string();
    store.write(&record)?;
    Ok(revoked)
}

pub(crate) async fn authorization_token(
    account: &SubscriptionAccount,
    proxy: &ProviderProxy,
) -> Result<String> {
    account.channel().validate_proxy(proxy)?;
    let store = AccountStore::new(account)?;
    let record = store.read()?.ok_or_else(|| sign_in_required(account))?;
    if let Some(access) = current_access(&record) {
        return Ok(access.to_owned());
    }
    let _lock = store.lock().await?;
    let mut record = store.read()?.ok_or_else(|| sign_in_required(account))?;
    if let Some(access) = current_access(&record) {
        return Ok(access.to_owned());
    }
    let tokens = record
        .tokens
        .as_ref()
        .ok_or_else(|| sign_in_required(account))?;
    if !chatgpt::plan_enabled(&tokens.scope) {
        return Err(Error::InvalidArgument(format!(
            "authorize ChatGPT plan usage for '{}'",
            account.name()?
        )));
    }
    let refresh_token = tokens
        .refresh
        .as_deref()
        .ok_or_else(|| sign_in_required(account))?;
    match account.channel() {
        SubscriptionChannel::ChatGpt => {
            match chatgpt::refresh(proxy, &record.client_id, refresh_token, &tokens.scope).await {
                Ok(tokens) => record.tokens = Some(tokens),
                Err(chatgpt::TokenFailure::InvalidGrant | chatgpt::TokenFailure::Expired) => {
                    record.tokens = None;
                    record.revision = Uuid::new_v4().to_string();
                    store.write(&record)?;
                    return Err(sign_in_required(account));
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
    store.write(&record)?;
    current_access(&record)
        .map(str::to_owned)
        .ok_or_else(|| sign_in_required(account))
}

pub(super) fn apply_request_shape(channel: SubscriptionChannel, body: &mut Value) {
    match channel {
        SubscriptionChannel::ChatGpt => {
            body.as_object_mut()
                .expect("request is an object")
                .remove("temperature");
            if let Some(Value::Array(tools)) = body.get_mut("tools").map(Value::take) {
                body["tools"] = json!([{
                    "type": "namespace",
                    "name": "kuku",
                    "description": "Tools provided by kuku.",
                    "tools": tools,
                }]);
            }
            if let Some(input) = body["input"].as_array_mut() {
                for item in input {
                    if item["type"] == "function_call" && item["namespace"].is_null() {
                        item["namespace"] = json!("kuku");
                    }
                }
            }
        }
    }
}

fn account_info(record: Option<&AccountRecord>) -> SubscriptionAccountInfo {
    let status = match record.and_then(|record| record.tokens.as_ref()) {
        Some(tokens) if chatgpt::plan_enabled(&tokens.scope) => {
            if tokens.refresh.is_some() || tokens.expires_at > now() {
                SubscriptionStatus::SignedIn
            } else {
                SubscriptionStatus::SignedOut
            }
        }
        Some(_) => SubscriptionStatus::PlanDisabled,
        None => SubscriptionStatus::SignedOut,
    };
    SubscriptionAccountInfo {
        masked_email: record
            .and_then(|record| record.masked_email.clone())
            .filter(|email| is_masked_email(email)),
        status,
    }
}

fn current_access(record: &AccountRecord) -> Option<&str> {
    let tokens = record.tokens.as_ref()?;
    let margin = if tokens.refresh.is_some() {
        REFRESH_WINDOW_SECS
    } else {
        0
    };
    (chatgpt::plan_enabled(&tokens.scope) && tokens.expires_at > now().saturating_add(margin))
        .then_some(tokens.access.as_deref())
        .flatten()
}

fn sign_in_required(account: &SubscriptionAccount) -> Error {
    Error::InvalidArgument(format!(
        "sign in again with `kuku auth login {} {}`",
        account.channel().as_str(),
        account.name().unwrap_or("ACCOUNT")
    ))
}

#[cfg(test)]
mod display_tests {
    use crate::config::{SubscriptionAccount, SubscriptionChannel};

    use super::{subscription_display_label, SubscriptionAccountInfo, SubscriptionStatus};

    #[test]
    fn label_uses_the_masked_email_and_never_exposes_automatic_names() {
        let directory = std::env::temp_dir();
        let account = SubscriptionAccount::new(
            &directory,
            SubscriptionChannel::ChatGpt,
            "email-0123456789abcdef-abcdef123456",
        )
        .unwrap();
        let mut info = SubscriptionAccountInfo {
            masked_email: Some("a***@e***".into()),
            status: SubscriptionStatus::SignedIn,
        };
        assert_eq!(
            subscription_display_label(&account, Some(&info)),
            "ChatGPT · a***@e***"
        );
        let alias =
            SubscriptionAccount::new(&directory, SubscriptionChannel::ChatGpt, "work").unwrap();
        assert_eq!(
            subscription_display_label(&alias, Some(&info)),
            "ChatGPT · work · a***@e***"
        );
        info.masked_email = Some("ali***e@exa***.com".into());
        assert_eq!(
            subscription_display_label(&alias, Some(&info)),
            "ChatGPT · work · ali***e@exa***.com"
        );
        info.masked_email = Some("actual-address@example.com".into());
        assert_eq!(subscription_display_label(&account, Some(&info)), "ChatGPT");
    }
}
#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{apply_request_shape, SubscriptionChannel};

    #[test]
    fn request_shape_removes_unsupported_generated_fields_and_namespaces_tools() {
        let mut body = json!({
            "model": "fixture",
            "input": [
                {"type":"message","role":"developer","content":"Instructions"},
                {"type":"function_call","name":"read","call_id":"call_1","arguments":"{}"},
                {"type":"function_call_output","call_id":"call_1","output":"fixture"}
            ],
            "tools": [{"type":"function","name":"read","parameters":{"type":"object"}}],
            "temperature": 1
        });
        apply_request_shape(SubscriptionChannel::ChatGpt, &mut body);
        assert!(body.get("temperature").is_none());
        assert_eq!("namespace", body["tools"][0]["type"]);
        assert_eq!("kuku", body["tools"][0]["name"]);
        assert_eq!("read", body["tools"][0]["tools"][0]["name"]);
        assert_eq!("kuku", body["input"][1]["namespace"]);
        assert_eq!("call_1", body["input"][2]["call_id"]);
    }
}
