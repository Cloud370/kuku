#[allow(dead_code)]
pub mod mock_provider;

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
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl TestServer {
    pub async fn start(config: kuku::config::Config) -> Self {
        Self::start_with_password(config, None).await
    }

    pub async fn start_with_password(
        config: kuku::config::Config,
        password: Option<String>,
    ) -> Self {
        let workspace = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();

        std::env::set_var("KUKU_HOME", home.path());

        let config_store = Arc::new(ArcSwap::from_pointee(config));

        let state = Arc::new(kuku_server::AppState {
            run_manager: Mutex::new(kuku_server::run_manager::RunManager::new(16)),
            config: config_store,
            password,
        });

        let app = kuku_server::build_app(state);

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
            handle: Some(handle),
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            h.abort();
        }
    }
}
