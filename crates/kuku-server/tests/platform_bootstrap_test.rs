use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use kuku::config::SecretString;
use kuku_server::api::{
    ApiError, ApiVersion, AuthMode, CompleteInitRequest, ConnectionInfo, CredentialInput,
    CredentialSource, InitPhase, ProviderDraft, RegisterInitialWorkspaceRequest,
    RegisterWorkspaceRequest, TestProviderResult, TierDraft, UpdateDefaultTierRequest,
    UpdateProvidersRequest, WorkspaceId,
};
use kuku_server::platform::{
    derive_phase, router, AuthContext, BearerTokenStore, BootstrapService, ConfigPatch,
    ConfigService, OriginPolicy, PlatformServices, PlatformState, ProviderProbe,
    RegistrationRootRegistry, RegistrationRootSpec, ServerRevisionCoordinator, SettingsService,
    WorkspaceRegistry, WorkspaceUsagePort,
};

struct UnusedUsage;

impl WorkspaceUsagePort for UnusedUsage {
    fn has_durable_tasks<'a>(
        &'a self,
        _id: &'a WorkspaceId,
    ) -> Pin<Box<dyn Future<Output = Result<bool, ApiError>> + Send + 'a>> {
        Box::pin(async { Ok(false) })
    }
}

struct ProbeFixture {
    called: AtomicBool,
}

struct FailingProbe;

impl ProviderProbe for FailingProbe {
    fn probe<'a>(
        &'a self,
        _tier: &'a kuku_server::api::TierSummary,
    ) -> Pin<Box<dyn Future<Output = Result<TestProviderResult, ApiError>> + Send + 'a>> {
        Box::pin(async {
            Err(ApiError::new(
                kuku_server::api::ApiErrorCode::ProviderUnavailable,
                "upstream body contains provider-secret",
                "provider-fixture",
            )
            .with_details(serde_json::json!({ "body": "provider-secret" })))
        })
    }
}

impl ProviderProbe for ProbeFixture {
    fn probe<'a>(
        &'a self,
        tier: &'a kuku_server::api::TierSummary,
    ) -> Pin<Box<dyn Future<Output = Result<TestProviderResult, ApiError>> + Send + 'a>> {
        self.called.store(true, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async move {
            Ok(TestProviderResult {
                api_version: ApiVersion,
                reachable: true,
                provider: tier.provider.clone(),
                model: tier.model.clone(),
                message: None,
            })
        })
    }
}

async fn bootstrap_fixture(
    home: &std::path::Path,
    root: &std::path::Path,
    probe: Arc<ProbeFixture>,
) -> (
    Arc<BootstrapService>,
    Arc<ConfigService>,
    Arc<WorkspaceRegistry>,
    Arc<ServerRevisionCoordinator>,
) {
    let revisions = ServerRevisionCoordinator::open(home);
    let config = ConfigService::open(home.join("config.toml"), Arc::clone(&revisions))
        .await
        .unwrap();
    let roots = RegistrationRootRegistry::from_server_config(
        home,
        vec![RegistrationRootSpec {
            label: "Projects".to_owned(),
            path: root.to_owned(),
        }],
    )
    .unwrap();
    let workspaces =
        WorkspaceRegistry::open(home, roots, Arc::new(UnusedUsage), Arc::clone(&revisions))
            .unwrap();
    let bootstrap = BootstrapService::open(
        home,
        Arc::clone(&config),
        Arc::clone(&workspaces),
        probe,
        Arc::clone(&revisions),
    )
    .await
    .unwrap();
    (bootstrap, config, workspaces, revisions)
}

#[test]
fn phase_requires_every_server_owned_prerequisite() {
    assert_eq!(
        derive_phase(false, false, false, false, false),
        InitPhase::Required
    );
    assert_eq!(
        derive_phase(false, false, false, false, true),
        InitPhase::Required
    );
    assert_eq!(
        derive_phase(true, true, true, true, false),
        InitPhase::ProbePassed
    );
    assert_eq!(
        derive_phase(true, true, true, true, true),
        InitPhase::Complete
    );
}

