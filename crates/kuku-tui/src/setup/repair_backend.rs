use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use kuku::event::EventPayload;
use kuku_client::local::RestartOutcome;
use kuku_client::{
    AgentProgress, AgentStatus, Connection, ConnectionOptions, RetryFailureKind, RetrySource,
    Snapshot, Target,
};
use sha2::{Digest, Sha256};

use crate::local_settings::{
    active_service, ensure_same_service, service_identity, ServiceIdentity,
};

use super::config::{save_credential, Settings};

const CHANGED: &str =
    "The failed request, model, or local service changed; reopen credential repair.";
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
}

pub(crate) fn authentication_failure(snapshot: &Snapshot) -> Option<CredentialRepairRequest> {
    if snapshot.agent.status != AgentStatus::Interrupted || snapshot.cleaning_up {
        return None;
    }
    let AgentProgress::RetryStopped { retry } = snapshot.progress.as_ref()? else {
        return None;
    };
    if retry.source != RetrySource::Provider
        || retry.failed_kind != Some(RetryFailureKind::Authentication)
        || retry.will_retry != Some(false)
    {
        return None;
    }
    Some(CredentialRepairRequest {
        target: Target {
            workspace: snapshot.workspace.clone(),
            session_id: snapshot.session_id.clone(),
            agent: snapshot.agent.address.clone(),
        },
        turn: snapshot.agent.turn?,
        request_id: retry.request_id.clone()?,
    })
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
    let current = active_service(options, Some(connection))
        .await
        .map_err(|_| CHANGED)?;
    ensure_same_service(&service, &current).map_err(|_| CHANGED)?;
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
        Some(connection),
        SavedCredentialRepair {
            config_sha256,
            service,
        },
    )
    .await)
}

pub(crate) async fn apply(
    options: &ConnectionOptions,
    connection: Option<&Connection>,
    saved: SavedCredentialRepair,
) -> CredentialRepairApplied {
    match restart(options, connection, &saved).await {
        Ok(()) => CredentialRepairApplied::Applied,
        Err(reason) => CredentialRepairApplied::SavedNotApplied(
            saved,
            format!("{SAVED_NOT_APPLIED}\n{reason}"),
        ),
    }
}

async fn restart(
    options: &ConnectionOptions,
    connection: Option<&Connection>,
    saved: &SavedCredentialRepair,
) -> Result<(), String> {
    let current = active_service(options, connection)
        .await
        .map_err(|_| CHANGED)?;
    ensure_same_service(&saved.service, &current).map_err(|_| CHANGED)?;
    verify_saved_config(saved)?;
    let local = options.local_service().map_err(|_| CHANGED)?;
    let restarted =
        match local
            .restart()
            .await
            .map_err(|_| "Restart failed; use the local service restart action when it is idle.")?
        {
            RestartOutcome::Deferred(_) => return Err(
                "Other work is still active. Finish it, then choose Restart local service here."
                    .into(),
            ),
            RestartOutcome::Restarted(connection) => {
                service_identity(options, &connection).map_err(|_| CHANGED)?
            }
        };
    verify_restarted_service(saved, &restarted)
}

fn verify_restarted_service(
    saved: &SavedCredentialRepair,
    restarted: &ServiceIdentity,
) -> Result<(), String> {
    if restarted.config_path != saved.service.config_path
        || restarted.data_dir != saved.service.data_dir
        || restarted.instance_id == saved.service.instance_id
    {
        return Err(CHANGED.into());
    }
    verify_saved_config(saved)
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
        selection.connection == route.connection && selection.model == route.model
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
                selected = Some(FailedRoute {
                    connection: record["connection"].as_str().ok_or(CHANGED)?.to_owned(),
                    model: record["model"].as_str().ok_or(CHANGED)?.to_owned(),
                });
            }
        }
        if page.next_after <= after {
            break;
        }
        after = page.next_after;
    }
    selected.ok_or_else(|| CHANGED.to_owned())
}

#[cfg(test)]
#[path = "repair_backend_tests.rs"]
mod tests;
