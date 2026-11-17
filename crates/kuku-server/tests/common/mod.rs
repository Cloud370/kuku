#[allow(dead_code)]
pub mod mock_provider;
#[allow(dead_code)]
pub mod stream;

use std::net::SocketAddr;
use std::sync::Arc;

use arc_swap::ArcSwap;
use tokio::sync::Mutex;

#[allow(dead_code)]
pub struct TestServer {
    pub addr: SocketAddr,
    pub base_url: String,
    pub workspace: tempfile::TempDir,
    pub home: tempfile::TempDir,
    pub state: Arc<kuku_server::AppState>,
    handle: Option<tokio::task::JoinHandle<()>>,
}

#[allow(
    dead_code,
    reason = "integration binaries use different subsets of shared helpers"
)]
impl TestServer {
    pub async fn start(config: kuku::config::Config) -> Self {
        Self::start_with_password(config, None).await
    }

    pub async fn start_with_password(
        config: kuku::config::Config,
        password: Option<String>,
    ) -> Self {
        Self::start_with_authorizer(config, password, Arc::new(kuku::permission::TrustedLocal))
            .await
    }

    pub async fn start_deferred(config: kuku::config::Config) -> Self {
        Self::start_with_authorizer(config, None, Arc::new(DeferTools)).await
    }

    async fn start_with_authorizer(
        config: kuku::config::Config,
        password: Option<String>,
        authorizer: Arc<dyn kuku::permission::ToolAuthorizer>,
    ) -> Self {
        let workspace = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();

        let config_store = Arc::new(ArcSwap::from_pointee(config));

        let state = Arc::new(kuku_server::AppState {
            run_manager: Mutex::new(kuku_server::run_manager::RunManager::new(16)),
            config: config_store,
            password,
            kuku_home: home.path().to_path_buf(),
            authorizer,
        });

        let app = kuku_server::build_app(state.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://{addr}");

        let handle = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });

        Self {
            addr,
            base_url,
            workspace,
            home,
            state,
            handle: Some(handle),
        }
    }

    pub async fn close(&mut self) {
        self.state
            .run_manager
            .lock()
            .await
            .close_all()
            .await
            .unwrap();
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

#[derive(Debug)]
#[allow(
    dead_code,
    reason = "only deferred-authorization integration cases construct this policy"
)]
struct DeferTools;

impl kuku::permission::ToolAuthorizer for DeferTools {
    fn authorize(
        &self,
        _action: &kuku::permission::ToolAuthorization,
    ) -> kuku::Result<kuku::permission::AuthorizationDecision> {
        Ok(kuku::permission::AuthorizationDecision::Defer)
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            h.abort();
        }
    }
}