#[tokio::test]
async fn init_probe_and_completion_survive_reopen_without_exposing_secrets() {
    let home = tempfile::tempdir().unwrap();
    let allowed = tempfile::tempdir().unwrap();
    std::fs::create_dir(allowed.path().join("project")).unwrap();
    let probe = Arc::new(ProbeFixture {
        called: AtomicBool::new(false),
    });
    let (bootstrap, config, workspaces, revisions) =
        bootstrap_fixture(home.path(), allowed.path(), Arc::clone(&probe)).await;
    assert_eq!(InitPhase::Required, bootstrap.status().await.phase);

    let expected_revision = bootstrap.status().await.server_revision;
    let status = bootstrap
        .update_providers(UpdateProvidersRequest {
            providers: vec![
                ProviderDraft {
                    provider_id: "direct".to_owned(),
                    format: "openai-responses".to_owned(),
                    base_url: "http://127.0.0.1:9000".to_owned(),
                    credential: CredentialInput::DirectValue(SecretString::new("$HOME")),
                },
                ProviderDraft {
                    provider_id: "environment".to_owned(),
                    format: "openai-responses".to_owned(),
                    base_url: "http://127.0.0.1:9001".to_owned(),
                    credential: CredentialInput::EnvironmentReference("HOME".to_owned()),
                },
            ],
            tiers: vec![TierDraft {
                tier_id: "custom".to_owned(),
                provider_id: "direct".to_owned(),
                model: "model-x".to_owned(),
                purpose: "General".to_owned(),
                think: Some("medium".to_owned()),
            }],
            expected_revision,
        })
        .await
        .unwrap();
    assert_eq!(InitPhase::ProvidersReady, status.phase);

    let status = bootstrap
        .update_default_tier(UpdateDefaultTierRequest {
            tier_id: "custom".to_owned(),
            expected_revision: status.server_revision,
        })
        .await
        .unwrap();
    assert_eq!(InitPhase::DefaultTierReady, status.phase);
    let root_id = bootstrap.registration_roots().items[0].root_id.clone();
    let status = bootstrap
        .register_initial_workspace(RegisterInitialWorkspaceRequest {
            workspace: RegisterWorkspaceRequest {
                root_id,
                relative_path: "project".to_owned(),
                label: "Project".to_owned(),
                expected_revision: status.server_revision,
            },
        })
        .await
        .unwrap();
    assert_eq!(InitPhase::WorkspaceReady, status.phase);
    let result = bootstrap
        .test_provider(kuku_server::api::TestProviderRequest {
            tier_id: "custom".to_owned(),
            expected_revision: status.server_revision,
        })
        .await
        .unwrap();
    assert!(result.reachable);
    let status = bootstrap.status().await;
    assert_eq!(InitPhase::ProbePassed, status.phase);
    let status = bootstrap
        .complete(CompleteInitRequest {
            expected_revision: status.server_revision,
        })
        .await
        .unwrap();
    assert_eq!(InitPhase::Complete, status.phase);

    let settings =
        SettingsService::open(home.path(), Arc::clone(&config), workspaces, revisions, 4)
            .await
            .unwrap();
    let current = settings.snapshot().await.unwrap();
    settings
        .commit(kuku_server::api::UpdateSettingsRequest {
            expected_revision: current.server_revision,
            patch: kuku_server::api::SettingsPatch {
                default_tier: None,
                default_workspace_id: None,
                max_concurrent_runs: None,
                discovery: Some(kuku_server::api::DiscoverySettings {
                    auto_discover: false,
                }),
            },
        })
        .await
        .unwrap();
    let status = bootstrap.status().await;
    assert!(status.complete);
    assert_eq!(InitPhase::Complete, status.phase);

    let catalog_json = serde_json::to_string(&config.catalog().await).unwrap();
    assert!(!catalog_json.contains("$HOME"));
    assert!(catalog_json.contains("environment_reference"));
    let identity = bootstrap.identity();
    drop(bootstrap);
    drop(config);

    let reopened_probe = Arc::new(ProbeFixture {
        called: AtomicBool::new(false),
    });
    let (reopened, reopened_config, _, _) =
        bootstrap_fixture(home.path(), allowed.path(), Arc::clone(&reopened_probe)).await;
    assert_eq!(identity, reopened.identity());
    assert_eq!(InitPhase::Complete, reopened.status().await.phase);
    let credentials = reopened_config.catalog().await.credentials;
    assert_eq!(
        CredentialSource::DirectValue,
        credentials[0].source.unwrap()
    );
    assert_eq!(
        CredentialSource::EnvironmentReference,
        credentials[1].source.unwrap()
    );
}

