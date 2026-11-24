use std::fs;
use std::future::{Future, poll_fn};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::task::Poll;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;

use super::{BuildPolicy, LocalError, LocalService, ServiceStatus};

struct ManagedState {
    status: Mutex<ServiceStatus>,
    restarts: Mutex<Vec<String>>,
}

struct ManagedFixture {
    _home: tempfile::TempDir,
    service: LocalService,
    lease: Option<fs::File>,
    state: Arc<ManagedState>,
    address: SocketAddr,
    server: tokio::task::JoinHandle<()>,
}

impl ManagedFixture {
    async fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        fs::write(
            home.path().join("config.toml"),
            kuku::config::generate_default(),
        )
        .unwrap();
        let service = LocalService::resolve(home.path(), None).unwrap();
        let directory = service.service_directory();
        kuku::util::private_directory(&directory).unwrap();
        let lease = fs::File::create(directory.join("instance.lock")).unwrap();
        lease.try_lock().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let state = Arc::new(ManagedState {
            status: Mutex::new(ServiceStatus {
                version: "fixture".into(),
                protocol_version: super::SESSION_PROTOCOL_VERSION,
                management_version: super::MANAGEMENT_VERSION,
                instance_id: Some("a".repeat(64)),
                pid: Some(std::process::id()),
                data_dir: Some(service.data_dir().to_string_lossy().into_owned()),
                config_path: Some(service.config_path().to_string_lossy().into_owned()),
                config_sha256: None,
                busy: Vec::new(),
                draining: false,
            }),
            restarts: Mutex::new(Vec::new()),
        });
        let app = Router::new()
            .route("/service/status", get(managed_status))
            .route("/service/restart", axum::routing::post(managed_restart))
            .with_state(state.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let fixture = Self {
            _home: home,
            service,
            lease: Some(lease),
            state,
            address,
            server,
        };
        fixture.publish('a', fixture.service.config_path());
        fixture
    }

    fn publish(&self, instance: char, config: &Path) {
        fs::write(config, kuku::config::generate_default()).unwrap();
        let mut status = self.state.status.lock().unwrap();
        status.instance_id = Some(instance.to_string().repeat(64));
        status.config_path = Some(config.to_string_lossy().into_owned());
        fs::write(self.service.service_directory().join("connection.json"), serde_json::to_vec(&json!({
            "address":self.address,"token":"c".repeat(64),"instance_id":status.instance_id,
            "pid":status.pid,"config_path":config.as_os_str(),"data_dir":self.service.data_dir().as_os_str(),
        })).unwrap()).unwrap();
    }
}

impl Drop for ManagedFixture {
    fn drop(&mut self) {
        self.lease.take();
        self.server.abort();
    }
}

async fn managed_status(State(state): State<Arc<ManagedState>>) -> Json<ServiceStatus> {
    Json(state.status.lock().unwrap().clone())
}

async fn managed_restart(
    State(state): State<Arc<ManagedState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    state
        .restarts
        .lock()
        .unwrap()
        .push(state.status.lock().unwrap().instance_id.clone().unwrap());
    (
        StatusCode::CONFLICT,
        Json(
            json!({"code":"service_busy","message":"fixture remains running","busy":["fixture work"]}),
        ),
    )
}

