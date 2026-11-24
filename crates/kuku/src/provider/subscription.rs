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
            let object = body.as_object_mut().expect("request is an object");
            for field in [
                "max_output_tokens",
                "temperature",
                "previous_response_id",
                "prompt_cache_retention",
            ] {
                object.remove(field);
            }
            body["stream"] = json!(true);
            body["store"] = json!(false);
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
            .filter(|email| !email.chars().any(char::is_control)),
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
mod tests {
    use serde_json::json;

    use super::{apply_request_shape, SubscriptionChannel};

    #[test]
    fn responses_use_stateless_streaming_and_match_namespaced_tool_calls() {
        let mut body = json!({
            "model": "fixture",
            "input": [
                {"type":"message","role":"developer","content":"Instructions"},
                {"type":"function_call","name":"read","call_id":"call_1","arguments":"{}"},
                {"type":"function_call_output","call_id":"call_1","output":"fixture"}
            ],
            "tools": [{"type":"function","name":"read","parameters":{"type":"object"}}],
            "max_output_tokens": 64,
            "previous_response_id": "old"
        });
        apply_request_shape(SubscriptionChannel::ChatGpt, &mut body);
        assert_eq!(true, body["stream"]);
        assert_eq!(false, body["store"]);
        assert!(body.get("max_output_tokens").is_none());
        assert!(body.get("previous_response_id").is_none());
        assert_eq!("namespace", body["tools"][0]["type"]);
        assert_eq!("kuku", body["tools"][0]["name"]);
        assert_eq!("read", body["tools"][0]["tools"][0]["name"]);
        assert_eq!("kuku", body["input"][1]["namespace"]);
        assert_eq!("call_1", body["input"][2]["call_id"]);
    }
}
