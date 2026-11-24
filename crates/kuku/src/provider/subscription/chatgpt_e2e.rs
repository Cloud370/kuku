use std::collections::BTreeMap;
use std::pin::Pin;
use std::time::Duration;

use boring2::asn1::Asn1Time;
use boring2::hash::MessageDigest;
use boring2::pkey::PKey;
use boring2::rsa::Rsa;
use boring2::ssl::{Ssl, SslAcceptor, SslMethod};
use boring2::x509::extension::{BasicConstraints, SubjectAlternativeName};
use boring2::x509::{X509, X509Name};
use jsonwebtoken::encode;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_boring2::SslStream;
use wreq::tls::CertStore;

use crate::adapter::{
    ModelTrial, SubscriptionStatus, discover_models, sign_out_subscription, trial_model,
};
use crate::config::{ApiKey, ProviderConfig, ProviderFormat, ProviderOptions, ProviderProxy};
use crate::provider::http_client::TEST_TRUST;

use super::{
    ISSUER, SubscriptionChannel, SubscriptionLogin, SubscriptionLoginProgress, now, test_signer,
};

#[tokio::test]
async fn subscription_login_refresh_catalog_tools_and_revocation_share_the_proxy() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let (tls, trust) = proxy_tls();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = ProviderProxy::Explicit {
            url: format!("http://{}", listener.local_addr().unwrap()),
            auth: None,
        };
        let directory = tempfile::tempdir().unwrap();
        TEST_TRUST
            .scope(trust, async {
                let attempt = SubscriptionLogin::start(
                    directory.path(),
                    SubscriptionChannel::ChatGpt,
                    None,
                    proxy.clone(),
                )
                .await
                .unwrap();
                let mut callback = attempt.browser.redirect.clone();
                callback.query_pairs_mut().extend_pairs([
                    ("code", "rejected-fixture-code"),
                    ("state", attempt.browser.state.as_str()),
                    ("client_id", "oaiapp_fixture"),
                ]);
                let previous_state = attempt.browser.state.clone();
                let previous_nonce = attempt.browser.nonce.clone();
                let previous_verifier = attempt.browser.verifier.clone();
                let upstream = FixtureProxy { listener, tls };
                let reject = async {
                    let (mut stream, _, bytes) = upstream
                        .request("auth.openai.com", "POST /api/accounts/oauth/token")
                        .await;
                    let form = url::form_urlencoded::parse(&bytes).collect::<BTreeMap<_, _>>();
                    assert_eq!("oaiapp_fixture", form["client_id"]);
                    assert_eq!("rejected-fixture-code", form["code"]);
                    reply_with_status(
                        &mut stream,
                        "400 Bad Request",
                        "application/json",
                        r#"{"error":"invalid_grant"}"#,
                    )
                    .await;
                };
                let (outcome, ()) =
                    tokio::join!(attempt.complete_or_retry(callback.as_str()), reject);
                let SubscriptionLoginProgress::Retry(attempt) = outcome.unwrap() else {
                    panic!("expected a new authorization link")
                };
                let retry_url = url::Url::parse(attempt.authorization_url()).unwrap();
                let query = retry_url.query_pairs().collect::<BTreeMap<_, _>>();
                assert_eq!("oaiapp_fixture", query["client_id"]);
                assert!(!query.contains_key("agent_name_hint"));
                assert_ne!(previous_state, attempt.browser.state);
                assert_ne!(previous_nonce, attempt.browser.nonce);
                assert_ne!(previous_verifier, attempt.browser.verifier);
                let mut callback = attempt.browser.redirect.clone();
                callback.query_pairs_mut().extend_pairs([
                    ("code", "fixture-code"),
                    ("state", attempt.browser.state.as_str()),
                ]);
                let (id_token, keys) = identity(&attempt.browser.nonce);
                let verifier = attempt.browser.verifier.clone();
                let uri = attempt.browser.redirect.to_string();
                let provider =
                    tokio::spawn(
                        async move { upstream.serve(&verifier, &uri, &id_token, keys).await },
                    );
                let login = attempt.complete(callback.as_str()).await.unwrap();
                assert_eq!(SubscriptionStatus::SignedIn, login.info.status);
                assert_eq!(
                    Some("an***a@exa***.invalid"),
                    login.info.masked_email.as_deref()
                );
                assert!(login.account.name().unwrap().starts_with("email-"));
                let before = std::fs::read_to_string(login.account.credential_file()).unwrap();
                assert!(!before.contains("anna@example.invalid"));

                let connection = ProviderConfig {
                    format: ProviderFormat::OpenAiResponses,
                    base_url: "https://api.openai.com/v1".into(),
                    api_key: ApiKey::Plaintext(String::new()),
                    options: ProviderOptions {
                        proxy: proxy.clone(),
                        subscription: Some(login.account.clone()),
                        ..ProviderOptions::default()
                    },
                };
                let (catalog, trial) = tokio::join!(
                    discover_models(&connection),
                    trial_model(
                        &connection,
                        "fixture-model",
                        ModelTrial::Tools {
                            service_tier: Some("fast".into())
                        }
                    )
                );
                let catalog = catalog.unwrap();
                assert_eq!("fixture-model", catalog[0].id);
                assert_eq!(Some("Fixture Model"), catalog[0].name.as_deref());
                assert_eq!(
                    Some(vec!["fast".into(), "ultrafast".into()]),
                    catalog[0].capabilities.service_tiers
                );
                let trial = trial.unwrap();
                assert_eq!(Some(true), trial.tool_round_trip);
                assert!(trial.usage.is_some());
                assert!(trial.continuation_usage.is_some());
                let faster = trial_model(
                    &connection,
                    "fixture-model",
                    ModelTrial::Text {
                        effort: None,
                        service_tier: Some("ultrafast".into()),
                    },
                )
                .await
                .unwrap();
                let report = faster.provider_report.as_ref().unwrap();
                assert_eq!(Some("ultrafast"), report.service_tier.as_deref());
                let after = std::fs::read_to_string(login.account.credential_file()).unwrap();
                assert!(after.contains("rotated-refresh") && !after.contains("first-refresh"));
                assert!(sign_out_subscription(&login.account, &proxy).await.unwrap());
                let signed_out: Value = serde_json::from_slice(
                    &std::fs::read(login.account.credential_file()).unwrap(),
                )
                .unwrap();
                assert!(signed_out["tokens"].is_null());
                assert_eq!("oaiapp_fixture", signed_out["client_id"]);
                provider.await.unwrap();
            })
            .await;
    })
    .await
    .unwrap();
}