#[tokio::test]
async fn platform_router_exposes_canonical_paths_without_a_credential_endpoint() {
    let home = tempfile::tempdir().unwrap();
    let allowed = tempfile::tempdir().unwrap();
    std::fs::create_dir(allowed.path().join("project")).unwrap();
    let probe = Arc::new(ProbeFixture {
        called: AtomicBool::new(false),
    });
    let (bootstrap, config, workspaces, revisions) =
        bootstrap_fixture(home.path(), allowed.path(), probe).await;
    let settings = SettingsService::open(
        home.path(),
        Arc::clone(&config),
        Arc::clone(&workspaces),
        revisions,
        4,
    )
    .await
    .unwrap();
    let auth = BearerTokenStore::open(home.path(), None).unwrap();
    let credential = auth.expose_for_terminal().to_owned();
    let identity = bootstrap.identity();
    let state = Arc::new(PlatformServices {
        bootstrap,
        config,
        settings,
        workspaces,
        auth,
        origin_policy: Arc::new(
            OriginPolicy::new(vec!["http://127.0.0.1:3000".to_owned()]).unwrap(),
        ),
        connection: ConnectionInfo {
            server_id: identity.server_id,
            display_name: identity.display_name,
            preferred_origin: "http://127.0.0.1:3000".to_owned(),
            local_url: "http://127.0.0.1:3000".to_owned(),
            lan_url: None,
            plaintext: true,
        },
    });
    let app = router(state).layer(axum::Extension(AuthContext {
        authenticated: true,
        mode: AuthMode::Bearer,
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let base = format!("http://{address}");

    let status = wreq::get(format!("{base}/status")).send().await.unwrap();
    assert_eq!(200, status.status());
    let status_body = status.text().await.unwrap();
    assert!(!status_body.contains(&credential));
    let roots = wreq::get(format!("{base}/registration-roots"))
        .send()
        .await
        .unwrap();
    assert_eq!(200, roots.status());
    let verify = wreq::get(format!("{base}/auth/verify"))
        .send()
        .await
        .unwrap();
    assert_eq!(404, verify.status());
    server.abort();
}

#[tokio::test]
async fn failed_probe_is_canonical_and_redacted() {
    let home = tempfile::tempdir().unwrap();
    let allowed = tempfile::tempdir().unwrap();
    std::fs::create_dir(allowed.path().join("project")).unwrap();
    let revisions = ServerRevisionCoordinator::open(home.path());
    let config = ConfigService::open(home.path().join("config.toml"), Arc::clone(&revisions))
        .await
        .unwrap();
    let raw = valid_config();
    let expected = config.revision().await.unwrap();
    config
        .commit_config(ConfigPatch::replace(raw), expected)
        .await
        .unwrap();
    let roots = RegistrationRootRegistry::from_server_config(
        home.path(),
        vec![RegistrationRootSpec {
            label: "Projects".to_owned(),
            path: allowed.path().to_owned(),
        }],
    )
    .unwrap();
    let workspaces = WorkspaceRegistry::open(
        home.path(),
        roots,
        Arc::new(UnusedUsage),
        Arc::clone(&revisions),
    )
    .unwrap();
    let bootstrap = BootstrapService::open(
        home.path(),
        config,
        workspaces,
        Arc::new(FailingProbe),
        revisions,
    )
    .await
    .unwrap();
    let error = bootstrap
        .test_provider(kuku_server::api::TestProviderRequest {
            tier_id: "custom".to_owned(),
            expected_revision: bootstrap.status().await.server_revision,
        })
        .await
        .unwrap_err();
    assert_eq!(
        kuku_server::api::ApiErrorCode::ProviderUnavailable,
        error.code()
    );
    let encoded = serde_json::to_string(&error).unwrap();
    assert!(!encoded.contains("provider-secret"));
    assert!(error.details.is_none());
}

#[tokio::test]
async fn stale_init_revision_wins_before_provider_validation() {
    let home = tempfile::tempdir().unwrap();
    let allowed = tempfile::tempdir().unwrap();
    let probe = Arc::new(ProbeFixture {
        called: AtomicBool::new(false),
    });
    let (bootstrap, _, _, _) =
        bootstrap_fixture(home.path(), allowed.path(), Arc::clone(&probe)).await;
    let stale = bootstrap.status().await.server_revision;
    let current = bootstrap
        .update_providers(UpdateProvidersRequest {
            providers: vec![ProviderDraft {
                provider_id: "local".to_owned(),
                format: "openai-responses".to_owned(),
                base_url: "http://127.0.0.1:9000".to_owned(),
                credential: CredentialInput::DirectValue(SecretString::new("key")),
            }],
            tiers: vec![TierDraft {
                tier_id: "custom".to_owned(),
                provider_id: "local".to_owned(),
                model: "model-x".to_owned(),
                purpose: "General".to_owned(),
                think: None,
            }],
            expected_revision: stale.clone(),
        })
        .await
        .unwrap();
    assert_ne!(stale, current.server_revision);

    let error = bootstrap
        .update_providers(UpdateProvidersRequest {
            providers: vec![ProviderDraft {
                provider_id: "local".to_owned(),
                format: "not-a-format".to_owned(),
                base_url: String::new(),
                credential: CredentialInput::DirectValue(SecretString::new("never-log-this")),
            }],
            tiers: Vec::new(),
            expected_revision: stale,
        })
        .await
        .unwrap_err();
    assert_eq!(
        kuku_server::api::ApiErrorCode::StaleServerRevision,
        error.code()
    );
    assert!(!serde_json::to_string(&error)
        .unwrap()
        .contains("never-log-this"));
}

#[tokio::test]
async fn settings_commit_updates_all_domains_under_one_revision() {
    let home = tempfile::tempdir().unwrap();
    let allowed = tempfile::tempdir().unwrap();
    std::fs::create_dir(allowed.path().join("first")).unwrap();
    std::fs::create_dir(allowed.path().join("second")).unwrap();
    let revisions = ServerRevisionCoordinator::open(home.path());
    let config = ConfigService::open(home.path().join("config.toml"), Arc::clone(&revisions))
        .await
        .unwrap();
    let initial = kuku::config::parse_config_file(
        r#"default_model = "first"
[model.first]
provider = "local"
model = "model-a"
[model.second]
provider = "local"
model = "model-b"
[provider.local]
format = "openai-responses"
base_url = "http://127.0.0.1:9000"
credential = { source = "direct_value", value = "settings-secret" }
"#,
    )
    .unwrap();
    let expected = config.revision().await.unwrap();
    config
        .commit_config(ConfigPatch::replace(initial), expected)
        .await
        .unwrap();
    let roots = RegistrationRootRegistry::from_server_config(
        home.path(),
        vec![RegistrationRootSpec {
            label: "Projects".to_owned(),
            path: allowed.path().to_owned(),
        }],
    )
    .unwrap();
    let workspaces = WorkspaceRegistry::open(
        home.path(),
        roots,
        Arc::new(UnusedUsage),
        Arc::clone(&revisions),
    )
    .unwrap();
    let root_id = workspaces.registration_roots().list()[0].root_id.clone();
    let first = workspaces
        .register(RegisterWorkspaceRequest {
            root_id: root_id.clone(),
            relative_path: "first".to_owned(),
            label: "First".to_owned(),
            expected_revision: workspaces.revision().await.unwrap(),
        })
        .await
        .unwrap();
    let second = workspaces
        .register(RegisterWorkspaceRequest {
            root_id,
            relative_path: "second".to_owned(),
            label: "Second".to_owned(),
            expected_revision: workspaces.revision().await.unwrap(),
        })
        .await
        .unwrap();
    assert!(first.is_default);

    let settings = SettingsService::open(
        home.path(),
        Arc::clone(&config),
        Arc::clone(&workspaces),
        Arc::clone(&revisions),
        4,
    )
    .await
    .unwrap();
    let stale = settings.snapshot().await.unwrap().server_revision;
    let snapshot = settings
        .commit(kuku_server::api::UpdateSettingsRequest {
            expected_revision: stale.clone(),
            patch: kuku_server::api::SettingsPatch {
                default_tier: Some("second".to_owned()),
                default_workspace_id: Some(second.workspace_id.clone()),
                max_concurrent_runs: Some(7),
                discovery: Some(kuku_server::api::DiscoverySettings {
                    auto_discover: false,
                }),
            },
        })
        .await
        .unwrap();
    assert_eq!("second", snapshot.default_tier);
    assert_eq!(Some(second.workspace_id), snapshot.default_workspace_id);
    assert_eq!(7, snapshot.max_concurrent_runs);
    assert!(!snapshot.discovery.auto_discover);
    assert!(
        !config
            .snapshot()
            .await
            .unwrap()
            .raw
            .unwrap()
            .discovery
            .unwrap()
            .auto_discover
    );
    assert!(!home.path().join("settings.journal.json").exists());
    assert!(!serde_json::to_string(&snapshot)
        .unwrap()
        .contains("settings-secret"));

    let error = settings
        .commit(kuku_server::api::UpdateSettingsRequest {
            expected_revision: stale,
            patch: kuku_server::api::SettingsPatch {
                default_tier: Some("first".to_owned()),
                default_workspace_id: None,
                max_concurrent_runs: None,
                discovery: None,
            },
        })
        .await
        .unwrap_err();
    assert_eq!(
        kuku_server::api::ApiErrorCode::StaleServerRevision,
        error.code()
    );
}

#[tokio::test]
async fn formatting_only_config_reload_preserves_probe_input_revision() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("config.toml");
    std::fs::write(
        &path,
        r#"default_model="custom"
[provider.local]
credential={source="direct_value",value="key"}
base_url="http://127.0.0.1:9000"
format="openai-responses"
[model.custom]
model="model-x"
provider="local"
"#,
    )
    .unwrap();
    let revisions = ServerRevisionCoordinator::open(home.path());
    let service = ConfigService::open(path.clone(), Arc::clone(&revisions))
        .await
        .unwrap();
    let before = revisions.probe_inputs().await.unwrap();
    let mut raw = service.snapshot().await.unwrap().raw.unwrap();
    let tier = raw.model.get_mut("custom").unwrap();
    tier.think = Some("medium".to_owned());
    tier.context_window = Some(200_000);
    tier.max_output_tokens = Some(48_000);
    tier.purpose = Some("custom".to_owned());
    raw.discovery = Some(kuku::config::DiscoveryConfig::default());
    raw.handoff = Some(kuku::config::HandoffConfig::default());
    raw.logs = Some(kuku::config::LogsConfig::default());
    raw.plugin = Some(kuku::config::PluginConfig::default());
    raw.update = Some(kuku::config::UpdateConfig::default());
    std::fs::write(&path, toml::to_string_pretty(&raw).unwrap()).unwrap();
    service.reload_from_disk().await.unwrap();
    let after = revisions.probe_inputs().await.unwrap();
    assert_eq!(before, after);
}

#[test]
fn settings_recovery_rolls_every_partial_transaction_forward() {
    let intended_config = "default_model = \"recovered\"\nsecret = \"journal-secret\"\n";
    let intended_workspaces = r#"{"format_version":1,"default_workspace_id":null,"records":[]}"#;
    let intended_settings = r#"{"format_version":1,"max_concurrent_runs":9}"#;
    let digest = |value: &str| {
        kuku_server::platform::accepted_digest(value.as_bytes())
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    for applied in 0..=3 {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("config.toml"), "old-config").unwrap();
        std::fs::write(home.path().join("workspaces.json"), "old-workspaces").unwrap();
        std::fs::write(home.path().join("settings.json"), "old-settings").unwrap();
        if applied >= 1 {
            std::fs::write(home.path().join("config.toml"), intended_config).unwrap();
        }
        if applied >= 2 {
            std::fs::write(home.path().join("workspaces.json"), intended_workspaces).unwrap();
        }
        if applied >= 3 {
            std::fs::write(home.path().join("settings.json"), intended_settings).unwrap();
        }
        let journal = serde_json::json!({
            "format_version": 1,
            "config_toml": intended_config,
            "config_digest": digest(intended_config),
            "workspaces_json": intended_workspaces,
            "workspaces_digest": digest(intended_workspaces),
            "settings_json": intended_settings,
            "settings_digest": digest(intended_settings),
        });
        std::fs::write(
            home.path().join("settings.journal.json"),
            serde_json::to_vec(&journal).unwrap(),
        )
        .unwrap();

        SettingsService::recover(home.path()).unwrap();
        assert_eq!(
            intended_config,
            std::fs::read_to_string(home.path().join("config.toml")).unwrap()
        );
        assert_eq!(
            intended_workspaces,
            std::fs::read_to_string(home.path().join("workspaces.json")).unwrap()
        );
        assert_eq!(
            intended_settings,
            std::fs::read_to_string(home.path().join("settings.json")).unwrap()
        );
        assert!(!home.path().join("settings.journal.json").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for name in ["config.toml", "workspaces.json", "settings.json"] {
                assert_eq!(
                    0o600,
                    std::fs::metadata(home.path().join(name))
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777
                );
            }
        }
    }
}

fn valid_config() -> kuku::config::ConfigFile {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"default_model = "custom"

[model.custom]
provider = "local"
model = "model-x"

[provider.local]
format = "openai-responses"
base_url = "http://127.0.0.1:9000"
credential = { source = "direct_value", value = "key" }
"#,
    )
    .unwrap();
    kuku::config::load_config(&path).unwrap()
}

#[tokio::test]
async fn missing_config_is_state_not_startup_failure() {
    let home = tempfile::tempdir().unwrap();
    let revisions = ServerRevisionCoordinator::open(home.path());
    let service = ConfigService::open(home.path().join("config.toml"), revisions)
        .await
        .unwrap();
    assert_eq!(service.state().await, PlatformState::Missing);
    let revision = service.revision().await.unwrap();
    assert_eq!(revision.as_str().len(), 64);
    assert!(revision
        .as_str()
        .bytes()
        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
}

#[tokio::test]
async fn invalid_startup_uses_stable_invalid_revision_sentinel() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("config.toml");
    std::fs::write(&path, "not valid [[[\n").unwrap();
    let revisions = ServerRevisionCoordinator::open(home.path());
    let service = ConfigService::open(path.clone(), revisions).await.unwrap();
    let first = service.revision().await.unwrap();
    std::fs::write(&path, "still invalid [[\n").unwrap();
    service.reload_from_disk().await.unwrap();
    let second = service.revision().await.unwrap();
    assert_eq!(first, second);
}

#[tokio::test]
async fn stale_revision_returns_server_revision_conflict() {
    let home = tempfile::tempdir().unwrap();
    let revisions = ServerRevisionCoordinator::open(home.path());
    let service = ConfigService::open(home.path().join("config.toml"), Arc::clone(&revisions))
        .await
        .unwrap();
    let stale = service.revision().await.unwrap();
    let _ = service
        .commit_config(ConfigPatch::replace(valid_config()), stale.clone())
        .await
        .unwrap();
    let error = service
        .commit_config(ConfigPatch::replace(valid_config()), stale)
        .await
        .unwrap_err();
    assert_eq!(
        error.code(),
        kuku_server::api::ApiErrorCode::StaleServerRevision
    );
}

#[tokio::test]
async fn deleting_config_before_commit_is_a_disk_conflict() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("config.toml");
    std::fs::write(
        &path,
        r#"default_model = "custom"
[model.custom]
provider = "local"
model = "model-x"
[provider.local]
format = "openai-responses"
base_url = "http://127.0.0.1:9000"
credential = { source = "direct_value", value = "key" }
"#,
    )
    .unwrap();
    let revisions = ServerRevisionCoordinator::open(home.path());
    let service = ConfigService::open(path.clone(), revisions).await.unwrap();
    let expected = service.revision().await.unwrap();
    std::fs::remove_file(path).unwrap();
    let error = service
        .commit_config(ConfigPatch::replace(valid_config()), expected)
        .await
        .unwrap_err();
    assert_eq!(error.code(), kuku_server::api::ApiErrorCode::Outdated);
}

