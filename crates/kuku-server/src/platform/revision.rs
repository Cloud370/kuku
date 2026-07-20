use std::collections::BTreeMap;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::api::{ApiError, ApiErrorCode, RevisionToken};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RevisionDomain {
    Config,
    Init,
    Workspace,
    Settings,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedDigest([u8; 32]);

impl AcceptedDigest {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

pub fn accepted_digest(bytes: &[u8]) -> AcceptedDigest {
    AcceptedDigest(Sha256::digest(bytes).into())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeInputRevision(RevisionToken);

impl ProbeInputRevision {
    pub fn token(&self) -> &RevisionToken {
        &self.0
    }
}

struct RevisionInner {
    accepted: Mutex<BTreeMap<RevisionDomain, AcceptedDigest>>,
    mutation: Arc<Mutex<()>>,
}

pub struct ServerRevisionCoordinator {
    inner: Arc<RevisionInner>,
}

pub struct ServerRevisionGuard {
    coordinator: Arc<RevisionInner>,
    _mutation: OwnedMutexGuard<()>,
}

impl std::fmt::Debug for ServerRevisionGuard {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServerRevisionGuard")
            .finish_non_exhaustive()
    }
}

impl ServerRevisionCoordinator {
    pub fn open(_kuku_home: &std::path::Path) -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::new(RevisionInner {
                accepted: Mutex::new(BTreeMap::new()),
                mutation: Arc::new(Mutex::new(())),
            }),
        })
    }

    pub async fn current(&self) -> Result<RevisionToken, ApiError> {
        let accepted = self.inner.accepted.lock().await;
        Ok(token_for(&accepted))
    }

    pub async fn begin(
        self: &Arc<Self>,
        expected: &RevisionToken,
    ) -> Result<ServerRevisionGuard, ApiError> {
        let mutation = self.inner.mutation.clone().lock_owned().await;
        let accepted = self.inner.accepted.lock().await;
        let current = token_for(&accepted);
        drop(accepted);
        if &current != expected {
            return Err(stale_revision(current));
        }
        Ok(ServerRevisionGuard {
            coordinator: self.inner.clone(),
            _mutation: mutation,
        })
    }

    pub async fn register_initial(&self, domain: RevisionDomain, digest: AcceptedDigest) {
        self.inner.accepted.lock().await.insert(domain, digest);
    }

    pub async fn probe_inputs(&self) -> Result<ProbeInputRevision, ApiError> {
        let accepted = self.inner.accepted.lock().await;
        Ok(ProbeInputRevision(token_for_domains(
            &accepted,
            &[RevisionDomain::Config, RevisionDomain::Workspace],
        )))
    }
}

impl ServerRevisionGuard {
    pub async fn finish(
        self,
        domain: RevisionDomain,
        digest: AcceptedDigest,
    ) -> Result<RevisionToken, ApiError> {
        self.finish_many(vec![(domain, digest)]).await
    }

    pub async fn finish_many(
        self,
        digests: Vec<(RevisionDomain, AcceptedDigest)>,
    ) -> Result<RevisionToken, ApiError> {
        let mut accepted = self.coordinator.accepted.lock().await;
        for (domain, digest) in digests {
            accepted.insert(domain, digest);
        }
        Ok(token_for(&accepted))
    }
}

fn token_for(accepted: &BTreeMap<RevisionDomain, AcceptedDigest>) -> RevisionToken {
    token_for_domains(
        accepted,
        &[
            RevisionDomain::Config,
            RevisionDomain::Init,
            RevisionDomain::Workspace,
            RevisionDomain::Settings,
        ],
    )
}

fn token_for_domains(
    accepted: &BTreeMap<RevisionDomain, AcceptedDigest>,
    domains: &[RevisionDomain],
) -> RevisionToken {
    let mut bytes = Vec::new();
    for domain in domains {
        if let Some(digest) = accepted.get(domain) {
            bytes.push(*domain as u8);
            bytes.extend_from_slice(digest.as_bytes());
        }
    }
    let digest = accepted_digest(&bytes);
    RevisionToken::parse(
        digest
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
    )
    .expect("revision digest is always 64 lowercase hexadecimal characters")
}

fn stale_revision(current: RevisionToken) -> ApiError {
    ApiError::new(
        ApiErrorCode::StaleServerRevision,
        "server revision is stale",
        "platform-revision",
    )
    .with_details(serde_json::json!({ "server_revision": current }))
}