struct FixtureProxy {
    listener: TcpListener,
    tls: SslAcceptor,
}

impl FixtureProxy {
    async fn serve(&self, verifier: &str, redirect: &str, id_token: &str, keys: Value) {
        let (mut stream, headers, body) = self
            .request("auth.openai.com", "POST /api/accounts/oauth/token")
            .await;
        assert!(!headers.contains_key("authorization"));
        let form = url::form_urlencoded::parse(&body)
            .into_owned()
            .collect::<BTreeMap<_, _>>();
        assert_eq!("fixture-code", form["code"]);
        assert_eq!(verifier, form["code_verifier"]);
        assert_eq!(redirect, form["redirect_uri"]);
        assert_eq!("oaiapp_fixture", form["client_id"]);
        reply_json(
            &mut stream,
            json!({
                "id_token": id_token, "token_type": "Bearer",
                "access_token": "first-access", "refresh_token": "first-refresh", "expires_in": 1,
                "scope": "offline_access chatgpt.tokens.use.direct"
            }),
        )
        .await;

        let (mut stream, _, body) = self
            .request("auth.openai.com", "GET /.well-known/jwks.json")
            .await;
        assert!(body.is_empty());
        reply_json(&mut stream, keys).await;

        let (mut stream, _, body) = self
            .request("auth.openai.com", "POST /api/accounts/oauth/token")
            .await;
        let form = url::form_urlencoded::parse(&body)
            .into_owned()
            .collect::<BTreeMap<_, _>>();
        assert_eq!("refresh_token", form["grant_type"]);
        assert_eq!("first-refresh", form["refresh_token"]);
        reply_json(&mut stream, json!({
            "token_type": "Bearer", "access_token": "rotated-access", "refresh_token": "rotated-refresh",
            "expires_in": 3600
        })).await;

        let mut dispatch_ids = std::collections::BTreeSet::new();
        for _ in 0..2 {
            let (mut stream, line, headers, bytes) = self.exchange("api.openai.com").await;
            assert_eq!("Bearer rotated-access", headers["authorization"]);
            if line.starts_with("GET /v1/models ") {
                reply_json(
                    &mut stream,
                    json!({"models":[
                    {"slug":"fixture-model","display_name":"Fixture Model","visibility":"list","service_tiers":["fast","ultrafast"]},
                        {"slug":"hidden","visibility":"hidden"}
                    ]}),
                )
                .await;
            } else {
                assert!(line.starts_with("POST /v1/responses "));
                assert!(dispatch_ids.insert(headers["x-client-request-id"].clone()));
                assert!(!headers.contains_key("x-session-id"));
                let body: Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(true, body["stream"]);
                assert_eq!(false, body["store"]);
                assert_eq!("fast", body["service_tier"]);
                assert_eq!("kuku", body["tools"][0]["name"]);
                assert!(body.get("max_output_tokens").is_none());
                assert!(body.get("prompt_cache_key").is_none());
                assert!(body.get("include").is_none());
                reply_sse(
                    &mut stream,
                    json!({"output":[{
                        "type":"function_call","namespace":"kuku","name":"kuku_setup_check",
                        "call_id":"fixture-call","arguments":"{}"
                    }],"usage":{"input_tokens":18,"output_tokens":6,"total_tokens":24}}),
                )
                .await;
            }
        }
        let (mut stream, headers, bytes) =
            self.request("api.openai.com", "POST /v1/responses").await;
        assert_eq!("Bearer rotated-access", headers["authorization"]);
        assert!(dispatch_ids.insert(headers["x-client-request-id"].clone()));
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!("fast", body["service_tier"]);
        let result = &body["input"][2];
        assert_eq!("fixture-call", result["call_id"]);
        assert_eq!("function_call_output", result["type"]);
        reply_sse(&mut stream, json!({
            "output":[{"type":"message","content":[{"type":"output_text","text":result["output"]}]}],
            "usage":{"input_tokens":25,"output_tokens":4,"total_tokens":29}
        })).await;

        let (mut stream, headers, bytes) =
            self.request("api.openai.com", "POST /v1/responses").await;
        assert_eq!("Bearer rotated-access", headers["authorization"]);
        assert!(dispatch_ids.insert(headers["x-client-request-id"].clone()));
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!("ultrafast", body["service_tier"]);
        reply_sse(
            &mut stream,
            json!({
                "output":[{"type":"message","content":[{"type":"output_text","text":"377"}]}],
                "service_tier":"ultrafast",
                "usage":{"input_tokens":22,"output_tokens":3,"total_tokens":25}
            }),
        )
        .await;

        let (mut stream, _, _) = self
            .request("auth.openai.com", "GET /.well-known/openid-configuration")
            .await;
        reply_json(
            &mut stream,
            json!({
                "issuer": ISSUER, "revocation_endpoint": "https://auth.openai.com/oauth/revoke"
            }),
        )
        .await;
        let (mut stream, _, body) = self.request("auth.openai.com", "POST /oauth/revoke").await;
        let form = url::form_urlencoded::parse(&body)
            .into_owned()
            .collect::<BTreeMap<_, _>>();
        assert_eq!("rotated-refresh", form["token"]);
        reply(&mut stream, "text/plain", "").await;
    }