#[tokio::test]
async fn creating_config_after_missing_startup_is_a_disk_conflict() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("config.toml");
    let revisions = ServerRevisionCoordinator::open(home.path());
    let service = ConfigService::open(path.clone(), revisions).await.unwrap();
    let expected = service.revision().await.unwrap();
    std::fs::write(&path, "external = true\n").unwrap();
    let error = service
        .commit_config(ConfigPatch::replace(valid_config()), expected)
        .await
        .unwrap_err();
    assert_eq!(error.code(), kuku_server::api::ApiErrorCode::Outdated);
}

#[tokio::test]
async fn valid_reload_replaces_snapshot_by_content_digest() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("config.toml");
    std::fs::write(
        &path,
        r#"default_model = "custom"
[model.custom]
provider = "local"
model = "model-x"
[provider.local]
format = "openai-responses"
base_url = "http://127.0.0.1:9000"
credential = { source = "direct_value", value = "key" }
"#,
    )
    .unwrap();
    let revisions = ServerRevisionCoordinator::open(home.path());
    let service = ConfigService::open(path.clone(), revisions).await.unwrap();
    std::fs::write(
        &path,
        r#"default_model = "next"
[model.next]
provider = "local"
model = "model-y"
[provider.local]
format = "openai-responses"
base_url = "http://127.0.0.1:9000"
credential = { source = "direct_value", value = "key" }
"#,
    )
    .unwrap();
    service.reload_from_disk().await.unwrap();
    assert_eq!(
        service
            .snapshot()
            .await
            .unwrap()
            .resolved
            .unwrap()
            .default_tier(),
        "next"
    );
}

