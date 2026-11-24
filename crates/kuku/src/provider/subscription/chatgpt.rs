use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, Mac};
use jsonwebtoken::{decode, decode_header, jwk::JwkSet, Algorithm, DecodingKey, Validation};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use url::Url;
use uuid::Uuid;
use wreq::{Client, Response, StatusCode};

use crate::config::subscription::CHATGPT_RESOURCE;
use crate::config::{ProviderProxy, SubscriptionAccount, SubscriptionChannel};
use crate::{Error, Result};

use super::store::{now, AccountRecord, AccountStore, HostRegistration, Tokens};
use super::{account_info, is_email_suffix, SubscriptionAccountInfo};

const ISSUER: &str = "https://auth.openai.com";
const AUTH_ENDPOINT: &str = "https://auth.openai.com/api/accounts/authorize";
const TOKEN_ENDPOINT: &str = "https://auth.openai.com/api/accounts/oauth/token";
const JWKS_ENDPOINT: &str = "https://auth.openai.com/.well-known/jwks.json";
const DISCOVERY_ENDPOINT: &str = "https://auth.openai.com/.well-known/openid-configuration";
const DYNAMIC_CLIENT: &str = "dynamic_agent_client";
const USER_SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const CALLBACK_PATH: &str = "/auth/callback";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_AUTH_BYTES: usize = 64 * 1024;

/// A short-lived authorization attempt tied to one account and one manual callback.
pub struct SubscriptionLogin {
    data_dir: PathBuf,
    channel: SubscriptionChannel,
    account: Option<SubscriptionAccount>,
    host: HostRegistration,
    proxy: ProviderProxy,
    pending: PendingLogin,
    browser: BrowserAuthorization,
}

/// The result of exchanging the browser callback.
pub enum SubscriptionLoginProgress {
    /// The verified account and its current sign-in state.
    Completed(SubscriptionLoginResult),
    /// OpenAI rejected the code and the same registration needs a new browser authorization.
    Retry(Box<SubscriptionLogin>),
}

/// The account selected or created by a successful browser authorization.
pub struct SubscriptionLoginResult {
    /// The account that can be attached to a model connection.
    pub account: SubscriptionAccount,
    /// Privacy-preserving identity and local sign-in status.
    pub info: SubscriptionAccountInfo,
}

impl SubscriptionLogin {
    /// Prepare a new registration or reauthorize an explicitly selected account.
    pub async fn start(
        data_dir: &Path,
        channel: SubscriptionChannel,
        name: Option<&str>,
        proxy: ProviderProxy,
    ) -> Result<Self> {
        let _ = proxy_client(&proxy)?;
        let directory_reference =
            SubscriptionAccount::new(data_dir, channel, name.unwrap_or("account"))?;
        let store = AccountStore::new(&directory_reference)?;
        let prior = if name.is_some() { store.read()? } else { None };
        let host = store.host_registration().await?;
        let pending = PendingLogin::from_record(prior);
        let browser = BrowserAuthorization::new(&host, &pending)?;
        Ok(Self {
            data_dir: data_dir.to_path_buf(),
            channel,
            account: name.map(|_| directory_reference),
            host,
            proxy,
            pending,
            browser,
        })
    }

    /// Return the complete browser authorization link for this attempt.
    pub fn authorization_url(&self) -> &str {
        self.browser.authorization_url.as_str()
    }

    /// Accept the complete pasted callback, validate the grant, and publish the new token set.
    pub async fn complete(self, pasted: &str) -> Result<SubscriptionLoginResult> {
        match self.complete_or_retry(pasted).await? {
            SubscriptionLoginProgress::Completed(login) => Ok(login),
            SubscriptionLoginProgress::Retry(_) => Err(TokenFailure::InvalidGrant.into()),
        }
    }