    async fn request(
        &self,
        host: &str,
        method_and_path: &str,
    ) -> (
        SslStream<tokio::net::TcpStream>,
        BTreeMap<String, String>,
        Vec<u8>,
    ) {
        let (stream, line, headers, bytes) = self.exchange(host).await;
        assert!(line.starts_with(&format!("{method_and_path} ")));
        (stream, headers, bytes)
    }

    async fn exchange(
        &self,
        host: &str,
    ) -> (
        SslStream<tokio::net::TcpStream>,
        String,
        BTreeMap<String, String>,
        Vec<u8>,
    ) {
        let (mut socket, _) = self.listener.accept().await.unwrap();
        assert!(
            request_header(&mut socket)
                .await
                .starts_with(&format!("CONNECT {host}:443 "))
        );
        socket
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await
            .unwrap();
        let ssl = Ssl::new(self.tls.context()).unwrap();
        let mut stream = SslStream::new(ssl, socket).unwrap();
        Pin::new(&mut stream).accept().await.unwrap();
        let header = request_header(&mut stream).await;
        let mut lines = header.lines();
        let request = lines.next().unwrap().to_owned();
        let fields = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(key, value)| (key.to_ascii_lowercase(), value.trim().to_owned()))
            .collect::<BTreeMap<_, _>>();
        let length = fields
            .get("content-length")
            .map_or(0, |length| length.parse().unwrap());
        let mut body = vec![0; length];
        stream.read_exact(&mut body).await.unwrap();
        (stream, request, fields, body)
    }
}

