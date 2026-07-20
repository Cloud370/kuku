#[allow(dead_code)]
pub mod context_fixtures;
#[allow(dead_code)]
pub mod mock_provider;
#[allow(dead_code)]
pub mod stream;

use std::net::SocketAddr;

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

        let registration_root = workspace.path().parent().unwrap().to_path_buf();
        let state = kuku_server::AppState::open(
            home.path(),
            Some(config),
            password,
            vec![kuku_server::platform::RegistrationRootSpec {
                label: "Test workspaces".to_owned(),
                path: registration_root,
            }],
            "http://127.0.0.1".to_owned(),
            16,
        )
        .await
        .unwrap();

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

impl TestServer {
    #[allow(dead_code)]
    pub async fn shutdown(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
            let _ = handle.await;
        }
    }
}
