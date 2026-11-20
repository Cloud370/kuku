use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use kuku::config::ProviderFormat;
use kuku::event::EventPayload;
use kuku_client::local::{LocalError, RestartOutcome, ServiceIdentity};
use kuku_client::{
    AgentProgress, AgentStatus, Connection, ConnectionOptions, RetryFailureKind, RetryMetadata,
    RetrySource, Snapshot, Target,
};
use sha2::{Digest, Sha256};
use url::Url;

use crate::local_settings::{active_service, ensure_same_service};

use super::config::{save_credential, Settings};

const CHANGED: &str =
    "The failed request, model, or local service changed; reopen credential repair.";
const ROUTE_UNAVAILABLE: &str = "The failed request has no complete recorded route. Inspect the service connection settings on its host before retrying; no credential was saved.";
const SAVE_FAILED: &str = "The credential could not be saved. No settings were changed; reopen credential repair and try again.";
const SAVED_NOT_APPLIED: &str = "Credentials were saved, but application to this Session is not confirmed. No request was retried.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CredentialRepairRequest {
    pub(crate) target: Target,
    pub(crate) turn: u64,
    pub(crate) request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FailedRoute {
    pub(crate) connection: String,
    pub(crate) model: String,
    pub(crate) protocol: ProviderFormat,
    pub(crate) endpoint: Url,
}

pub(crate) struct CredentialRepairPlan {
    pub(crate) request: CredentialRepairRequest,
    pub(crate) route: FailedRoute,
    pub(crate) config_path: PathBuf,
    settings: Settings,
    service: ServiceIdentity,
}

pub(crate) enum PreparedCredentialRepair {
    Local(Arc<CredentialRepairPlan>),
    Remote(FailedRoute),
}

#[derive(Clone)]
pub(crate) struct SavedCredentialRepair {
    config_sha256: String,
    service: ServiceIdentity,
}

pub(crate) enum CredentialRepairApplied {
    Applied,
    SavedNotApplied(SavedCredentialRepair, String),
    SavedTargetChanged(String),
}

pub(crate) fn authentication_failure(snapshot: &Snapshot) -> Option<CredentialRepairRequest> {
    let AgentProgress::RetryStopped { retry } = snapshot.progress.as_ref()? else {
        return None;
    };
    CredentialRepairRequest::from_retry(snapshot, retry)
}

impl CredentialRepairRequest {
    pub(crate) fn from_retry(snapshot: &Snapshot, retry: &RetryMetadata) -> Option<Self> {
        if snapshot.agent.status != AgentStatus::Interrupted
            || snapshot.cleaning_up
            || retry.source != RetrySource::Provider
            || retry.failed_kind != Some(RetryFailureKind::Authentication)
            || retry.will_retry != Some(false)
        {
            return None;
        }
        Some(Self {
            target: Target {
                workspace: snapshot.workspace.clone(),
                session_id: snapshot.session_id.clone(),
                agent: snapshot.agent.address.clone(),
            },
            turn: snapshot.agent.turn?,
            request_id: retry.request_id.clone()?,
        })
    }
}

pub(crate) async fn prepare(
    options: &ConnectionOptions,
    connection: Option<&Connection>,
    request: CredentialRepairRequest,
) -> Result<PreparedCredentialRepair, String> {
    let connection = connection.ok_or_else(|| CHANGED.to_owned())?;
    let snapshot = connection
        .client
        .observation_snapshot(&request.target)
        .await
        .map_err(|_| CHANGED)?;
    verify_failure(&snapshot, &request)?;
    let route = failed_route(connection, &request, snapshot.cursor).await?;
    verify_route(&snapshot, &route)?;
    if options.remote.is_some() || options.token.is_some() || connection.local.is_none() {
        return Ok(PreparedCredentialRepair::Remote(route));
    }
    let service = active_service(options, Some(connection))
        .await
        .map_err(|_| CHANGED)?;
    let environment = |name: &str| std::env::var(name);
    let settings = Settings::load_with_status(
        options.clone(),
        Some(service.config_path.clone()),
        &environment,
    )
    .map_err(|_| CHANGED)?;
    if !settings.can_store_key() || !settings.resolved.providers.contains_key(&route.connection) {
        return Err("Private credential storage is unavailable for this connection. Correct the service configuration on its host and restart it before retrying.".into());
    }
    verify_config_route(&settings, &route)?;
    Ok(PreparedCredentialRepair::Local(Arc::new(
        CredentialRepairPlan {
            config_path: service.config_path.clone(),
            request,
            route,
            settings,
            service,
        },
    )))
}

pub(crate) async fn save(
    options: &ConnectionOptions,
    connection: Option<&Connection>,
    plan: Arc<CredentialRepairPlan>,
    key: String,
) -> Result<CredentialRepairApplied, String> {
    let connection = connection.ok_or_else(|| CHANGED.to_owned())?;
    let current = active_service(options, Some(connection))
        .await
        .map_err(|_| CHANGED)?;
    ensure_same_service(&plan.service, &current).map_err(|_| CHANGED)?;
    let snapshot = connection
        .client
        .observation_snapshot(&plan.request.target)
        .await
        .map_err(|_| CHANGED)?;
    verify_failure(&snapshot, &plan.request)?;
    verify_route(&snapshot, &plan.route)?;
    let service = plan.service.clone();
    let config_sha256 = tokio::task::spawn_blocking(move || {
        save_credential(&plan.settings, &plan.route.connection, &key)
    })
    .await
    .map_err(|_| SAVE_FAILED)?
    .map_err(|_| SAVE_FAILED)?;
    Ok(apply(
        options,
        SavedCredentialRepair {
            config_sha256,
            service,
        },
    )
    .await)
}

