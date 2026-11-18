use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use url::Url;
use wreq::header::HeaderValue;
use wreq::{Method, RequestBuilder, Response};

use crate::local::{LocalError, LocalService, SESSION_PROTOCOL_VERSION};
use crate::subscription::SubscriptionScope;
use crate::{
    Acknowledgement, AgentConfig, AgentSummary, ApiError, Command, EventPage, HistoryPage,
    NamedValue, NamedValueSource, OutputChunk, SessionSummary, Snapshot, Subscription, Target,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// A connection, protocol, service, or client persistence error.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A local managed-service operation failed.
    #[error(transparent)]
    Local(#[from] LocalError),
    /// The caller supplied an invalid connection option.
    #[error("{0}")]
    Options(String),
    /// The HTTP exchange did not provide a trustworthy result.
    #[error("service connection failed: {0}")]
    Transport(#[source] wreq::Error),
    /// The service rejected a request.
    #[error("{error_message} ({code})")]
    Service {
        /// HTTP status from the service.
        status: u16,
        /// Stable service error code.
        code: String,
        /// Diagnostic from the service.
        error_message: String,
    },
    /// The endpoint returned an HTTP failure without a valid service error envelope.
    #[error("service returned HTTP {0} without a protocol error")]
    HttpStatus(u16),
    /// The service response did not satisfy the protocol.
    #[error("invalid service response: {0}")]
    Protocol(String),
    /// A managed or explicit service uses another Session protocol revision.
    #[error("Session protocol {client} is required, but server {version} uses protocol {server}; check `kuku server status` and restart the managed service or upgrade the remote service")]
    ProtocolVersion {
        /// Protocol used by this client.
        client: u32,
        /// Protocol reported by the service.
        server: u32,
        /// Server build version.
        version: String,
    },
    /// The local filesystem, including pending-command storage, was unavailable.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The local pending command's identity is already assigned to another intent.
    #[error("pending command {0:?} already belongs to a different intent")]
    PendingConflict(String),
    /// The subscription closed before another frame could be read.
    #[error("service observation ended")]
    ObservationEnded,
}

impl Error {
    /// Return the stable code when the failure came from the service.
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Service { code, .. } => Some(code),
            Self::Local(_)
            | Self::Options(_)
            | Self::Transport(_)
            | Self::HttpStatus(_)
            | Self::Protocol(_)
            | Self::ProtocolVersion { .. }
            | Self::Io(_)
            | Self::PendingConflict(_)
            | Self::ObservationEnded => None,
        }
    }

    /// Return whether an application request received a definitive client-error rejection.
    pub fn definitively_rejected(&self) -> bool {
        matches!(
            self,
            Self::Service {
                status: 400..=499,
                ..
            }
        ) && !matches!(
            self,
            Self::Service {
                status: 408 | 429,
                ..
            }
        )
    }

    pub(crate) fn transport(error: wreq::Error) -> Self {
        Self::Transport(error.without_uri())
    }
}

/// A headless authenticated client bound to one service endpoint.
#[derive(Clone)]
pub struct Client {
    endpoint: Url,
    authorization: HeaderValue,
    http: wreq::Client,
}

/// Build and protocol versions published by the connected service.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Health {
    /// Service build identity.
    pub version: String,
    /// Session protocol version, or zero for an older service without a declared version.
    #[serde(default)]
    pub protocol_version: u32,
    /// Management protocol version, or zero if absent.
    #[serde(default)]
    pub management_version: u32,
}

