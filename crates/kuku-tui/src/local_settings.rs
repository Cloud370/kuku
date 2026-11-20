use kuku_client::local::ServiceIdentity;
use kuku_client::{Connection, ConnectionOptions};
use url::Url;

pub(crate) const LOCAL_SERVICE_REQUIRED: &str = "Set current model as default requires the active local service; reopen /model on a local connection.";
pub(crate) const LOCAL_SERVICE_CHANGED: &str =
    "The local service or config changed; reopen /model before saving.";

pub(crate) async fn active_service(
    options: &ConnectionOptions,
    connection: Option<&Connection>,
) -> Result<ServiceIdentity, String> {
    let connection = connection.ok_or_else(|| LOCAL_SERVICE_REQUIRED.to_owned())?;
    require_local(
        options.remote.is_some(),
        options.token.is_some(),
        connection.local.is_some(),
    )?;
    let service = options
        .local_service()
        .map_err(|_| LOCAL_SERVICE_REQUIRED.to_owned())?;
    let active = service
        .status()
        .await
        .map_err(|_| LOCAL_SERVICE_CHANGED.to_owned())?
        .ok_or_else(|| LOCAL_SERVICE_CHANGED.to_owned())?;
    if !same_service_endpoint(connection.client.endpoint(), active.endpoint()) {
        return Err(LOCAL_SERVICE_CHANGED.into());
    }
    let identity = active.identity();
    if options.config.is_some() && identity.config_path != service.config_path() {
        return Err(LOCAL_SERVICE_CHANGED.into());
    }
    Ok(identity)
}

pub(crate) fn same_service_endpoint(client_endpoint: &str, active_endpoint: &str) -> bool {
    let (Some(client), Some(active)) = (
        service_endpoint_url(client_endpoint),
        service_endpoint_url(active_endpoint),
    ) else {
        return false;
    };
    client == active
}

fn service_endpoint_url(endpoint: &str) -> Option<Url> {
    let endpoint = Url::parse(endpoint).ok()?;
    if !matches!(endpoint.scheme(), "http" | "https")
        || endpoint.host_str().is_none()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return None;
    }
    Some(endpoint)
}

pub(crate) fn require_local(
    remote: bool,
    token: bool,
    connected_local: bool,
) -> Result<(), String> {
    if remote || token || !connected_local {
        Err(LOCAL_SERVICE_REQUIRED.into())
    } else {
        Ok(())
    }
}

pub(crate) fn ensure_same_service(
    expected: &ServiceIdentity,
    current: &ServiceIdentity,
) -> Result<(), String> {
    if expected == current {
        Ok(())
    } else {
        Err(LOCAL_SERVICE_CHANGED.into())
    }
}