    /// Exchange the callback, preparing a fresh link for the same registration if OpenAI rejects the code.
    pub async fn complete_or_retry(mut self, pasted: &str) -> Result<SubscriptionLoginProgress> {
        if self.browser.started.elapsed() >= LOGIN_TIMEOUT {
            return Err(Error::InvalidArgument(
                "ChatGPT sign-in expired; start a new attempt".into(),
            ));
        }
        let callback = parse_callback(
            pasted.trim(),
            &self.browser.redirect,
            &self.browser.state,
            self.pending.client_id.as_deref(),
        )?;
        let client = proxy_client(&self.proxy)?;
        let exchange = token_request(
            &client,
            &[
                ("grant_type", "authorization_code"),
                ("code", &callback.code),
                ("code_verifier", &self.browser.verifier),
                ("client_id", &callback.client_id),
                ("redirect_uri", self.browser.redirect.as_str()),
                ("resource", CHATGPT_RESOURCE),
            ],
        )
        .await;
        let reply = match exchange {
            Ok(reply) => reply,
            Err(TokenFailure::InvalidGrant) => {
                self.pending.client_id = Some(callback.client_id);
                self.browser = BrowserAuthorization::new(&self.host, &self.pending)?;
                return Ok(SubscriptionLoginProgress::Retry(Box::new(self)));
            }
            Err(error) => return Err(error.into()),
        };
        let id_token = reply
            .id_token
            .as_deref()
            .ok_or(TokenFailure::InvalidResponse)?;
        let identity =
            verified_identity(&client, id_token, &callback.client_id, &self.browser.nonce).await?;
        if self
            .pending
            .subject
            .as_deref()
            .is_some_and(|subject| identity.sub != subject)
        {
            return Err(Error::InvalidArgument(
                "OpenAI authorized a different account; use a different name to add it".into(),
            ));
        }
        let email = identity
            .email
            .as_deref()
            .and_then(|value| private_email(&self.host.email_key, value));
        let account = if let Some(account) = self.account {
            account
        } else {
            let name = automatic_name(
                &self.host.email_key,
                email.as_ref(),
                &identity.sub,
                &callback.client_id,
            );
            SubscriptionAccount::new(&self.data_dir, self.channel, &name)?
        };
        let store = AccountStore::new(&account)?;
        let _lock = store.lock().await?;
        let current = store.read()?;
        if current.as_ref().map(|record| record.revision.as_str())
            != self.pending.revision.as_deref()
        {
            return Err(Error::InvalidArgument(
                "subscription account changed; start a new sign-in".into(),
            ));
        }
        let record = AccountRecord {
            channel: self.channel,
            revision: Uuid::new_v4().to_string(),
            client_id: callback.client_id,
            subject: identity.sub,
            masked_email: email.as_ref().map(|email| email.masked.clone()),
            email_identifier: email.map(|email| email.identifier),
            tokens: Some(reply.into_tokens(None)?),
        };
        store.write(&record)?;
        Ok(SubscriptionLoginProgress::Completed(
            SubscriptionLoginResult {
                account,
                info: account_info(Some(&record)),
            },
        ))
    }
}

struct BrowserAuthorization {
    authorization_url: Url,
    redirect: Url,
    state: String,
    nonce: String,
    verifier: String,
    started: Instant,
}

impl BrowserAuthorization {
    fn new(host: &HostRegistration, pending: &PendingLogin) -> Result<Self> {
        let verifier = random_secret()?;
        let state = random_secret()?;
        let nonce = random_secret()?;
        let redirect = redirect_url()?;
        let mut authorization_url =
            Url::parse(AUTH_ENDPOINT).expect("fixed authorization endpoint");
        let client_id = pending.client_id.as_deref().unwrap_or(DYNAMIC_CLIENT);
        authorization_url.query_pairs_mut().extend_pairs([
            ("client_id", client_id),
            ("ext_agent_host_id", &host.id),
            ("response_type", "code"),
            ("redirect_uri", redirect.as_str()),
            ("scope", USER_SCOPES),
            ("resource", CHATGPT_RESOURCE),
            ("state", &state),
            ("nonce", &nonce),
            ("code_challenge_method", "S256"),
            (
                "code_challenge",
                &URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())),
            ),
        ]);
        if pending.client_id.is_none() {
            authorization_url
                .query_pairs_mut()
                .append_pair("agent_name_hint", "kuku");
        }
        if pending.prompt_consent {
            authorization_url
                .query_pairs_mut()
                .append_pair("prompt", "consent");
        }
        Ok(Self {
            authorization_url,
            redirect,
            state,
            nonce,
            verifier,
            started: Instant::now(),
        })
    }
}

