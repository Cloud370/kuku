use std::fs;
use std::sync::Arc;

use httpmock::prelude::{MockServer, GET};
use kuku::config::{generate_default, load_config, ApiKey};
use kuku_client::local::ServiceIdentity;
use kuku_client::{
    AgentProgress, AgentSelection, AgentStatus, Connection, ConnectionOptions, RetryFailureKind,
    RetryMetadata, RetryPhase, RetrySource, Snapshot,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use toml_edit::{value, DocumentMut};

use super::super::config::{save_credential, Settings};
use super::{
    authentication_failure, prepare, verify_config_route, verify_failure, verify_route,
    verify_saved_config, CredentialRepairApplied, CredentialRepairPlan, FailedRoute,
    PreparedCredentialRepair, SavedCredentialRepair,
};

fn snapshot() -> Snapshot {
    let mut snapshot: Snapshot = serde_json::from_value(json!({
        "session_id":"session", "workspace":std::env::current_dir().unwrap(), "cursor":20,
        "agent":{"address":"main", "status":"interrupted", "turn":7, "config":{},
            "pending_inputs":0, "usage":{"model_requests":1,"unreported_requests":0}},
        "agents":[], "pending":[],
    }))
    .unwrap();
    snapshot.progress = Some(AgentProgress::RetryStopped {
        retry: RetryMetadata {
            source: RetrySource::Provider,
            phase: RetryPhase::Stopped,
            failed_kind: Some(RetryFailureKind::Authentication),
            will_retry: Some(false),
            request_id: Some("req-1".into()),
            ..Default::default()
        },
    });
    snapshot.selection = Some(AgentSelection {
        connection: "actual-failed-provider".into(),
        model: "actual-model".into(),
        protocol: kuku::config::ProviderFormat::Anthropic,
        context_window_tokens: 16_000,
        max_output_tokens: None,
        settings: Default::default(),
        output: Default::default(),
        request_url: "https://example.test/v1/messages".into(),
        capabilities: Default::default(),
        tools: Vec::new(),
    });
    snapshot
}

fn settings(root: &std::path::Path, text: &str) -> Settings {
    let workspace = root.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let config = root.join("config.toml");
    fs::write(&config, text).unwrap();
    Settings::load_with_status(
        ConnectionOptions {
            workspace: Some(workspace),
            data_dir: Some(root.join("data")),
            config: Some(config),
            ..Default::default()
        },
        None,
        &|name| std::env::var(name),
    )
    .unwrap()
}

#[test]
fn credential_repair_is_offered_only_for_a_typed_stopped_authentication_failure() {
    let mut snapshot = snapshot();
    assert!(authentication_failure(&snapshot).is_some());
    for kind in [
        RetryFailureKind::PermissionDenied,
        RetryFailureKind::RateLimited,
        RetryFailureKind::Transport,
        RetryFailureKind::Unknown,
    ] {
        if let Some(AgentProgress::RetryStopped { retry }) = &mut snapshot.progress {
            retry.failed_kind = Some(kind);
        }
        assert!(authentication_failure(&snapshot).is_none());
    }
    snapshot = self::snapshot();
    snapshot.agent.status = AgentStatus::Running;
    assert!(authentication_failure(&snapshot).is_none());
}

#[test]
fn credential_repair_revalidates_request_identity_and_the_actual_failed_route() {
    let mut snapshot = snapshot();
    let request = authentication_failure(&snapshot).unwrap();
    let route = FailedRoute {
        protocol: kuku::config::ProviderFormat::Anthropic,
        endpoint: "https://api.anthropic.com".parse().unwrap(),
        connection: "actual-failed-provider".into(),
        model: "actual-model".into(),
    };
    assert!(verify_failure(&snapshot, &request).is_ok());
    assert!(verify_route(&snapshot, &route).is_ok());
    let mut wrong = request.clone();
    wrong.request_id = "another-request".into();
    assert!(verify_failure(&snapshot, &wrong).is_err());
    snapshot.selection.as_mut().unwrap().connection = "later-default".into();
    assert!(verify_route(&snapshot, &route).is_err());
    snapshot = self::snapshot();
    snapshot.pending_selection = snapshot.selection.clone();
    snapshot.pending_selection.as_mut().unwrap().model = "later-model".into();
    assert!(verify_route(&snapshot, &route).is_err());
}

#[test]
fn credential_only_save_preserves_default_route_and_unrelated_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let mut document: DocumentMut = generate_default().parse().unwrap();
    document["unrelated"] = value("keep");
    document["provider"]["openai"]["api_key"] = value("$KUKU_REPAIR_OLD_ENV");
    let settings = settings(directory.path(), &document.to_string());
    let model = settings.document["model"].to_string();
    let default = settings.document["default_model"].to_string();
    let key = "fixture-repair-only-secret";
    save_credential(&settings, "openai", key).unwrap();
    let output = fs::read_to_string(&settings.config_path).unwrap();
    assert!(!output.contains(key));
    let saved: DocumentMut = output.parse().unwrap();
    assert_eq!(model, saved["model"].to_string());
    assert_eq!(default, saved["default_model"].to_string());
    assert_eq!("keep", saved["unrelated"].as_str().unwrap());
    assert_eq!(
        settings.document["provider"]["anthropic"].to_string(),
        saved["provider"]["anthropic"].to_string()
    );
    let resolved = load_config(&settings.config_path)
        .unwrap()
        .resolve()
        .unwrap();
    let ApiKey::File(path) = &resolved.providers["openai"].api_key else {
        panic!("private file reference expected");
    };
    assert!(kuku::util::is_valid_private_key(path));
    assert_eq!(key, resolved.providers["openai"].api_key.resolve().unwrap());
}