#[tokio::test]
async fn shutdown_wait_requires_lease_release_after_transport_reset() {
    let mut fixture = ManagedFixture::new().await;
    fixture.server.abort();
    let _ = (&mut fixture.server).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    fixture.address = listener.local_addr().unwrap();
    fixture.publish('a', fixture.service.config_path());
    let reset = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        socket.set_zero_linger().unwrap();
        socket.readable().await.unwrap();
        drop(socket);
    });
    let failure = match fixture.service.status().await {
        Err(error) => error,
        Ok(_) => panic!("reset endpoint unexpectedly answered status"),
    };
    reset.await.unwrap();
    assert!(
        matches!(&failure, LocalError::Transport(error) if error.is_request() && !error.is_connect()),
        "{failure}"
    );

    let waiting = fixture.service.wait_for_exit();
    tokio::pin!(waiting);
    poll_fn(|context| {
        assert!(waiting.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
    drop(fixture.lease.take());
    tokio::time::timeout(std::time::Duration::from_secs(2), waiting)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn observed_instance_replacement_while_waiting_for_launcher_is_not_restarted() {
    let fixture = ManagedFixture::new().await;
    let observed = fixture.service.status().await.unwrap().unwrap();
    let mut launcher =
        fslock::LockFile::open(&fixture.service.service_directory().join("launcher.lock")).unwrap();
    launcher.lock().unwrap();
    let expected = observed.identity();
    let mut restart = Box::pin(fixture.service.restart_if_current(&expected));
    poll_fn(|context| {
        assert!(restart.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
    fixture.publish('b', &fixture.service.data_dir().join("config-b.toml"));
    launcher.unlock().unwrap();
    assert!(matches!(restart.await, Err(LocalError::ServiceChanged)));
    assert_eq!(
        0,
        fixture.state.restarts.lock().unwrap().len(),
        "a replacement instance must receive no restart request"
    );
    assert_ne!(
        observed.status().instance_id,
        fixture
            .service
            .status()
            .await
            .unwrap()
            .unwrap()
            .status()
            .instance_id
    );
}

#[tokio::test]
async fn conditional_restart_does_not_launch_when_the_observed_instance_disappears() {
    let mut fixture = ManagedFixture::new().await;
    let observed = fixture.service.status().await.unwrap().unwrap();
    fixture.lease.take();
    assert!(matches!(
        fixture
            .service
            .restart_if_current(&observed.identity())
            .await,
        Err(LocalError::ServiceChanged)
    ));
    assert!(fixture.state.restarts.lock().unwrap().is_empty());
}

#[tokio::test]
async fn conditional_restart_keeps_busy_work_outside_instance_identity() {
    let fixture = ManagedFixture::new().await;
    let observed = fixture.service.status().await.unwrap().unwrap();
    fixture.state.status.lock().unwrap().busy = vec!["new work".into()];
    assert!(
        matches!(fixture.service.restart_if_current(&observed.identity()).await, Ok(super::RestartOutcome::Deferred(status)) if status.busy == ["fixture work"])
    );
    assert_eq!(1, fixture.state.restarts.lock().unwrap().len());
}

#[tokio::test]
async fn explicit_restart_still_operates_on_the_current_replacement() {
    let fixture = ManagedFixture::new().await;
    let observed = fixture.service.status().await.unwrap().unwrap();
    fixture.publish('b', &fixture.service.data_dir().join("config-b.toml"));
    assert!(matches!(
        fixture.service.restart().await,
        Ok(super::RestartOutcome::Deferred(_))
    ));
    assert_eq!(
        ["b".repeat(64)],
        fixture.state.restarts.lock().unwrap().as_slice()
    );
    assert_ne!(
        observed.status().instance_id,
        fixture
            .service
            .status()
            .await
            .unwrap()
            .unwrap()
            .status()
            .instance_id
    );
}

#[tokio::test]
async fn abandoned_discovery_is_ignored_even_if_its_record_is_invalid() {
    let home = tempfile::tempdir().unwrap();
    let service = LocalService::resolve(home.path(), None).unwrap();
    let directory = service.service_directory();
    kuku::util::private_directory(&directory).unwrap();
    std::fs::File::create(directory.join("instance.lock")).unwrap();
    fs::write(directory.join("connection.json"), b"old record").unwrap();
    assert!(service.status().await.unwrap().is_none());
    assert!(matches!(
        service.connect().await,
        Err(LocalError::ConfigMissing(path)) if path == service.config_path()
    ));
}

#[cfg(windows)]
#[tokio::test]
async fn a_running_instance_reports_a_persistent_discovery_access_error() {
    let home = tempfile::tempdir().unwrap();
    let service = LocalService::resolve(home.path(), None).unwrap();
    let directory = service.service_directory();
    kuku::util::private_directory(&directory).unwrap();
    let lease = fs::File::create(directory.join("instance.lock")).unwrap();
    lease.try_lock().unwrap();
    fs::create_dir(directory.join("connection.json")).unwrap();

    assert!(matches!(
        service.status().await,
        Err(LocalError::Io(error)) if error.kind() == std::io::ErrorKind::PermissionDenied
    ));
}

#[tokio::test]
async fn installed_build_reuses_the_match_and_defers_a_busy_upgrade() {
    let fixture = ManagedFixture::new().await;
    let service = &fixture.service;
    fixture.state.status.lock().unwrap().version = "0.9.0 (before)".into();
    let same = service
        .connect_build(BuildPolicy::Start("0.9.0 (before)"))
        .await
        .unwrap();
    assert!(fixture.state.restarts.lock().unwrap().is_empty());
    assert!(
        matches!(service.connect_build(BuildPolicy::Start("0.9.0 (after)")).await,
            Err(LocalError::BuildDeferred { running, requested }) if
                running.instance_id.as_deref() == Some(same.identity().instance_id.as_str())
                && running.busy == ["fixture work"]
                && requested == "0.9.0 (after)")
    );
    assert_eq!(1, fixture.state.restarts.lock().unwrap().len());
}

#[tokio::test]
async fn old_releases_and_already_open_windows_cannot_replace_a_running_build() {
    let fixture = ManagedFixture::new().await;
    fixture.state.status.lock().unwrap().version = "0.10.0 (current)".into();
    for policy in [
        BuildPolicy::Start("0.9.0 (older)"),
        BuildPolicy::Start("0.10.0-rc.1 (candidate)"),
        BuildPolicy::Reconnect("0.10.0 (another)"),
    ] {
        assert!(matches!(
            fixture.service.connect_build(policy).await,
            Err(LocalError::BuildMismatch { running, .. }) if running == "0.10.0 (current)"
        ));
    }
    assert!(fixture.state.restarts.lock().unwrap().is_empty());
}

#[tokio::test]
async fn matching_a_build_cannot_switch_a_conflicting_config_path() {
    let fixture = ManagedFixture::new().await;
    let other = fixture.service.data_dir().join("config-b.toml");
    let service = LocalService::resolve(fixture.service.data_dir(), Some(&other)).unwrap();
    assert!(matches!(
        service.connect_build(BuildPolicy::Start("0.1.0 (update)")).await,
        Err(LocalError::ConfigConflict { active, requested })
            if active == fixture.service.config_path() && requested == other
    ));
    assert!(fixture.state.restarts.lock().unwrap().is_empty());
}