pub(crate) async fn apply(
    options: &ConnectionOptions,
    saved: SavedCredentialRepair,
) -> CredentialRepairApplied {
    let changed =
        || CredentialRepairApplied::SavedTargetChanged(format!("{SAVED_NOT_APPLIED}\n{CHANGED}"));
    if verify_saved_config(&saved).is_err() {
        return changed();
    }
    let Ok(local) = options.local_service() else {
        return changed();
    };
    let reason = match local.restart_if_current(&saved.service).await {
        Ok(RestartOutcome::Restarted(_)) => {
            return if verify_saved_config(&saved).is_ok() {
                CredentialRepairApplied::Applied
            } else {
                changed()
            };
        }
        Ok(RestartOutcome::Deferred(_)) => {
            "Other work is still active. Finish it, then choose Restart local service here."
        }
        Err(LocalError::ServiceChanged) => return changed(),
        Err(_) => "Restart failed; use the local service restart action when it is idle.",
    };
    CredentialRepairApplied::SavedNotApplied(saved, format!("{SAVED_NOT_APPLIED}\n{reason}"))
}

fn verify_saved_config(saved: &SavedCredentialRepair) -> Result<(), String> {
    let bytes = fs::read(&saved.service.config_path).map_err(|_| CHANGED)?;
    if format!("{:x}", Sha256::digest(bytes)) != saved.config_sha256 {
        return Err(CHANGED.into());
    }
    Ok(())
}

fn verify_failure(snapshot: &Snapshot, request: &CredentialRepairRequest) -> Result<(), String> {
    if authentication_failure(snapshot).as_ref() != Some(request) {
        return Err(CHANGED.into());
    }
    Ok(())
}

fn verify_route(snapshot: &Snapshot, route: &FailedRoute) -> Result<(), String> {
    let same_route = |selection: &kuku_client::AgentSelection| {
        selection.connection == route.connection
            && selection.model == route.model
            && selection.protocol == route.protocol
    };
    if !snapshot.selection.as_ref().is_some_and(same_route)
        || snapshot
            .pending_selection
            .as_ref()
            .is_some_and(|selection| !same_route(selection))
    {
        return Err(CHANGED.into());
    }
    Ok(())
}

fn verify_config_route(settings: &Settings, route: &FailedRoute) -> Result<(), String> {
    let provider = settings
        .resolved
        .providers
        .get(&route.connection)
        .ok_or(CHANGED)?;
    let endpoint = Url::parse(&provider.base_url).map_err(|_| CHANGED)?;
    if provider.format != route.protocol || endpoint != route.endpoint {
        return Err(CHANGED.into());
    }
    Ok(())
}

async fn failed_route(
    connection: &Connection,
    request: &CredentialRepairRequest,
    head: u64,
) -> Result<FailedRoute, String> {
    let mut after = request.turn.saturating_sub(1);
    let mut selected = None;
    while after < head {
        let page = connection
            .client
            .events(
                &request.target.workspace,
                &request.target.session_id,
                Some(&request.target.agent),
                after,
                256,
            )
            .await
            .map_err(|_| CHANGED)?;
        for event in page.events {
            let EventPayload::Unknown(record) = event.payload else {
                continue;
            };
            if event.id <= head
                && record["kind"] == "runtime.step_prepared"
                && record["agent"] == request.target.agent
                && record["turn"] == request.turn
                && record["request_id"] == request.request_id
            {
                selected = Some((
                    record["connection"]
                        .as_str()
                        .ok_or(ROUTE_UNAVAILABLE)?
                        .to_owned(),
                    record["model"]
                        .as_str()
                        .ok_or(ROUTE_UNAVAILABLE)?
                        .to_owned(),
                    record["segment"].as_u64().ok_or(ROUTE_UNAVAILABLE)?,
                ));
            }
        }
        if page.next_after <= after {
            break;
        }
        after = page.next_after;
    }
    let (name, model, segment) = selected.ok_or(ROUTE_UNAVAILABLE)?;
    let page = connection
        .client
        .events(
            &request.target.workspace,
            &request.target.session_id,
            Some(&request.target.agent),
            segment.saturating_sub(1),
            1,
        )
        .await
        .map_err(|_| ROUTE_UNAVAILABLE)?;
    let record = page
        .events
        .into_iter()
        .find_map(|event| match event.payload {
            EventPayload::Unknown(record)
                if event.id == segment
                    && record["kind"] == "runtime.context_segment"
                    && record["agent"] == request.target.agent
                    && record["connection"] == name
                    && record["model"] == model =>
            {
                Some(record)
            }
            _ => None,
        })
        .ok_or(ROUTE_UNAVAILABLE)?;
    let protocol = match record["protocol"].as_str().ok_or(ROUTE_UNAVAILABLE)? {
        "openai-compatible" => ProviderFormat::OpenAiChat,
        protocol => protocol.parse().map_err(|_| ROUTE_UNAVAILABLE)?,
    };
    let endpoint = Url::parse(record["endpoint"].as_str().ok_or(ROUTE_UNAVAILABLE)?)
        .map_err(|_| ROUTE_UNAVAILABLE)?;
    Ok(FailedRoute {
        connection: name,
        model,
        protocol,
        endpoint,
    })
}

#[cfg(test)]
#[path = "repair_backend_tests.rs"]
mod tests;