struct PendingLogin {
    revision: Option<String>,
    client_id: Option<String>,
    subject: Option<String>,
    prompt_consent: bool,
}

impl PendingLogin {
    fn from_record(record: Option<AccountRecord>) -> Self {
        match record {
            Some(record) => Self {
                revision: Some(record.revision),
                client_id: Some(record.client_id),
                subject: Some(record.subject),
                prompt_consent: record
                    .tokens
                    .as_ref()
                    .is_some_and(|tokens| !plan_enabled(&tokens.scope)),
            },
            None => Self {
                revision: None,
                client_id: None,
                subject: None,
                prompt_consent: false,
            },
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(super) enum TokenFailure {
    #[error("could not contact OpenAI using the selected network settings")]
    Transport,
    #[error("OpenAI returned an invalid sign-in response")]
    InvalidResponse,
    #[error("OpenAI rejected the authorization code (invalid_grant)")]
    InvalidGrant,
    #[error("OpenAI rejected the refresh token; sign in again")]
    Expired,
    #[error("OpenAI rejected the account's client registration")]
    InvalidClient,
    #[error("OpenAI authorization request failed (HTTP {0})")]
    Http(StatusCode),
}

impl From<TokenFailure> for Error {
    fn from(error: TokenFailure) -> Self {
        Self::InvalidArgument(error.to_string())
    }
}

#[derive(Deserialize)]
struct TokenReply {
    id_token: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
    token_type: Option<String>,
    scope: Option<String>,
    expires_in: Option<i64>,
}

impl TokenReply {
    fn into_tokens(
        self,
        previous_scope: Option<&str>,
    ) -> std::result::Result<Tokens, TokenFailure> {
        let scope = self
            .scope
            .or_else(|| previous_scope.map(str::to_owned))
            .unwrap_or_default();
        let access = self.access_token.filter(|value| !value.is_empty());
        let refresh = self.refresh_token.filter(|value| !value.is_empty());
        let lifetime = self.expires_in.unwrap_or(0);
        if plan_enabled(&scope)
            && (access.is_none()
                || self
                    .token_type
                    .as_deref()
                    .is_none_or(|value| !value.eq_ignore_ascii_case("bearer"))
                || lifetime <= 0
                || (previous_scope.is_some() && refresh.is_none()))
        {
            return Err(TokenFailure::InvalidResponse);
        }
        Ok(Tokens {
            access,
            refresh,
            scope,
            expires_at: now().saturating_add(lifetime),
        })
    }
}

#[derive(Deserialize)]
struct TokenErrorBody {
    error: String,
}

#[derive(Deserialize)]
struct Identity {
    sub: String,
    nonce: String,
    email: Option<String>,
}

struct Callback {
    code: String,
    client_id: String,
}

struct PrivateEmail {
    masked: String,
    identifier: String,
}

fn private_email(key: &[u8; 32], email: &str) -> Option<PrivateEmail> {
    let (local, domain) = email.trim().rsplit_once('@')?;
    if local.is_empty() || domain.is_empty() || email.chars().any(char::is_control) {
        return None;
    }
    let local_length = local.chars().count();
    let ending = local
        .chars()
        .last()
        .filter(|ch| local_length >= 4 && ch.is_ascii_alphanumeric());
    let beginning = masked_prefix(local, usize::from(ending.is_some()));

    let (domain_name, top_level) = domain
        .rsplit_once('.')
        .filter(|(name, suffix)| !name.is_empty() && is_email_suffix(suffix))
        .unwrap_or((domain, ""));
    let domain_prefix = masked_prefix(domain_name, 0);
    let mut masked = format!("{beginning}***");
    masked.extend(ending);
    masked.push_str(&format!("@{domain_prefix}***"));
    if !top_level.is_empty() {
        masked.push('.');
        masked.push_str(top_level);
    }

    let identifier =
        private_identifier(key, b"email", &format!("{local}@{}", domain.to_lowercase()));
    Some(PrivateEmail { masked, identifier })
}

fn masked_prefix(part: &str, visible_ending: usize) -> String {
    let length = part
        .chars()
        .count()
        .saturating_sub(1 + visible_ending)
        .min(3);
    part.chars()
        .take(length)
        .take_while(char::is_ascii_alphanumeric)
        .collect()
}

fn automatic_name(
    key: &[u8; 32],
    email: Option<&PrivateEmail>,
    subject: &str,
    client_id: &str,
) -> String {
    let (prefix, person) = match email {
        Some(email) => ("email", email.identifier.clone()),
        None => ("account", private_identifier(key, b"subject", subject)),
    };
    let registration = private_identifier(key, b"registration", client_id);
    format!("{prefix}-{}-{}", &person[..16], &registration[..12])
}

fn private_identifier(key: &[u8; 32], kind: &[u8], value: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key size");
    mac.update(kind);
    mac.update(b"\0");
    mac.update(value.as_bytes());
    format!("{:x}", mac.finalize().into_bytes())
}

pub(super) fn plan_enabled(scope: &str) -> bool {
    scope
        .split_ascii_whitespace()
        .any(|scope| scope == "chatgpt.tokens.use.direct")
}

pub(super) async fn refresh(
    proxy: &ProviderProxy,
    client_id: &str,
    refresh_token: &str,
    scope: &str,
) -> std::result::Result<Tokens, TokenFailure> {
    let client = proxy_client(proxy).map_err(|_| TokenFailure::Transport)?;
    token_request(
        &client,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", client_id),
            ("refresh_token", refresh_token),
            ("resource", CHATGPT_RESOURCE),
        ],
    )
    .await?
    .into_tokens(Some(scope))
}

pub(super) async fn revoke(proxy: &ProviderProxy, client_id: &str, refresh_token: &str) -> bool {
    #[derive(Deserialize)]
    struct Discovery {
        issuer: String,
        revocation_endpoint: String,
    }
    let Ok(client) = proxy_client(proxy) else {
        return false;
    };
    let Ok(discovery) = get_json::<Discovery>(&client, DISCOVERY_ENDPOINT).await else {
        return false;
    };
    let Ok(url) = Url::parse(&discovery.revocation_endpoint) else {
        return false;
    };
    if discovery.issuer != ISSUER
        || url.origin() != Url::parse(ISSUER).unwrap().origin()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return false;
    }
    let form = [
        ("token", refresh_token),
        ("token_type_hint", "refresh_token"),
        ("client_id", client_id),
    ];
    for delay_ms in [0, 200, 500] {
        if delay_ms != 0 {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }
        match send_form(&client, url.as_str(), &form).await {
            Ok(response) if response.status() == StatusCode::OK => return true,
            Ok(response) if !response.status().is_server_error() => return false,
            _ => {}
        }
    }
    false
}

fn parse_callback(
    pasted: &str,
    redirect: &Url,
    state: &str,
    prior_client_id: Option<&str>,
) -> Result<Callback> {
    if pasted.len() > MAX_AUTH_BYTES {
        return Err(callback_error());
    }
    let url = Url::parse(pasted).map_err(|_| callback_error())?;
    if url.origin() != redirect.origin()
        || url.path() != redirect.path()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(callback_error());
    }
    let mut params = BTreeMap::new();
    for (name, value) in url.query_pairs() {
        if params.insert(name, value).is_some() {
            return Err(callback_error());
        }
    }
    if params.get("state").map(|value| value.as_ref()) != Some(state) {
        return Err(Error::InvalidArgument(
            "callback does not match this sign-in attempt".into(),
        ));
    }
    if params.contains_key("error") {
        return Err(Error::InvalidArgument(
            "ChatGPT authorization did not complete".into(),
        ));
    }
    let code = params
        .remove("code")
        .filter(|code| !code.is_empty())
        .ok_or_else(callback_error)?;
    let issued = params.remove("client_id");
    let client_id = match (prior_client_id, issued.as_deref()) {
        (Some(saved), None) => saved,
        (Some(saved), Some(issued)) if saved == issued => saved,
        (None, Some(issued)) if valid_client_id(issued) => issued,
        _ => {
            return Err(Error::InvalidArgument(
                "callback client registration does not match this sign-in".into(),
            ))
        }
    };
    Ok(Callback {
        code: code.into_owned(),
        client_id: client_id.to_owned(),
    })
}

