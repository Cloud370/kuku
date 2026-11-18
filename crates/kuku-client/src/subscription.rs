use std::path::Path;
use std::pin::Pin;
use std::time::Duration;

use bytes::Bytes;
use futures_core::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::time::{sleep_until, Instant};
use tokio_stream::StreamExt;
use wreq::Method;

use crate::http::checked;
use crate::{AgentSettlement, Client, Error, HistoryEntry, Target};

const READ_TIMEOUT: Duration = Duration::from_secs(45);
const RETRY_DELAYS_MS: [u64; 4] = [100, 250, 500, 1000];
const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

/// A committed change to input delivery for the observed Agent.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum InputDelivery {
    /// Inputs admitted together at an execution boundary.
    Accepted {
        /// Original command identities in admission order.
        input_ids: Vec<String>,
    },
    /// A previously queued input that was withdrawn.
    Withdrawn {
        /// Original command identity of the withdrawn input.
        input_id: String,
    },
    /// One queued input became withdrawn and another took its place.
    Replaced {
        /// Original command identity of the withdrawn input.
        input_id: String,
        /// Command identity of the new input in the queue.
        replacement_id: String,
    },
}

/// A typed observation frame, carrying either a durable fact or transient preview.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Observation {
    /// A committed Session event advancing the observation cursor.
    Committed {
        /// Stable durable event cursor.
        id: u64,
        /// Agent address associated with the fact, when known.
        agent: Option<String>,
        /// Committed event kind.
        kind: String,
        /// Optional displayable history entry for the selected target.
        entry: Option<Box<HistoryEntry>>,
        /// Whether the fact affects a client's Session or Agent state.
        #[serde(default)]
        state_changed: bool,
        /// Turn of the committed fact, when applicable.
        turn: Option<u64>,
        /// Provider request associated with the fact, when applicable.
        request_id: Option<String>,
        /// Tool invocation associated with a durable dispatch, when available.
        tool_call_id: Option<String>,
        /// Operation code for a runtime-state fact.
        operation: Option<String>,
        /// Delivery outcome for the selected Agent, when present.
        delivery: Option<InputDelivery>,
        /// Durable status and output returned for a settled turn.
        settlement: Option<Box<AgentSettlement>>,
        /// Whether a committed stop still had owned work to clean up.
        cleanup_pending: Option<bool>,
    },
    /// Transient text, tool progress or another runtime signal.
    Live {
        /// Agent that produced the preview.
        agent: String,
        /// Execution identity that emitted the preview.
        execution_id: u64,
        /// Last durable Session cursor seen at the preview's source.
        #[serde(default)]
        cursor: u64,
        /// Turn that emitted the preview, when available.
        turn: Option<u64>,
        /// One-based display number of that Agent's Turn, when known.
        turn_number: Option<u64>,
        /// The existing SDK progress event in wire form.
        event: Value,
    },
    /// Transient frames were dropped and should be refreshed from durable history.
    Lagged {
        /// Number of missed transient frames, or zero after a reconnect with an unknown count.
        skipped: u64,
    },
    /// An idle connection is still healthy.
    Heartbeat,
}

/// An observation or an observed change to the subscription's HTTP connection.
#[derive(Debug, Clone, PartialEq)]
pub enum SubscriptionUpdate {
    /// The service accepted the observation stream.
    Connected,
    /// A retryable failure interrupted the stream or an attempt to open it.
    Reconnecting(String),
    /// A frame delivered by the subscription.
    Observation(Observation),
}

/// A bounded observation reader that resumes from its last delivered committed event.
pub struct Subscription {
    client: Client,
    target: Target,
    scope: SubscriptionScope,
    cursor: u64,
    reader: Option<Reader>,
    reconnect: bool,
    failures: usize,
    retry_at: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubscriptionScope {
    Agent,
    Session,
    AgentWithActivity,
}

impl Subscription {
    /// Return the last delivered committed Session event.
    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    /// Return the target this subscription was originally bound to.
    pub fn target(&self) -> &Target {
        &self.target
    }

    /// Read the next nonduplicate observation, reconnecting with bounded backoff when disconnected.
    pub async fn next(&mut self) -> Result<Observation, Error> {
        loop {
            if let SubscriptionUpdate::Observation(frame) = self.next_update().await? {
                return Ok(frame);
            }
        }
    }