#[tokio::test]
async fn valid_reload_observes_external_deletion() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("config.toml");
    std::fs::write(
        &path,
        r#"default_model = "custom"
[model.custom]
provider = "local"
model = "model-x"
[provider.local]
format = "openai-responses"
base_url = "http://127.0.0.1:9000"
credential = { source = "direct_value", value = "key" }
"#,
    )
    .unwrap();
    let revisions = ServerRevisionCoordinator::open(home.path());
    let service = ConfigService::open(path.clone(), revisions).await.unwrap();
    std::fs::remove_file(path).unwrap();
    assert_eq!(
        service.reload_from_disk().await.unwrap(),
        PlatformState::Missing
    );
    assert!(service.snapshot().await.unwrap().resolved.is_none());
}

#[tokio::test]
async fn watcher_detects_replacement_with_preserved_mtime() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("config.toml");
    std::fs::write(
        &path,
        r#"default_model = "custom"
[model.custom]
provider = "local"
model = "model-x"
[provider.local]
format = "openai-responses"
base_url = "http://127.0.0.1:9000"
credential = { source = "direct_value", value = "key" }
"#,
    )
    .unwrap();
    let original_mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
    let revisions = ServerRevisionCoordinator::open(home.path());
    let service = ConfigService::open(path.clone(), revisions).await.unwrap();
    let watcher =
        kuku_server::config_watcher::ConfigWatcherHandle::start(path.clone(), service.clone());
    std::fs::write(
        &path,
        r#"default_model = "next"
[model.next]
provider = "local"
model = "model-y"
[provider.local]
format = "openai-responses"
base_url = "http://127.0.0.1:9000"
credential = { source = "direct_value", value = "key" }
"#,
    )
    .unwrap();
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(original_mtime))
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(3200)).await;
    assert_eq!(
        service
            .snapshot()
            .await
            .unwrap()
            .resolved
            .unwrap()
            .default_tier(),
        "next"
    );
    watcher.shutdown().await;
}