fn valid_client_id(value: &str) -> bool {
    value.strip_prefix("oaiapp_").is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix.len() <= 256
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    })
}

fn callback_error() -> Error {
    Error::InvalidArgument("paste the complete callback URL from the browser address bar".into())
}

fn redirect_url() -> Result<Url> {
    let mut bytes = [0; 2];
    random_bytes(&mut bytes)?;
    let port = 49152 + (u16::from_le_bytes(bytes) & 16383);
    Ok(
        Url::parse(&format!("http://127.0.0.1:{port}{CALLBACK_PATH}"))
            .expect("fixed loopback address"),
    )
}

fn random_secret() -> Result<String> {
    let mut bytes = [0; 32];
    random_bytes(&mut bytes)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn random_bytes(bytes: &mut [u8]) -> Result<()> {
    getrandom::fill(bytes)
        .map_err(|_| Error::InvalidArgument("OS randomness is unavailable".into()))
}

fn proxy_client(proxy: &ProviderProxy) -> Result<Client> {
    SubscriptionChannel::ChatGpt.validate_proxy(proxy)?;
    crate::provider::http_client::api_client(proxy)
}

async fn verified_identity(
    client: &Client,
    id_token: &str,
    client_id: &str,
    nonce: &str,
) -> std::result::Result<Identity, TokenFailure> {
    let jwks = get_json::<JwkSet>(client, JWKS_ENDPOINT).await?;
    verify_identity(&jwks, id_token, client_id, nonce)
}

fn verify_identity(
    jwks: &JwkSet,
    id_token: &str,
    client_id: &str,
    nonce: &str,
) -> std::result::Result<Identity, TokenFailure> {
    let header = decode_header(id_token).map_err(|_| TokenFailure::InvalidResponse)?;
    if matches!(
        header.alg,
        Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512
    ) {
        return Err(TokenFailure::InvalidResponse);
    }
    let kid = header.kid.as_deref().ok_or(TokenFailure::InvalidResponse)?;
    let jwk = jwks.find(kid).ok_or(TokenFailure::InvalidResponse)?;
    let key = DecodingKey::from_jwk(jwk).map_err(|_| TokenFailure::InvalidResponse)?;
    let mut validation = Validation::new(header.alg);
    validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
    validation.set_issuer(&[ISSUER]);
    validation.set_audience(&[client_id]);
    validation.leeway = 5;
    let data = decode::<Identity>(id_token, &key, &validation)
        .map_err(|_| TokenFailure::InvalidResponse)?;
    if data.claims.sub.is_empty() || data.claims.nonce != nonce {
        return Err(TokenFailure::InvalidResponse);
    }
    Ok(data.claims)
}

async fn get_json<T: DeserializeOwned>(
    client: &Client,
    url: &str,
) -> std::result::Result<T, TokenFailure> {
    let response = client
        .get(url)
        .redirect(wreq::redirect::Policy::none())
        .timeout(HTTP_TIMEOUT)
        .send()
        .await
        .map_err(|_| TokenFailure::Transport)?;
    if !response.status().is_success() {
        return Err(TokenFailure::Http(response.status()));
    }
    read_json(response).await
}

async fn token_request(
    client: &Client,
    fields: &[(&str, &str)],
) -> std::result::Result<TokenReply, TokenFailure> {
    let response = send_form(client, TOKEN_ENDPOINT, fields).await?;
    let status = response.status();
    if status.is_success() {
        return read_json(response).await;
    }
    let body = read_json::<TokenErrorBody>(response).await.ok();
    match body.as_ref().map(|body| body.error.as_str()) {
        Some("invalid_grant") => Err(TokenFailure::InvalidGrant),
        Some(
            "invalid_refresh_token"
            | "token_expired"
            | "refresh_token_expired"
            | "refresh_token_invalidated"
            | "refresh_token_reused",
        ) => Err(TokenFailure::Expired),
        Some("invalid_client") => Err(TokenFailure::InvalidClient),
        _ => Err(TokenFailure::Http(status)),
    }
}

async fn send_form(
    client: &Client,
    url: &str,
    fields: &[(&str, &str)],
) -> std::result::Result<Response, TokenFailure> {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields)
        .finish();
    client
        .post(url)
        .redirect(wreq::redirect::Policy::none())
        .timeout(HTTP_TIMEOUT)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|_| TokenFailure::Transport)
}

