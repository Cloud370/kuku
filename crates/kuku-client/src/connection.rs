use std::path::{Path, PathBuf};

use crate::local::{BuildPolicy, LocalService};
use crate::{Client, Error, PendingStore};

/// Shared local and explicit remote connection choices for non-UI and interactive clients.
#[derive(Clone, Default)]
pub struct ConnectionOptions {
    /// Workspace selected by the client; required for remote Session operations.
    pub workspace: Option<PathBuf>,
    /// User data directory, including client-side pending records.
    pub data_dir: Option<PathBuf>,
    /// Explicit configuration path for the managed service.
    pub config: Option<PathBuf>,
    /// Explicitly selected remote service URL.
    pub remote: Option<String>,
    /// Authentication token for the remote service.
    pub token: Option<String>,
}

impl ConnectionOptions {
    /// Get the local path used for private client state and a managed service.
    pub fn data_dir(&self) -> Result<PathBuf, Error> {
        self.data_dir.clone().map_or_else(
            || kuku::session::kuku_home().map_err(|error| Error::Options(error.to_string())),
            Ok,
        )
    }

    /// Resolve a local workspace or return the path explicitly supplied to a remote host.
    pub fn workspace(&self) -> Result<PathBuf, Error> {
        if self.remote.is_some() {
            return self.workspace.clone().ok_or_else(|| {
                Error::Options("--workspace must specify a directory on the remote server".into())
            });
        }
        match &self.workspace {
            Some(path) => path.canonicalize().map_err(|error| {
                Error::Options(format!("resolve workspace {}: {error}", path.display()))
            }),
            None => kuku::session::current_workspace()
                .map_err(|error| Error::Options(error.to_string())),
        }
    }

    /// Resolve management options for a managed local service.
    pub fn local_service(&self) -> Result<LocalService, Error> {
        if self.remote.is_some() || self.token.is_some() {
            return Err(Error::Options(
                "local service management requires local connection options".into(),
            ));
        }
        Ok(LocalService::resolve(
            &self.data_dir()?,
            self.config.as_deref(),
        )?)
    }

    /// Build the explicit remote client without falling back to local discovery.
    pub fn remote_connection(&self) -> Result<Option<Client>, Error> {
        let Some(endpoint) = self.remote.as_deref() else {
            if self.token.is_some() {
                return Err(Error::Options("--token requires --remote".into()));
            }
            return Ok(None);
        };
        if self.config.is_some() {
            return Err(Error::Options(
                "configure a remote service on its host".into(),
            ));
        }
        let token = self
            .token
            .clone()
            .or_else(|| std::env::var("KUKU_TOKEN").ok())
            .ok_or_else(|| {
                Error::Options("remote authentication requires --token or KUKU_TOKEN".into())
            })?;
        Client::remote(endpoint, &token).map(Some)
    }

    /// Connect to an authenticated compatible service without selecting a workspace.
    pub async fn client(&self) -> Result<Client, Error> {
        self.open(None).await.map(|(client, _)| client)
    }

    /// Connect using the installed client's build for a managed local service.
    pub async fn client_for_build(&self, policy: BuildPolicy<'_>) -> Result<Client, Error> {
        self.open(Some(policy)).await.map(|(client, _)| client)
    }

    async fn open(
        &self,
        build: Option<BuildPolicy<'_>>,
    ) -> Result<(Client, Option<LocalService>), Error> {
        if let Some(client) = self.remote_connection()? {
            client.check_protocol().await?;
            return Ok((client, None));
        }
        let service = self.local_service()?;
        let connection = match build {
            Some(policy) => service.connect_build(policy).await?,
            None => service.connect().await?,
        };
        let client = Client::remote(connection.endpoint(), connection.token())?;
        Ok((client, Some(service)))
    }

    /// Connect, resolve the selected workspace, and open the scoped private pending store.
    pub async fn connect(&self) -> Result<Connection, Error> {
        self.connect_with(None).await
    }

    /// Open a workspace using the installed client's build for a managed local service.
    pub async fn connect_for_build(&self, policy: BuildPolicy<'_>) -> Result<Connection, Error> {
        self.connect_with(Some(policy)).await
    }

    async fn connect_with(&self, build: Option<BuildPolicy<'_>>) -> Result<Connection, Error> {
        let workspace = self.workspace()?;
        let (client, local) = self.open(build).await?;
        let workspace = client.workspace(workspace).await?;
        let (data_dir, scope) = match &local {
            Some(service) => (service.data_dir().to_path_buf(), "local"),
            None => (self.data_dir()?, client.endpoint()),
        };
        let pending = PendingStore::for_service(&data_dir, scope)?;
        Ok(Connection {
            client,
            workspace,
            pending,
            local,
        })
    }
}

/// An authenticated service connection for one canonical workspace.
#[derive(Clone)]
pub struct Connection {
    /// Shared authenticated HTTP client.
    pub client: Client,
    /// Workspace resolved by the service.
    pub workspace: PathBuf,
    /// Pending commands for this specific local or remote service.
    pub pending: PendingStore,
    /// Managed service identity when using a local connection.
    pub local: Option<LocalService>,
}

impl Connection {
    /// Build an Agent target bound to this connection's selected workspace.
    pub fn target(&self, session_id: impl Into<String>, agent: impl Into<String>) -> crate::Target {
        crate::Target {
            workspace: self.workspace.clone(),
            session_id: session_id.into(),
            agent: agent.into(),
        }
    }

    /// Resolve a path supplied by the caller for the connected service host.
    pub fn service_path(&self, path: &Path) -> Result<PathBuf, Error> {
        if self.local.is_some() {
            path.canonicalize().map_err(|error| {
                Error::Options(format!("resolve path {}: {error}", path.display()))
            })
        } else {
            Ok(path.to_path_buf())
        }
    }
}