    /// Read observations along with actual attachment and reconnect attempts for interactive feedback.
    pub async fn next_update(&mut self) -> Result<SubscriptionUpdate, Error> {
        loop {
            if self.reader.is_none() {
                if let Some(at) = self.retry_at {
                    sleep_until(at).await;
                    self.retry_at = None;
                }
                match self.connect().await {
                    Ok(reader) => {
                        self.reader = Some(reader);
                        return Ok(SubscriptionUpdate::Connected);
                    }
                    Err(error) => {
                        if let Some(update) = self.retry(error, self.failures == 0)? {
                            return Ok(update);
                        }
                        continue;
                    }
                }
            }
            if std::mem::take(&mut self.reconnect) {
                return Ok(SubscriptionUpdate::Observation(Observation::Lagged {
                    skipped: 0,
                }));
            }
            match self
                .reader
                .as_mut()
                .expect("reader was initialized")
                .next()
                .await
            {
                Ok(Observation::Heartbeat) => {
                    self.failures = 0;
                }
                Ok(Observation::Committed { id, .. }) if id <= self.cursor => {}
                Ok(Observation::Live { ref agent, .. })
                    if self.scope == SubscriptionScope::Agent && self.target.agent != *agent => {}
                Ok(frame) => {
                    self.failures = 0;
                    if let Observation::Committed { id, .. } = &frame {
                        self.cursor = *id;
                    }
                    return Ok(SubscriptionUpdate::Observation(frame));
                }
                Err(error) => {
                    self.reader = None;
                    self.reconnect = true;
                    if let Some(update) = self.retry(error, true)? {
                        return Ok(update);
                    }
                }
            }
        }
    }

    pub(crate) fn new(
        client: Client,
        target: &Target,
        scope: SubscriptionScope,
        after: u64,
    ) -> Self {
        Self {
            client,
            target: target.clone(),
            scope,
            cursor: after,
            reader: None,
            reconnect: false,
            failures: 0,
            retry_at: None,
        }
    }

    fn retry(&mut self, error: Error, report: bool) -> Result<Option<SubscriptionUpdate>, Error> {
        let Some(delay) = RETRY_DELAYS_MS
            .get(self.failures)
            .filter(|_| retryable(&error))
        else {
            return Err(error);
        };
        self.failures += 1;
        self.retry_at = Some(Instant::now() + Duration::from_millis(*delay));
        Ok(report.then(|| SubscriptionUpdate::Reconnecting(error.to_string())))
    }

    async fn connect(&mut self) -> Result<Reader, Error> {
        #[derive(Serialize)]
        struct Query<'a> {
            workspace: &'a Path,
            agent: Option<&'a str>,
            after: u64,
            include_agent_activity: Option<bool>,
        }
        let response = self
            .client
            .request(
                Method::GET,
                ["sessions", &self.target.session_id, "observe"],
            )
            .query(&Query {
                workspace: &self.target.workspace,
                agent: (self.scope != SubscriptionScope::Session)
                    .then_some(self.target.agent.as_str()),
                after: self.cursor,
                include_agent_activity: (self.scope == SubscriptionScope::AgentWithActivity)
                    .then_some(true),
            })
            .read_timeout(READ_TIMEOUT)
            .send()
            .await
            .map_err(Error::transport)?;
        let response = checked(response).await?;
        let kind = response
            .headers()
            .get(wreq::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok());
        if !kind.is_some_and(|kind| kind.split(';').next() == Some("application/x-ndjson")) {
            return Err(Error::Protocol(
                "observation stream must use application/x-ndjson".into(),
            ));
        }
        Ok(Reader {
            stream: Box::pin(response.bytes_stream()),
            remaining: Bytes::new(),
            line: Vec::new(),
        })
    }
}

type BodyStream = Pin<Box<dyn Stream<Item = wreq::Result<Bytes>> + Send>>;

struct Reader {
    stream: BodyStream,
    remaining: Bytes,
    line: Vec<u8>,
}

impl Reader {
    async fn next(&mut self) -> Result<Observation, Error> {
        loop {
            if let Some(newline) = self.remaining.iter().position(|byte| *byte == b'\n') {
                let fragment = self.remaining.split_to(newline + 1);
                self.extend(&fragment[..newline])?;
                let parsed = if self.line.iter().all(u8::is_ascii_whitespace) {
                    None
                } else {
                    Some(serde_json::from_slice(&self.line))
                };
                self.line.clear();
                if let Some(parsed) = parsed {
                    return parsed.map_err(|error| Error::Protocol(error.to_string()));
                }
            } else {
                let fragment = std::mem::take(&mut self.remaining);
                self.extend(&fragment)?;
                self.remaining = self
                    .stream
                    .next()
                    .await
                    .transpose()
                    .map_err(Error::transport)?
                    .ok_or(Error::ObservationEnded)?;
            }
        }
    }

    fn extend(&mut self, fragment: &[u8]) -> Result<(), Error> {
        if self.line.len() + fragment.len() > MAX_LINE_BYTES {
            return Err(Error::Protocol(
                "observation frame exceeds the supported size".into(),
            ));
        }
        self.line.extend_from_slice(fragment);
        Ok(())
    }
}

fn retryable(error: &Error) -> bool {
    matches!(
        error,
        Error::Transport(_)
            | Error::ObservationEnded
            | Error::HttpStatus(502..=504)
            | Error::Service {
                status: 502..=504,
                ..
            }
    )
}