async fn read_json<T: DeserializeOwned>(
    mut response: Response,
) -> std::result::Result<T, TokenFailure> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| TokenFailure::Transport)?
    {
        if body.len().saturating_add(chunk.len()) > MAX_AUTH_BYTES {
            return Err(TokenFailure::InvalidResponse);
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| TokenFailure::InvalidResponse)
}

#[cfg(test)]
#[path = "chatgpt_e2e.rs"]
mod end_to_end;

#[cfg(test)]
fn test_signer() -> (
    jsonwebtoken::EncodingKey,
    jsonwebtoken::jwk::Jwk,
    jsonwebtoken::Header,
) {
    let rsa = boring2::rsa::Rsa::generate(2048).unwrap();
    let key = jsonwebtoken::EncodingKey::from_rsa_der(&rsa.private_key_to_der().unwrap());
    let mut jwk = jsonwebtoken::jwk::Jwk::from_encoding_key(&key, Algorithm::RS256).unwrap();
    jwk.common.key_id = Some("fixture-key".into());
    let mut header = jsonwebtoken::Header::new(Algorithm::RS256);
    header.kid.clone_from(&jwk.common.key_id);
    (key, jwk, header)
}

#[cfg(test)]
mod tests {
    use jsonwebtoken::{encode, jwk::JwkSet};
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::{
        automatic_name, now, parse_callback, private_email, test_signer, verify_identity,
        SubscriptionLogin, ISSUER,
    };
    use crate::config::{ProviderProxy, SubscriptionAccount, SubscriptionChannel};