#[test]
fn credential_save_failure_keeps_original_bytes_and_removes_the_staged_secret() {
    let directory = tempfile::tempdir().unwrap();
    let settings = settings(directory.path(), generate_default());
    let concurrent = "concurrent settings change";
    fs::write(&settings.config_path, concurrent).unwrap();
    let permissions = fs::metadata(&settings.config_path).unwrap().permissions();
    let key = "fixture-repair-only-secret";
    let error = save_credential(&settings, "anthropic", key).unwrap_err();
    assert_eq!(
        concurrent,
        fs::read_to_string(&settings.config_path).unwrap()
    );
    assert_eq!(
        permissions,
        fs::metadata(&settings.config_path).unwrap().permissions()
    );
    assert_eq!(0, fs::read_dir(&settings.credentials_dir).unwrap().count());
    assert!(!error.to_string().contains(key));
}

#[test]
fn invalid_credential_never_creates_a_file_or_exposes_its_contents() {
    let directory = tempfile::tempdir().unwrap();
    let settings = settings(directory.path(), generate_default());
    let original = fs::read(&settings.config_path).unwrap();
    let key = "fixture-repair-only-secret\nsecond-line";
    let error = save_credential(&settings, "anthropic", key).unwrap_err();
    assert_eq!(original, fs::read(&settings.config_path).unwrap());
    assert!(!settings.credentials_dir.exists());
    assert!(!error.to_string().contains("fixture-repair-only-secret"));
}

#[tokio::test]
async fn remote_and_token_connections_read_the_failed_route_without_writing_local_settings() {
    let server = MockServer::start_async().await;
    let state = snapshot();
    let request = authentication_failure(&state).unwrap();
    let snapshot = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/sessions/session/snapshot")
                .query_param("progress", "true");
            then.status(200).json_body(json!({
                "session_id":state.session_id,"workspace":state.workspace,"cursor":state.cursor,
                "agent":state.agent,"agents":[],"pending":[],"selection":state.selection,
                "progress":state.progress,
            }));
        })
        .await;
    let events = server
        .mock_async(|when, then| {
            when.method(GET).path("/sessions/session/events").query_param("after", "6");
            then.status(200).json_body(json!({"events":[
            {"id":9,"payload":{"id":9,"kind":"runtime.step_prepared","agent":"main","turn":7,
             "request_id":"req-1","connection":"older-failed-route","model":"older-model","segment":7}},
            {"id":10,"payload":{"id":10,"kind":"runtime.step_prepared","agent":"main","turn":7,
             "request_id":"req-1","connection":"actual-failed-provider","model":"actual-model","segment":8}}
        ],"next_after":20}));
        })
        .await;
    let segment = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/sessions/session/events")
                .query_param("after", "7");
            then.status(200).json_body(json!({"events":[
            {"id":8,"payload":{"id":8,"kind":"runtime.context_segment","agent":"main",
             "connection":"actual-failed-provider","model":"actual-model",
             "protocol":"anthropic","endpoint":"https://api.anthropic.com"}}
        ],"next_after":8}));
        })
        .await;
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.toml");
    let original = "local config must not change";
    fs::write(&config, original).unwrap();
    let connection = Connection {
        client: kuku_client::Client::remote(&server.base_url(), "fixture-service-token").unwrap(),
        workspace: request.target.workspace.clone(),
        pending: kuku_client::PendingStore::open(&directory.path().join("pending")).unwrap(),
        local: None,
    };
    for remote in [true, false] {
        let options = ConnectionOptions {
            remote: remote.then(|| server.base_url()),
            token: Some("fixture-service-token".into()),
            data_dir: Some(directory.path().into()),
            config: Some(config.clone()),
            ..Default::default()
        };
        let result = prepare(&options, Some(&connection), request.clone())
            .await
            .unwrap();
        assert!(matches!(result, PreparedCredentialRepair::Remote(route)
            if route.connection == "actual-failed-provider" && route.model == "actual-model"));
        assert_eq!(original, fs::read_to_string(&config).unwrap());
        assert!(!directory.path().join("credentials").exists());
    }
    snapshot.assert_hits_async(2).await;
    events.assert_hits_async(2).await;
    segment.assert_hits_async(2).await;
}