async fn request_header(stream: &mut (impl AsyncRead + Unpin)) -> String {
    let mut bytes = Vec::new();
    while bytes.len() < 16 * 1024 {
        bytes.push(stream.read_u8().await.unwrap());
        if bytes.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8(bytes).unwrap()
}

async fn reply(stream: &mut SslStream<tokio::net::TcpStream>, content_type: &str, body: &str) {
    reply_with_status(stream, "200 OK", content_type, body).await;
}

async fn reply_with_status(
    stream: &mut SslStream<tokio::net::TcpStream>,
    status: &str,
    content_type: &str,
    body: &str,
) {
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).await.unwrap();
    stream.write_all(body.as_bytes()).await.unwrap();
    stream.shutdown().await.unwrap();
}

async fn reply_json(stream: &mut SslStream<tokio::net::TcpStream>, body: Value) {
    reply(stream, "application/json", &body.to_string()).await;
}

async fn reply_sse(stream: &mut SslStream<tokio::net::TcpStream>, body: Value) {
    let event = json!({"type":"response.completed","response":body});
    reply(
        stream,
        "text/event-stream",
        &format!("event: response.completed\ndata: {event}\n\n"),
    )
    .await;
}

fn identity(nonce: &str) -> (String, Value) {
    let (key, jwk, header) = test_signer();
    let token = encode(
        &header,
        &json!({
            "iss":ISSUER, "aud":"oaiapp_fixture", "sub":"fixture-user", "nonce":nonce,
            "exp":now()+3600, "email":"anna@example.invalid"
        }),
        &key,
    )
    .unwrap();
    (token, json!({"keys":[jwk]}))
}

fn proxy_tls() -> (SslAcceptor, CertStore) {
    let key = PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap();
    let mut name = X509Name::builder().unwrap();
    name.append_entry_by_text("CN", "Subscription acceptance fixture")
        .unwrap();
    let name = name.build();
    let mut cert = X509::builder().unwrap();
    cert.set_version(2).unwrap();
    cert.set_subject_name(&name).unwrap();
    cert.set_issuer_name(&name).unwrap();
    cert.set_not_before(&Asn1Time::days_from_now(0).unwrap())
        .unwrap();
    cert.set_not_after(&Asn1Time::days_from_now(1).unwrap())
        .unwrap();
    cert.set_pubkey(&key).unwrap();
    cert.append_extension(&BasicConstraints::new().critical().ca().build().unwrap())
        .unwrap();
    let alternative = SubjectAlternativeName::new()
        .dns("api.openai.com")
        .dns("auth.openai.com")
        .build(&cert.x509v3_context(None, None))
        .unwrap();
    cert.append_extension(&alternative).unwrap();
    cert.sign(&key, MessageDigest::sha256()).unwrap();
    let cert = cert.build();
    let der = cert.to_der().unwrap();
    let trust = CertStore::builder()
        .add_der_cert(der.as_slice())
        .build()
        .unwrap();
    let mut acceptor = SslAcceptor::mozilla_intermediate(SslMethod::tls_server()).unwrap();
    acceptor.set_private_key(&key).unwrap();
    acceptor.set_certificate(&cert).unwrap();
    (acceptor.build(), trust)
}