    #[test]
    fn manual_callback_matches_the_original_attempt_and_client() {
        let redirect = url::Url::parse("http://127.0.0.1:52914/auth/callback").unwrap();
        let callback =
            format!("{redirect}?code=secret%2Bcode&state=pending&client_id=oaiapp_sample");
        let accepted = parse_callback(&callback, &redirect, "pending", None).unwrap();
        assert_eq!("secret+code", accepted.code);
        assert_eq!("oaiapp_sample", accepted.client_id);
        let callback_without_client = format!("{redirect}?code=secret%2Bcode&state=pending");
        assert!(parse_callback(&callback_without_client, &redirect, "pending", None).is_err());
        let returning = parse_callback(
            &callback_without_client,
            &redirect,
            "pending",
            Some("oaiapp_sample"),
        )
        .unwrap();
        assert_eq!("oaiapp_sample", returning.client_id);
        assert!(parse_callback(&callback, &redirect, "pending", Some("oaiapp_other")).is_err());
        assert!(parse_callback(&callback, &redirect, "other", None).is_err());
        assert!(parse_callback(
            &format!("{callback}&state=pending"),
            &redirect,
            "pending",
            None
        )
        .is_err());
        let different_port = callback.replace("52914", "52915");
        assert!(parse_callback(&different_port, &redirect, "pending", None).is_err());
        let denied = format!("{redirect}?error=access_denied&state=pending");
        assert!(parse_callback(&denied, &redirect, "pending", None).is_err());
    }