#[test]
fn busy_service_keeps_the_saved_state_and_requires_an_explicit_restart_action() {
    use super::super::repair::{CredentialRepair, CredentialRepairAction};
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    let directory = tempfile::tempdir().unwrap();
    let app = crate::app::App::new(std::env::current_dir().unwrap(), None);
    let request = authentication_failure(&snapshot()).unwrap();
    let settings = settings(directory.path(), generate_default());
    let service = ServiceIdentity {
        endpoint: "http://127.0.0.1:1".into(),
        instance_id: "before-restart".into(),
        pid: 1,
        data_dir: directory.path().to_string_lossy().into_owned(),
        config_path: settings.config_path.clone(),
    };
    let mut repair = CredentialRepair::new(1, app.selection(), request.clone());
    repair.prepared(Ok(PreparedCredentialRepair::Local(Arc::new(
        CredentialRepairPlan {
            config_path: settings.config_path.clone(),
            settings,
            service: service.clone(),
            request,
            route: FailedRoute {
                protocol: kuku::config::ProviderFormat::Anthropic,
                endpoint: "https://api.anthropic.com".parse().unwrap(),
                connection: "anthropic".into(),
                model: "fixture-model".into(),
            },
        },
    ))));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(96, 24)).unwrap();
    terminal.draw(|frame| repair.render(frame)).unwrap();
    let enter = || Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    repair.handle(Event::Paste("fixture-new-credential".into()));
    assert!(matches!(
        repair.handle(enter()),
        CredentialRepairAction::None
    ));
    assert!(matches!(
        repair.handle(enter()),
        CredentialRepairAction::Save(_, _)
    ));
    let saved = SavedCredentialRepair {
        config_sha256: "pending".into(),
        service,
    };
    assert!(!repair.saved(Ok(CredentialRepairApplied::SavedNotApplied(
        saved,
        "Other work is active".into()
    ))));
    assert!(matches!(
        repair.handle(enter()),
        CredentialRepairAction::Restart(_)
    ));
}

#[test]
fn a_changed_saved_config_is_not_reported_as_applied() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("config.toml");
    fs::write(&config_path, "saved configuration").unwrap();
    let saved = SavedCredentialRepair {
        config_sha256: format!("{:x}", Sha256::digest(b"saved configuration")),
        service: ServiceIdentity {
            endpoint: "http://127.0.0.1:1111".into(),
            instance_id: "observed".into(),
            pid: 1,
            data_dir: directory.path().to_string_lossy().into_owned(),
            config_path: config_path.clone(),
        },
    };
    assert!(verify_saved_config(&saved).is_ok());
    fs::write(&config_path, "edited by another client").unwrap();
    assert!(verify_saved_config(&saved).is_err());
}

#[test]
fn the_same_connection_name_cannot_hide_a_changed_endpoint_or_protocol() {
    let directory = tempfile::tempdir().unwrap();
    let mut settings = settings(directory.path(), generate_default());
    let provider = &settings.resolved.providers["anthropic"];
    let route = FailedRoute {
        connection: "anthropic".into(),
        model: "fixture-model".into(),
        endpoint: provider.base_url.parse().unwrap(),
        protocol: provider.format,
    };
    assert!(verify_config_route(&settings, &route).is_ok());
    settings
        .resolved
        .providers
        .get_mut("anthropic")
        .unwrap()
        .base_url = "https://changed.invalid".into();
    assert!(verify_config_route(&settings, &route).is_err());
    settings
        .resolved
        .providers
        .get_mut("anthropic")
        .unwrap()
        .base_url = route.endpoint.as_str().into();
    settings
        .resolved
        .providers
        .get_mut("anthropic")
        .unwrap()
        .format = kuku::config::ProviderFormat::OpenAiResponses;
    assert!(verify_config_route(&settings, &route).is_err());
}
