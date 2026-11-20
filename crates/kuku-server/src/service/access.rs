use std::collections::HashMap;
use std::fmt::Write as _;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use subtle::ConstantTimeEq;

use super::{failure, ServiceState};

const TICKET_LIFETIME: Duration = Duration::from_secs(60);
const MAX_PENDING_TICKETS: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Access {
    Native,
    Browser,
}

pub(super) struct BrowserAccess {
    token: String,
    tickets: Mutex<HashMap<String, Instant>>,
}

impl BrowserAccess {
    pub(super) fn new() -> io::Result<Self> {
        Ok(Self {
            token: new_token()?,
            tickets: Mutex::new(HashMap::new()),
        })
    }

    fn issue_ticket(&self) -> Result<String, TicketError> {
        let mut tickets = self.tickets.lock().unwrap();
        let now = Instant::now();
        tickets.retain(|_, expires| *expires > now);
        if tickets.len() == MAX_PENDING_TICKETS {
            return Err(TicketError::Full);
        }
        let ticket = new_token().map_err(|_| TicketError::Randomness)?;
        tickets.insert(ticket.clone(), now + TICKET_LIFETIME);
        Ok(ticket)
    }

    fn exchange_ticket(&self, ticket: &str) -> Option<&str> {
        self.tickets
            .lock()
            .unwrap()
            .remove(ticket)
            .filter(|expires| *expires > Instant::now())
            .map(|_| self.token.as_str())
    }
}

enum TicketError {
    Full,
    Randomness,
}

#[derive(Deserialize)]
pub(super) struct BrowserTicket {
    ticket: String,
}

pub(super) fn new_token() -> io::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| io::Error::other(error.to_string()))?;
    let mut token = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut token, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(token)
}

pub(super) async fn authorize(
    State(service): State<Arc<ServiceState>>,
    mut request: Request,
    next: Next,
) -> Response {
    let Some(token) = bearer(&request) else {
        return auth_required();
    };
    let access = if service
        .app
        .password
        .as_deref()
        .is_some_and(|password| matches_token(token, password))
    {
        Access::Native
    } else if service
        .browser
        .as_ref()
        .is_some_and(|browser| matches_token(token, &browser.token))
    {
        Access::Browser
    } else {
        return auth_required();
    };
    request.extensions_mut().insert(access);
    next.run(request).await
}

pub(super) async fn origin(
    State(service): State<Arc<ServiceState>>,
    request: Request,
    next: Next,
) -> Response {
    if let Some(identity) = &service.identity {
        let host = request
            .headers()
            .get(header::HOST)
            .and_then(|value| value.to_str().ok());
        if host != Some(identity.host.as_str())
            || request
                .headers()
                .get(header::ORIGIN)
                .is_some_and(|value| value != identity.origin_header)
        {
            return failure(
                StatusCode::FORBIDDEN,
                "origin_denied",
                "the managed server only accepts its loopback origin",
            );
        }
    }
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

pub(super) async fn issue(
    State(service): State<Arc<ServiceState>>,
    axum::Extension(access): axum::Extension<Access>,
) -> Response {
    if access != Access::Native {
        return native_access_required();
    }
    let Some(browser) = &service.browser else {
        return failure(
            StatusCode::NOT_FOUND,
            "browser_handoff_unavailable",
            "browser handoff requires the packaged managed server",
        );
    };
    match browser.issue_ticket() {
        Ok(ticket) => no_store(Json(json!({ "ticket": ticket })).into_response()),
        Err(TicketError::Full) => failure(
            StatusCode::TOO_MANY_REQUESTS,
            "browser_ticket_limit",
            "too many browser handoffs are pending",
        ),
        Err(TicketError::Randomness) => failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "browser_ticket_unavailable",
            "could not generate a browser ticket",
        ),
    }
}

pub(super) async fn exchange(
    State(service): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Json(body): Json<BrowserTicket>,
) -> Response {
    let Some(identity) = &service.identity else {
        return auth_required();
    };
    if headers.get(header::ORIGIN) != Some(&identity.origin_header) {
        return failure(
            StatusCode::FORBIDDEN,
            "origin_denied",
            "browser tickets must be exchanged on the managed server origin",
        );
    }
    let Some(token) = service
        .browser
        .as_ref()
        .and_then(|browser| browser.exchange_ticket(&body.ticket))
    else {
        return failure(
            StatusCode::UNAUTHORIZED,
            "invalid_browser_ticket",
            "the browser ticket has expired or was already used",
        );
    };
    no_store(Json(json!({ "token": token })).into_response())
}

pub(super) fn native_access_required() -> Response {
    failure(
        StatusCode::FORBIDDEN,
        "native_access_required",
        "this action requires the native server access token",
    )
}

fn bearer(request: &Request) -> Option<&str> {
    let value = request
        .headers()
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let (scheme, token) = value.split_once(' ')?;
    (scheme.eq_ignore_ascii_case("bearer") && !token.trim().is_empty()).then_some(token)
}

fn matches_token(provided: &str, expected: &str) -> bool {
    bool::from(provided.as_bytes().ct_eq(expected.as_bytes()))
}

fn auth_required() -> Response {
    let mut response = failure(
        StatusCode::UNAUTHORIZED,
        "auth_required",
        "a valid server access token is required",
    );
    response
        .headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