#[tokio::test]
async fn concurrent_commit_and_reload_never_install_the_stale_snapshot() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("config.toml");
    std::fs::write(
        &path,
        r#"default_model = "custom"
[model.custom]
provider = "local"
model = "model-x"
[provider.local]
format = "openai-responses"
base_url = "http://127.0.0.1:9000"
credential = { source = "direct_value", value = "key" }
"#,
    )
    .unwrap();
    let revisions = ServerRevisionCoordinator::open(home.path());
    let service = ConfigService::open(path.clone(), revisions).await.unwrap();
    let expected = service.revision().await.unwrap();
    std::fs::write(
        &path,
        r#"default_model = "external"
[model.external]
provider = "local"
model = "model-z"
[provider.local]
format = "openai-responses"
base_url = "http://127.0.0.1:9000"
credential = { source = "direct_value", value = "key" }
"#,
    )
    .unwrap();
    let (reload, commit) = tokio::join!(
        service.reload_from_disk(),
        service.commit_config(ConfigPatch::replace(valid_config()), expected)
    );
    assert!(reload.is_ok());
    assert!(commit.is_err());
    assert_eq!(
        service
            .snapshot()
            .await
            .unwrap()
            .resolved
            .unwrap()
            .default_tier(),
        "external"
    );
}

