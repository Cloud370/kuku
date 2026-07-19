use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use axum::http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, REFERRER_POLICY, VARY, WWW_AUTHENTICATE,
};
use axum::http::HeaderMap;
use kuku_server::api::{ApiErrorCode, AuthMode};
use kuku_server::platform::{
    AuthPolicy, BearerTokenSource, BearerTokenStore, OriginPolicy, SecurityHeaders,
};

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[test]
fn generated_token_is_private_stable_and_redacted() {
    let home = tempfile::tempdir().unwrap();

    let first = BearerTokenStore::open(home.path(), None).unwrap();
    let second = BearerTokenStore::open(home.path(), None).unwrap();

    assert_eq!(64, first.expose_for_terminal().len());
    assert_eq!(first.expose_for_terminal(), second.expose_for_terminal());
    assert_eq!(BearerTokenSource::Generated, first.source());
    assert_eq!(BearerTokenSource::Persisted, second.source());
    assert!(!format!("{first:?}").contains(first.expose_for_terminal()));
    assert!(!format!("{first}").contains(first.expose_for_terminal()));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mode = std::fs::metadata(home.path().join("auth/token"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(0o600, mode);
    }
}

#[test]
fn explicit_token_is_read_without_copying_it_into_home() {
    let home = tempfile::tempdir().unwrap();
    let token_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(token_file.path(), format!("{TOKEN}\n")).unwrap();

    let store = BearerTokenStore::open(home.path(), Some(token_file.path())).unwrap();

    assert_eq!(TOKEN, store.expose_for_terminal());
    assert_eq!(BearerTokenSource::ExplicitFile, store.source());
    assert!(!home.path().join("auth/token").exists());
}

#[test]
fn malformed_persisted_token_is_rejected_without_leaking_it() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("auth")).unwrap();
    std::fs::write(home.path().join("auth/token"), "not-a-valid-secret").unwrap();

    let error = BearerTokenStore::open(home.path(), None).unwrap_err();

    assert_eq!(ApiErrorCode::Internal, error.code());
    assert!(!format!("{error:?}").contains("not-a-valid-secret"));
}

#[test]
fn bearer_auth_is_required_unless_direct_loopback_is_explicitly_trusted() {
    let home = tempfile::tempdir().unwrap();
    let token_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(token_file.path(), TOKEN).unwrap();
    let store = BearerTokenStore::open(home.path(), Some(token_file.path())).unwrap();
    let loopback = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 17777);
    let remote = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 4)), 17777);

    let trusted = store
        .authorize(
            &AuthPolicy {
                loopback_trust: true,
            },
            loopback,
            None,
        )
        .unwrap();
    assert_eq!(AuthMode::LoopbackTrusted, trusted.mode);
    assert!(trusted.authenticated);

    for (policy, peer) in [
        (
            AuthPolicy {
                loopback_trust: false,
            },
            loopback,
        ),
        (
            AuthPolicy {
                loopback_trust: true,
            },
            remote,
        ),
    ] {
        assert_eq!(
            ApiErrorCode::AuthRequired,
            store.authorize(&policy, peer, None).unwrap_err().code()
        );
    }

    for malformed in ["Basic abc", "Bearer", "Bearer wrong", "Bearer  wrong"] {
        assert_eq!(
            ApiErrorCode::AuthRequired,
            store
                .authorize(
                    &AuthPolicy {
                        loopback_trust: true,
                    },
                    loopback,
                    Some(malformed),
                )
                .unwrap_err()
                .code()
        );
    }

    let bearer = store
        .authorize(
            &AuthPolicy {
                loopback_trust: false,
            },
            remote,
            Some(&format!("Bearer {TOKEN}")),
        )
        .unwrap();
    assert_eq!(AuthMode::Bearer, bearer.mode);
    assert!(store.status(&bearer).authenticated);
}

#[test]
fn origin_policy_accepts_only_exact_http_origins() {
    let policy = OriginPolicy::new(vec![
        "http://kuku.local:17777".to_owned(),
        "https://kuku.example".to_owned(),
    ])
    .unwrap();

    assert_eq!(
        Some("http://kuku.local:17777"),
        policy.check(Some("http://kuku.local:17777")).unwrap()
    );
    assert_eq!(None, policy.check(None).unwrap());
    assert_eq!(
        ApiErrorCode::OriginNotAllowed,
        policy
            .check(Some("http://attacker.invalid"))
            .unwrap_err()
            .code()
    );

    for invalid in [
        "*",
        "http://example.test/path",
        "http://example.test?query",
        "http://example.test#fragment",
        "file:///tmp/index.html",
    ] {
        assert!(OriginPolicy::new(vec![invalid.to_owned()]).is_err());
    }
}

#[test]
fn security_headers_are_restrictive_and_never_enable_wildcard_cors() {
    let mut headers = HeaderMap::new();
    SecurityHeaders::apply(&mut headers, &["http://localhost:5173".to_owned()]);
    SecurityHeaders::apply_auth_challenge(&mut headers);

    assert_eq!("no-store", headers[CACHE_CONTROL]);
    assert_eq!("Origin", headers[VARY]);
    assert_eq!("no-referrer", headers[REFERRER_POLICY]);
    assert_eq!("Bearer", headers[WWW_AUTHENTICATE]);
    let csp = headers[CONTENT_SECURITY_POLICY].to_str().unwrap();
    assert!(csp.contains("default-src 'self'"));
    assert!(csp.contains("connect-src 'self' http://localhost:5173"));
    assert!(csp.contains("object-src 'none'"));
    assert!(csp.contains("frame-ancestors 'none'"));
    assert!(!headers.contains_key("access-control-allow-origin"));
    assert!(!csp.contains('*'));
}