    #[test]
    fn email_identifiers_are_private_and_distinguish_registrations() {
        let email = "Ada.Lovelace@Example.invalid";
        let key = [7; 32];
        let private = private_email(&key, email).unwrap();
        let same = private_email(&key, "Ada.Lovelace@example.invalid").unwrap();
        assert_eq!("Ada***e@Exa***.invalid", private.masked);
        assert_eq!("***@***.io", private_email(&key, "a@x.io").unwrap().masked);
        assert_eq!(
            "ab***@lo***",
            private_email(&key, "abc@loc").unwrap().masked
        );
        assert_eq!(private.identifier, same.identifier);
        assert_ne!(
            private.identifier,
            private_email(&[9; 32], email).unwrap().identifier
        );
        let work = automatic_name(&key, Some(&private), "subject", "oaiapp_work");
        let personal = automatic_name(&key, Some(&private), "subject", "oaiapp_personal");
        assert_ne!(work, personal);
        assert!(!work.contains("example") && !work.contains("Lovelace"));
        assert!(private_email(&key, "bad\x1b@example.invalid").is_none());
    }

    #[test]
    fn identity_must_be_signed_for_the_issued_client_and_pending_nonce() {
        let (key, jwk, header) = test_signer();
        let keys = JwkSet { keys: vec![jwk] };
        let claims = json!({
            "iss": ISSUER,
            "aud": ["oaiapp_account"],
            "sub": "verified-account",
            "nonce": "pending-nonce",
            "email": "verified@example.invalid",
            "exp": now() + 3600,
        });
        let token = encode(&header, &claims, &key).unwrap();
        let identity = verify_identity(&keys, &token, "oaiapp_account", "pending-nonce").unwrap();
        assert_eq!("verified-account", identity.sub);
        assert!(verify_identity(&keys, &token, "oaiapp_other", "pending-nonce").is_err());
        assert!(verify_identity(&keys, &token, "oaiapp_account", "other-nonce").is_err());
        let (_, last) = token.rsplit_once('.').unwrap();
        let tampered = format!("{}{}", &token[..token.len() - last.len()], "invalid");
        assert!(verify_identity(&keys, &tampered, "oaiapp_account", "pending-nonce").is_err());
    }

    #[tokio::test]
    async fn manual_sign_in_contacts_only_the_configured_proxy() {
        let root = tempfile::tempdir().unwrap();
        for mode in [ProviderProxy::Environment, ProviderProxy::Direct] {
            assert!(SubscriptionLogin::start(
                root.path(),
                SubscriptionChannel::ChatGpt,
                None,
                mode
            )
            .await
            .is_ok());
        }
        assert!(SubscriptionLogin::start(
            root.path(),
            SubscriptionChannel::ChatGpt,
            None,
            ProviderProxy::Explicit {
                url: "socks5://127.0.0.1:1080".into(),
                auth: None
            }
        )
        .await
        .is_err());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let proxy = ProviderProxy::Explicit {
            url: format!("http://{address}"),
            auth: None,
        };
        let attempt = SubscriptionLogin::start(
            root.path(),
            SubscriptionChannel::ChatGpt,
            Some("work"),
            proxy,
        )
        .await
        .unwrap();
        let url = url::Url::parse(attempt.authorization_url()).unwrap();
        assert_eq!(Some("auth.openai.com"), url.host_str());
        assert!(url
            .query_pairs()
            .all(|(key, _)| !matches!(key.as_ref(), "id_token_hint" | "login_hint")));
        let mut redirect = attempt.browser.redirect.clone();
        redirect.query_pairs_mut().extend_pairs([
            ("state", attempt.browser.state.as_str()),
            ("code", "fake-private-code"),
            ("client_id", "oaiapp_fixture"),
        ]);
        let intercepted = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                request.push(stream.read_u8().await.unwrap());
                if request.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            stream
                .write_all(
                    b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            String::from_utf8(request).unwrap()
        });
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            attempt.complete(redirect.as_str()),
        )
        .await
        .unwrap();
        let Err(error) = result else {
            panic!("the failing proxy must stop sign-in")
        };
        assert!(error.to_string().contains("selected network settings"));
        let captured = intercepted.await.unwrap();
        assert!(captured.starts_with("CONNECT auth.openai.com:443 "));
        assert!(!captured.contains("fake-private-code"));
        let account =
            SubscriptionAccount::new(root.path(), SubscriptionChannel::ChatGpt, "work").unwrap();
        assert!(!account.credential_file().exists());
    }
}