impl Client {
    /// Construct an explicit connection that always uses the given remote address.
    pub fn remote(endpoint: &str, token: &str) -> Result<Self, Error> {
        let endpoint = Url::parse(endpoint)
            .map_err(|_| Error::Options("service URL must be an absolute HTTP(S) URL".into()))?;
        if !matches!(endpoint.scheme(), "http" | "https")
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(Error::Options(
                "service URL must use HTTP(S) without credentials, query, or fragment".into(),
            ));
        }
        if token.trim().is_empty() {
            return Err(Error::Options(
                "service authentication token is required".into(),
            ));
        }
        let mut authorization = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| Error::Options("invalid service authentication token".into()))?;
        authorization.set_sensitive(true);
        let http = wreq::Client::builder()
            .no_proxy()
            .redirect(wreq::redirect::Policy::none())
            .retry(wreq::retry::Policy::never())
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(Error::transport)?;
        Ok(Self {
            endpoint,
            authorization,
            http,
        })
    }

    /// Discover or launch a compatible service over an authenticated loopback connection.
    pub async fn local(service: &LocalService) -> Result<Self, Error> {
        let connection = service.connect().await?;
        Self::remote(connection.endpoint(), connection.token())
    }

    /// Return the connected endpoint without authentication material.
    pub fn endpoint(&self) -> &str {
        self.endpoint.as_str()
    }

    /// Query published versions without requiring Session protocol compatibility.
    pub async fn health(&self) -> Result<Health, Error> {
        self.get(["health"], &()).await
    }

    /// Check Session protocol compatibility before issuing data-plane commands.
    pub async fn check_protocol(&self) -> Result<(), Error> {
        let health = self.health().await?;
        if health.protocol_version != SESSION_PROTOCOL_VERSION {
            return Err(Error::ProtocolVersion {
                client: SESSION_PROTOCOL_VERSION,
                server: health.protocol_version,
                version: health.version,
            });
        }
        Ok(())
    }

    /// Resolve an authorized path on the service host.
    pub async fn workspace(&self, path: impl AsRef<Path>) -> Result<PathBuf, Error> {
        #[derive(Deserialize)]
        struct ResolvedWorkspace {
            workspace: PathBuf,
        }
        let response: ResolvedWorkspace = self
            .get(["workspaces", "resolve"], &[("workspace", path.as_ref())])
            .await?;
        Ok(response.workspace)
    }

    /// Submit an immutable command and return its committed receipt.
    pub async fn submit(&self, command: &Command) -> Result<Acknowledgement, Error> {
        self.submit_inner(command, false).await
    }

    /// Look up a receipt without issuing or replaying a command.
    pub async fn receipt(
        &self,
        target: &Target,
        command_id: &str,
    ) -> Result<Option<Acknowledgement>, Error> {
        optional(
            self.get(
                ["sessions", &target.session_id, "receipts", command_id],
                &[("workspace", &target.workspace)],
            )
            .await,
            "receipt_not_found",
        )
    }

    /// Verify an existing receipt against the exact pending command, without initiating work.
    pub async fn recover(&self, command: &Command) -> Result<Option<Acknowledgement>, Error> {
        if self
            .receipt(&command.target, &command.command_id)
            .await?
            .is_none()
        {
            return Ok(None);
        }
        optional(self.submit_inner(command, true).await, "receipt_not_found")
    }

    /// Get the selected Agent and its Session state at a common durable cursor.
    pub async fn snapshot(&self, target: &Target) -> Result<Snapshot, Error> {
        self.get(
            ["sessions", &target.session_id, "snapshot"],
            &TargetQuery::new(target),
        )
        .await
    }

    /// Get a snapshot with bounded committed execution progress for Agents in the Session.
    pub async fn observation_snapshot(&self, target: &Target) -> Result<Snapshot, Error> {
        #[derive(Serialize)]
        struct Query<'a> {
            workspace: &'a Path,
            agent: &'a str,
            progress: bool,
        }
        self.get(
            ["sessions", &target.session_id, "snapshot"],
            &Query {
                workspace: &target.workspace,
                agent: &target.agent,
                progress: true,
            },
        )
        .await
    }

    /// List Agent states in an existing Session.
    pub async fn agents(
        &self,
        workspace: &Path,
        session_id: &str,
    ) -> Result<Vec<AgentSummary>, Error> {
        #[derive(Deserialize)]
        struct Agents {
            agents: Vec<AgentSummary>,
        }
        let response: Agents = self
            .get(
                ["sessions", session_id, "agents"],
                &[("workspace", workspace)],
            )
            .await?;
        Ok(response.agents)
    }

    /// List available Sessions, optionally filtered by workspace or text.
    pub async fn sessions(
        &self,
        workspace: Option<&Path>,
        search: Option<&str>,
    ) -> Result<Vec<SessionSummary>, Error> {
        #[derive(Serialize)]
        struct Query<'a> {
            workspace: Option<&'a Path>,
            search: Option<&'a str>,
        }
        #[derive(Deserialize)]
        struct Sessions {
            sessions: Vec<SessionSummary>,
        }
        let response: Sessions = self.get(["sessions"], &Query { workspace, search }).await?;
        Ok(response.sessions)
    }

    /// Delete a Session from an authorized workspace after the caller confirms the action.
    pub async fn delete(&self, workspace: &Path, session_id: &str) -> Result<(), Error> {
        let request = self.request(Method::DELETE, ["sessions", session_id]);
        let _: Value = self
            .json(request.query(&[("workspace", workspace)]))
            .await?;
        Ok(())
    }

    /// Read a bounded ledger page, optionally filtering to an Agent within that scanned page.
    pub async fn events(
        &self,
        workspace: &Path,
        session_id: &str,
        agent: Option<&str>,
        after: u64,
        limit: usize,
    ) -> Result<EventPage, Error> {
        #[derive(Serialize)]
        struct Query<'a> {
            workspace: &'a Path,
            agent: Option<&'a str>,
            after: u64,
            limit: usize,
        }
        self.get(
            ["sessions", session_id, "events"],
            &Query {
                workspace,
                agent,
                after,
                limit,
            },
        )
        .await
    }

    /// Read a page of displayable history up to an exclusive durable cursor.
    pub async fn history(
        &self,
        target: &Target,
        before: Option<u64>,
        limit: usize,
    ) -> Result<HistoryPage, Error> {
        #[derive(Serialize)]
        struct Query<'a> {
            workspace: &'a Path,
            agent: &'a str,
            before: Option<u64>,
            limit: usize,
        }
        self.get(
            ["sessions", &target.session_id, "history"],
            &Query {
                workspace: &target.workspace,
                agent: &target.agent,
                before,
                limit,
            },
        )
        .await
    }

    /// Create a reconnectable observation scoped to one Agent and committed cursor.
    pub fn subscribe(&self, target: &Target, after: u64) -> Subscription {
        Subscription::new(self.clone(), target, SubscriptionScope::Agent, after)
    }

    /// Create a reconnectable observation across all Agents in one Session.
    pub fn subscribe_session(&self, target: &Target, after: u64) -> Subscription {
        Subscription::new(self.clone(), target, SubscriptionScope::Session, after)
    }

    /// Follow the selected Agent's full stream and bounded progress from other Agents in its Session.
    pub fn subscribe_with_agent_activity(&self, target: &Target, after: u64) -> Subscription {
        Subscription::new(
            self.clone(),
            target,
            SubscriptionScope::AgentWithActivity,
            after,
        )
    }

    /// Read a bounded byte range of retained tool output.
    pub async fn output(
        &self,
        target: &Target,
        output_id: &str,
        offset: u64,
        max_bytes: usize,
    ) -> Result<OutputChunk, Error> {
        self.search_output(target, output_id, offset, max_bytes, None)
            .await
    }

    /// Scan a bounded byte range and optionally filter retained output by literal text.
    pub async fn search_output(
        &self,
        target: &Target,
        output_id: &str,
        offset: u64,
        max_bytes: usize,
        search: Option<&str>,
    ) -> Result<OutputChunk, Error> {
        #[derive(Serialize)]
        struct Query<'a> {
            workspace: &'a Path,
            agent: &'a str,
            offset: u64,
            max_bytes: usize,
            search: Option<&'a str>,
        }
        #[derive(Deserialize)]
        struct Output {
            output: OutputChunk,
        }
        let response: Output = self
            .get(
                ["sessions", &target.session_id, "outputs", output_id],
                &Query {
                    workspace: &target.workspace,
                    agent: &target.agent,
                    offset,
                    max_bytes,
                    search,
                },
            )
            .await?;
        Ok(response.output)
    }

    /// List the names and immutable references retained by the selected Agent.
    pub async fn named_values(&self, target: &Target) -> Result<Vec<NamedValue>, Error> {
        #[derive(Deserialize)]
        struct Values {
            values: Vec<NamedValue>,
        }
        let result: Values = self
            .get(
                ["sessions", &target.session_id, "values"],
                &TargetQuery::new(target),
            )
            .await?;
        Ok(result.values)
    }

    /// Save text, data, or an existing output under a Session name.
    pub async fn save_named_value(
        &self,
        target: &Target,
        name: &str,
        source: &NamedValueSource,
        replace: bool,
        create: Option<&AgentConfig>,
    ) -> Result<NamedValue, Error> {
        #[derive(Serialize)]
        struct SaveValue<'a> {
            #[serde(flatten)]
            target: TargetQuery<'a>,
            source: &'a NamedValueSource,
            replace: bool,
            create: Option<&'a AgentConfig>,
        }
        self.json(
            self.request(
                Method::PUT,
                ["sessions", &target.session_id, "values", name],
            )
            .json(&SaveValue {
                target: TargetQuery::new(target),
                source,
                replace,
                create,
            }),
        )
        .await
    }

    /// Build a same-origin browser link for the selected workspace and conversation.
    pub fn web_url(
        &self,
        workspace: &Path,
        session: Option<&str>,
        agent: Option<&str>,
    ) -> Result<Url, Error> {
        let workspace = workspace.to_str().ok_or_else(|| {
            Error::Options("browser links require a Unicode workspace path".into())
        })?;
        let mut url = self.endpoint.clone();
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("workspace", workspace);
            if let Some(session) = session {
                query.append_pair("session", session);
            }
            if let Some(agent) = agent {
                query.append_pair("agent", agent);
            }
        }
        Ok(url)
    }

    /// Request a one-time same-origin browser ticket and put it in the URL fragment.
    pub async fn browser_url(
        &self,
        workspace: &Path,
        session: Option<&str>,
        agent: Option<&str>,
    ) -> Result<Url, Error> {
        #[derive(Deserialize)]
        struct BrowserTicket {
            ticket: String,
        }
        let mut url = self.web_url(workspace, session, agent)?;
        let response: BrowserTicket = self
            .json(self.request(Method::POST, ["service", "browser-ticket"]))
            .await?;
        let ticket = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("ticket", &response.ticket)
            .finish();
        url.set_fragment(Some(&ticket));
        Ok(url)
    }

    pub(crate) async fn get<T, Q, const N: usize>(
        &self,
        path: [&str; N],
        query: &Q,
    ) -> Result<T, Error>
    where
        T: DeserializeOwned,
        Q: Serialize + ?Sized,
    {
        self.json(self.request(Method::GET, path).query(query))
            .await
    }

    pub(crate) fn request<const N: usize>(
        &self,
        method: Method,
        path: [&str; N],
    ) -> RequestBuilder {
        let mut url = self.endpoint.clone();
        url.path_segments_mut()
            .expect("validated hierarchical service URL")
            .pop_if_empty()
            .extend(path);
        self.http
            .request(method, url.as_str())
            .header(wreq::header::AUTHORIZATION, self.authorization.clone())
    }

    async fn submit_inner(
        &self,
        command: &Command,
        replay_only: bool,
    ) -> Result<Acknowledgement, Error> {
        #[derive(Serialize)]
        struct Envelope<'a> {
            #[serde(flatten)]
            target: TargetQuery<'a>,
            command_id: &'a str,
            #[serde(flatten)]
            action: &'a crate::Action,
        }
        let mut request = self.request(
            Method::POST,
            ["sessions", &command.target.session_id, "commands"],
        );
        if replay_only {
            request = request.query(&[("replay_only", true)]);
        }
        let acknowledgement: Acknowledgement = self
            .json(request.json(&Envelope {
                target: TargetQuery::new(&command.target),
                command_id: &command.command_id,
                action: &command.action,
            }))
            .await?;
        if acknowledgement.receipt.command_id != command.command_id
            || acknowledgement.receipt.agent != command.target.agent
            || (replay_only && !acknowledgement.replayed)
        {
            return Err(Error::Protocol(
                "receipt does not identify the submitted command".into(),
            ));
        }
        Ok(acknowledgement)
    }

    async fn json<T: DeserializeOwned>(&self, request: RequestBuilder) -> Result<T, Error> {
        let response = request
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(Error::transport)?;
        let status = response.status().as_u16();
        let bytes = checked(response)
            .await?
            .bytes()
            .await
            .map_err(Error::transport)?;
        if let Ok(envelope) = serde_json::from_slice::<ErrorEnvelope>(&bytes) {
            if !envelope.ok {
                return Err(service_error(status, envelope.error));
            }
        }
        serde_json::from_slice(&bytes).map_err(|error| Error::Protocol(error.to_string()))
    }
}

#[derive(Serialize)]
struct TargetQuery<'a> {
    workspace: &'a Path,
    agent: &'a str,
}

impl<'a> TargetQuery<'a> {
    fn new(target: &'a Target) -> Self {
        Self {
            workspace: &target.workspace,
            agent: &target.agent,
        }
    }
}

#[derive(Deserialize)]
struct ErrorEnvelope {
    ok: bool,
    #[serde(flatten)]
    error: ApiError,
}

pub(crate) async fn checked(response: Response) -> Result<Response, Error> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    Err(match response.json::<ApiError>().await {
        Ok(error) => service_error(status.as_u16(), error),
        Err(_) => Error::HttpStatus(status.as_u16()),
    })
}

fn service_error(status: u16, error: ApiError) -> Error {
    Error::Service {
        status,
        code: error.code,
        error_message: error.message,
    }
}

fn optional<T>(result: Result<T, Error>, missing_code: &str) -> Result<Option<T>, Error> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.code() == Some(missing_code) => Ok(None),
        Err(error) => Err(error),
    }
}