#[tokio::test]
async fn invalid_reload_keeps_last_good_snapshot() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("config.toml");
    std::fs::write(
        &path,
        r#"default_model = "custom"
[model.custom]
provider = "local"
model = "model-x"
[provider.local]
format = "openai-responses"
base_url = "http://127.0.0.1:9000"
credential = { source = "direct_value", value = "key" }
"#,
    )
    .unwrap();
    let revisions = ServerRevisionCoordinator::open(home.path());
    let service = ConfigService::open(path.clone(), revisions).await.unwrap();
    assert_eq!(service.state().await, PlatformState::Ready);
    std::fs::write(&path, "default_model = [").unwrap();
    assert!(matches!(
        service.reload_from_disk().await.unwrap(),
        PlatformState::Invalid { .. }
    ));
    assert!(service.snapshot().await.unwrap().resolved.is_some());
}

#[tokio::test]
async fn watcher_shutdown_is_sticky_and_joined() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("config.toml");
    let revisions = ServerRevisionCoordinator::open(home.path());
    let service = ConfigService::open(path.clone(), revisions).await.unwrap();
    let watcher = kuku_server::config_watcher::ConfigWatcherHandle::start(path, service);
    watcher.shutdown().await;
}

#[tokio::test]
async fn watcher_does_not_reload_after_shutdown() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("config.toml");
    std::fs::write(
        &path,
        r#"default_model = "custom"
[model.custom]
provider = "local"
model = "model-x"
[provider.local]
format = "openai-responses"
base_url = "http://127.0.0.1:9000"
credential = { source = "direct_value", value = "key" }
"#,
    )
    .unwrap();
    let revisions = ServerRevisionCoordinator::open(home.path());
    let service = ConfigService::open(path.clone(), revisions).await.unwrap();
    let watcher =
        kuku_server::config_watcher::ConfigWatcherHandle::start(path.clone(), service.clone());
    watcher.shutdown().await;
    std::fs::write(path, "default_model = [").unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert_eq!(service.state().await, PlatformState::Ready);
}
