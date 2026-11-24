use std::collections::BTreeMap;
use std::env::VarError;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::state::{SetupState, valid_env_name, valid_stored_key};
use super::{ConnectionChoice, CredentialSource, ProviderPreset};
use kuku::adapter::{SubscriptionAccountInfo, inspect_model, validate_model_config};
use kuku::config::{
    ApiKey, Config, ConfigFile, ModelConfig, ModelRef, ProviderConfig, ProviderFormat,
    ProviderProfile, ProviderProxy, RequestedOutput, SubscriptionChannel, ThinkLevel,
    parse_config_text_with_env, publish_config,
};
use kuku::session::AgentConfig;
use kuku::util::{is_valid_private_key, private_directory};
use kuku_client::ConnectionOptions;
use toml_edit::{DocumentMut, InlineTable, Item, Table, Value, value};

mod credential;

use credential::stage_key;
#[cfg(test)]
use credential::stage_key_with_sync;

type EnvironmentLookup<'a> = &'a (dyn Fn(&str) -> Result<String, VarError> + Sync);

pub(super) struct Settings {
    pub(super) config_path: PathBuf,
    pub(super) data_dir: PathBuf,
    pub(super) credentials_dir: PathBuf,
    pub(super) document: DocumentMut,
    pub(super) original_bytes: Option<Vec<u8>>,
    pub(super) original_permissions: Option<fs::Permissions>,
    pub(super) raw: ConfigFile,
    pub(super) resolved: Option<Config>,
    draft_providers: BTreeMap<String, ProviderConfig>,
    pub(super) existing: bool,
    pub(super) subscription_info: BTreeMap<String, SubscriptionAccountInfo>,
    storage_error: Option<&'static str>,
}

impl Settings {
    pub(super) async fn load(options: ConnectionOptions) -> io::Result<Self> {
        let environment = |name: &str| std::env::var(name);
        Self::load_with_lookup(options, &environment).await
    }

    async fn load_with_lookup(
        options: ConnectionOptions,
        environment: EnvironmentLookup<'_>,
    ) -> io::Result<Self> {
        let service = options
            .local_service()
            .map_err(|_| invalid_config("cannot resolve local service paths"))?;
        let active = service
            .status()
            .await
            .map_err(|_| invalid_config("cannot check the local service status"))?;
        let active_config = active
            .as_ref()
            .and_then(|connection| connection.status().config_path.as_deref())
            .map(PathBuf::from);
        Self::load_with_status(options, active_config, environment)
    }

    pub(super) async fn load_for_first_run(options: ConnectionOptions) -> io::Result<Option<Self>> {
        let environment = |name: &str| std::env::var(name);
        Self::load_for_first_run_with_lookup(options, &environment).await
    }

    async fn load_for_first_run_with_lookup(
        options: ConnectionOptions,
        environment: EnvironmentLookup<'_>,
    ) -> io::Result<Option<Self>> {
        let Ok(service) = options.local_service() else {
            return Ok(None);
        };
        let status = match service.status().await {
            Ok(active) => Ok(active.is_some()),
            Err(_) => Err(()),
        };
        if !super::service_is_idle(status) {
            return Ok(None);
        }
        let settings = Self::load_with_status(options, None, environment)?;
        if super::should_offer_setup(
            Ok(false),
            settings.existing,
            settings.default_route_credential_available(),
        ) {
            Ok(Some(settings))
        } else {
            Ok(None)
        }
    }

    pub(super) fn load_with_status(
        options: ConnectionOptions,
        active_config: Option<PathBuf>,
        environment: EnvironmentLookup<'_>,
    ) -> io::Result<Self> {
        let service = options
            .local_service()
            .map_err(|_| invalid_config("cannot resolve local service paths"))?;
        if options.config.is_some()
            && active_config
                .as_deref()
                .is_some_and(|path| path != service.config_path())
        {
            return Err(invalid_config(
                "--config conflicts with the configuration used by the active service",
            ));
        }
        let config_path = active_config.unwrap_or_else(|| service.config_path().to_owned());
        let data_dir = service.data_dir().to_owned();
        Self::at_paths(config_path, data_dir, environment)
    }

    pub(super) fn reload(&self) -> io::Result<Self> {
        Self::at_paths(self.config_path.clone(), self.data_dir.clone(), &|name| {
            std::env::var(name)
        })
    }

    fn at_paths(
        config_path: PathBuf,
        data_dir: PathBuf,
        environment: EnvironmentLookup<'_>,
    ) -> io::Result<Self> {
        let credentials_dir = data_dir.join("credentials");
        let loaded = load_document(&config_path, &data_dir, environment)?;
        let mut settings = Self {
            config_path,
            data_dir,
            credentials_dir,
            document: loaded.document,
            original_bytes: loaded.original_bytes,
            original_permissions: loaded.original_permissions,
            raw: loaded.raw,
            resolved: loaded.resolved,
            draft_providers: loaded.draft_providers,
            existing: loaded.existing,
            subscription_info: BTreeMap::new(),
            storage_error: None,
        };
        settings.subscription_info = super::subscription::account_info(settings.providers());
        settings.storage_error = settings.check_storage(false).err().map(|_| {
            "Stored keys are unavailable because the credentials directory is not safe; use Environment or a different --data-dir."
        });
        Ok(settings)
    }

    pub(super) fn connection_choices(&self) -> Vec<ConnectionChoice> {
        let mut choices = vec![ConnectionChoice::Custom];
        choices.extend(
            self.providers()
                .keys()
                .cloned()
                .map(ConnectionChoice::Existing),
        );
        choices.extend(
            ProviderPreset::ALL
                .into_iter()
                .map(ConnectionChoice::Preset),
        );
        choices.push(ConnectionChoice::Subscription(SubscriptionChannel::ChatGpt));
        choices
    }

    pub(super) fn providers(&self) -> &BTreeMap<String, ProviderConfig> {
        self.resolved
            .as_ref()
            .map_or(&self.draft_providers, |config| &config.providers)
    }

    pub(super) fn provider(&self, connection: &str) -> Option<&ProviderConfig> {
        self.providers().get(connection)
    }

    pub(super) fn default_model(&self) -> Option<&ModelRef> {
        self.resolved.as_ref().map(|config| &config.default_model)
    }

    pub(super) fn model(&self, model: &ModelRef) -> Option<&ModelConfig> {
        self.resolved.as_ref()?.model(model)
    }

    fn registered(&self, model: &ModelRef) -> bool {
        self.raw
            .model
            .get(&model.connection)
            .is_some_and(|models| models.contains_key(&model.model))
    }

    pub(super) fn protocol_for(&self, choice: &ConnectionChoice) -> ProviderFormat {
        match choice {
            ConnectionChoice::Existing(name) => self.providers()[name].format,
            ConnectionChoice::Preset(preset) => preset.format(),
            ConnectionChoice::Subscription(SubscriptionChannel::ChatGpt) => {
                ProviderFormat::OpenAiResponses
            }
            ConnectionChoice::Custom => ProviderFormat::OpenAiChat,
        }
    }

    pub(super) fn address_for(&self, choice: &ConnectionChoice) -> (String, bool, String) {
        if let ConnectionChoice::Existing(name) = choice {
            let config = &self.providers()[name];
            return (
                config
                    .options
                    .endpoint_url
                    .clone()
                    .unwrap_or_else(|| config.base_url.clone()),
                config.options.endpoint_url.is_some(),
                config.options.models_url.clone().unwrap_or_default(),
            );
        }
        (String::new(), false, String::new())
    }

    pub(super) fn initial_choice(&self) -> ConnectionChoice {
        self.default_model()
            .map_or(ConnectionChoice::Custom, |model| {
                ConnectionChoice::Existing(model.connection.clone())
            })
    }

    pub(super) fn choice_label(&self, choice: &ConnectionChoice) -> String {
        match choice {
            ConnectionChoice::Existing(name) => {
                if let Some(account) = self.subscription_for(choice) {
                    let info = self.subscription_info.get(name);
                    return kuku::adapter::subscription_display_label(account, info);
                }
                format!(
                    "Existing · {}",
                    super::connection_name::existing(self, name)
                )
            }
            ConnectionChoice::Preset(preset) => format!("Preset · {}", preset.label()),
            ConnectionChoice::Subscription(SubscriptionChannel::ChatGpt) => {
                "Preset · ChatGPT subscription".into()
            }
            ConnectionChoice::Custom => "New custom connection".into(),
        }
    }

    pub(super) fn model_for(&self, choice: &ConnectionChoice) -> String {
        match choice {
            ConnectionChoice::Preset(preset) => preset.default_model().into(),
            ConnectionChoice::Custom | ConnectionChoice::Subscription(_) => String::new(),
            ConnectionChoice::Existing(name) => {
                self.resolved.as_ref().map_or_else(String::new, |config| {
                    let default = &config.default_model;
                    if default.connection == *name {
                        default.model.clone()
                    } else {
                        config
                            .models
                            .keys()
                            .find(|model| model.connection == *name)
                            .map_or_else(String::new, |model| model.model.clone())
                    }
                })
            }
        }
    }

    pub(super) fn environment_for(&self, choice: &ConnectionChoice) -> String {
        match self.key_for(choice) {
            Some(ApiKey::Env(name)) => name.clone(),
            _ => String::new(),
        }
    }

    pub(super) fn source_for(&self, choice: &ConnectionChoice) -> CredentialSource {
        let key = self.key_for(choice);
        preferred_source(key, self.can_store_key())
    }

    pub(super) fn saved_key_available(&self, choice: &ConnectionChoice) -> bool {
        match self.key_for(choice) {
            Some(ApiKey::Plaintext(value)) => !value.trim().is_empty(),
            Some(ApiKey::File(path)) => is_valid_private_key(path),
            _ => false,
        }
    }

    pub(super) fn default_route_credential_available(&self) -> bool {
        self.default_route_credential_available_with(|key| key.resolve().ok())
    }

    fn default_route_credential_available_with(
        &self,
        resolve: impl FnOnce(&ApiKey) -> Option<String>,
    ) -> bool {
        let Some(model) = self.default_model() else {
            return false;
        };
        let Some(provider) = self.provider(&model.connection) else {
            return false;
        };
        if provider.options.subscription.is_some() {
            self.subscription_available(&ConnectionChoice::Existing(model.connection.clone()))
        } else {
            resolve(&provider.api_key).is_some_and(|value| !value.trim().is_empty())
        }
    }

    pub(super) fn key_for(&self, choice: &ConnectionChoice) -> Option<&ApiKey> {
        match choice {
            ConnectionChoice::Existing(name) => self.provider(name).map(|p| &p.api_key),
            ConnectionChoice::Preset(_)
            | ConnectionChoice::Subscription(_)
            | ConnectionChoice::Custom => None,
        }
    }

    pub(super) fn can_store_key(&self) -> bool {
        self.storage_error.is_none()
    }

    pub(super) fn storage_problem(&self) -> &'static str {
        self.storage_error
            .unwrap_or("Stored keys are unavailable; use the Environment source instead.")
    }

    fn check_storage(&self, create: bool) -> io::Result<()> {
        let candidate = &self.credentials_dir;
        match fs::symlink_metadata(candidate) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if !create {
                    return Ok(());
                }
            }
            _ => return Err(unsafe_storage()),
        }
        private_directory(candidate).map_err(|_| unsafe_storage())
    }
}

fn preferred_source(key: Option<&ApiKey>, can_store_key: bool) -> CredentialSource {
    match key {
        Some(ApiKey::File(_) | ApiKey::Plaintext(_)) => CredentialSource::StoredKey,
        Some(ApiKey::Env(_)) => CredentialSource::Environment,
        _ if can_store_key => CredentialSource::StoredKey,
        _ => CredentialSource::Environment,
    }
}

struct LoadedDocument {
    document: DocumentMut,
    original_bytes: Option<Vec<u8>>,
    original_permissions: Option<fs::Permissions>,
    raw: ConfigFile,
    resolved: Option<Config>,
    draft_providers: BTreeMap<String, ProviderConfig>,
    existing: bool,
}

pub(super) fn parse_service_config(
    raw: &str,
    data_dir: &Path,
    environment: EnvironmentLookup<'_>,
) -> Result<ConfigFile, kuku::Error> {
    parse_config_text_with_env(raw, |name| {
        if name == "KUKU_HOME" {
            data_dir
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| VarError::NotUnicode(data_dir.as_os_str().to_os_string()))
        } else {
            environment(name)
        }
    })
}

fn load_document(
    path: &Path,
    data_dir: &Path,
    environment: EnvironmentLookup<'_>,
) -> io::Result<LoadedDocument> {
    let original_bytes = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            Some(fs::read(path).map_err(|_| invalid_config("cannot read the configuration file"))?)
        }
        Ok(_) => {
            return Err(invalid_config(
                "the configuration path is not a regular file",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(_) => return Err(invalid_config("cannot inspect the configuration path")),
    };
    let existing = original_bytes
        .as_deref()
        .is_some_and(|bytes| !bytes.iter().all(u8::is_ascii_whitespace));
    let permissions = original_bytes
        .as_ref()
        .map(|_| {
            fs::metadata(path)
                .map(|metadata| metadata.permissions())
                .map_err(|_| invalid_config("cannot inspect the configuration file"))
        })
        .transpose()?;
    let contents = std::str::from_utf8(original_bytes.as_deref().unwrap_or_default())
        .map_err(|_| invalid_config("the existing configuration is invalid"))?;
    let document = contents
        .parse::<DocumentMut>()
        .map_err(|_| invalid_config("the existing configuration is invalid"))?;
    let raw = parse_service_config(contents, data_dir, environment)
        .map_err(|error| invalid_config(&error.to_string()))?;
    let (resolved, draft_providers) = if raw.default_model.is_none() && raw.model.is_empty() {
        // Subscription sign-in can save a connection before completing model setup.
        let providers = raw
            .resolve_providers()
            .map_err(|error| invalid_config(&error.to_string()))?;
        (None, providers)
    } else {
        let resolved = raw
            .resolve()
            .map_err(|error| invalid_config(&error.to_string()))?;
        (Some(resolved), BTreeMap::new())
    };
    Ok(LoadedDocument {
        document,
        original_bytes,
        original_permissions: permissions,
        raw,
        resolved,
        draft_providers,
        existing,
    })
}

pub(super) fn save(settings: &Settings, state: &SetupState) -> io::Result<(Config, String)> {
    save_with_hook(settings, state, || {})
}

pub(super) fn save_credential(
    settings: &Settings,
    connection: &str,
    key: &str,
) -> io::Result<String> {
    use sha2::{Digest, Sha256};

    let key = valid_stored_key(key)
        .map_err(|_| invalid_config("stored keys must be a non-empty single line"))?;
    let mut staged = stage_key(settings, &key)?;
    let mut document = settings.document.clone();
    let provider = document
        .get_mut("provider")
        .and_then(Item::as_table_like_mut)
        .and_then(|providers| providers.get_mut(connection))
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| invalid_config("the failed connection is no longer configured"))?;
    let path = staged
        .path()
        .to_str()
        .ok_or_else(|| invalid_config("credential path is not valid UTF-8"))?;
    provider.insert("api_key", value(format!("file:{path}")));
    let output = document.to_string();
    let environment = |name: &str| std::env::var(name);
    parse_service_config(&output, &settings.data_dir, &environment)
        .map_err(|_| invalid_config("the edited configuration is invalid"))?
        .resolve()
        .map_err(|_| invalid_config("the edited configuration is invalid"))?;
    write_config(settings, output.as_bytes(), || {})?;
    staged.keep = true;
    Ok(format!("{:x}", Sha256::digest(output.as_bytes())))
}

fn save_with_hook(
    settings: &Settings,
    state: &SetupState,
    before_compare: impl FnOnce(),
) -> io::Result<(Config, String)> {
    let environment = |name: &str| std::env::var(name);
    save_with_lookup(settings, state, &environment, before_compare)
}

fn save_with_lookup(
    settings: &Settings,
    state: &SetupState,
    environment: EnvironmentLookup<'_>,
    before_compare: impl FnOnce(),
) -> io::Result<(Config, String)> {
    use sha2::{Digest, Sha256};

    let models = state.model_ids().map_err(|error| invalid_config(&error))?;
    let choice = state.choice();
    let existing_key = settings.key_for(choice).cloned();
    let mut staged = None;
    let key_reference = if settings.subscription_for(choice).is_some() {
        None
    } else {
        match state.credential_source {
            CredentialSource::Environment => {
                let name = valid_env_name(state.environment.text())
                    .map_err(|_| invalid_config("invalid environment-variable name"))?;
                Some(format!("${name}"))
            }
            CredentialSource::StoredKey => {
                if !state.stored_key.text().trim().is_empty() {
                    if !settings.can_store_key() {
                        return Err(invalid_config(settings.storage_problem()));
                    }
                    let entered = valid_stored_key(state.stored_key.text()).map_err(|_| {
                        invalid_config("stored keys must be a non-empty single line")
                    })?;
                    let key = stage_key(settings, &entered)?;
                    let path = key
                        .path()
                        .to_str()
                        .ok_or_else(|| invalid_config("credential path is not valid UTF-8"))?;
                    let reference = format!("file:{path}");
                    staged = Some(key);
                    Some(reference)
                } else if settings.saved_key_available(choice) {
                    None
                } else {
                    if matches!(existing_key, Some(ApiKey::File(_))) {
                        return Err(invalid_config(
                            "the saved credential file is unavailable or unsafe",
                        ));
                    }
                    return Err(invalid_config(
                        "enter a key or choose an environment-variable name",
                    ));
                }
            }
        }
    };

    let mut document = settings.document.clone();
    let mut connection = state
        .connection(settings)
        .map_err(|error| invalid_config(&error))?;
    let provider_name = super::connection_name::for_save(settings, choice, &connection);
    state.update_capabilities(&mut connection);
    state.update_price_catalogs(&mut connection);
    let original = match choice {
        ConnectionChoice::Existing(name) => settings.provider(name),
        ConnectionChoice::Preset(_)
        | ConnectionChoice::Subscription(_)
        | ConnectionChoice::Custom => None,
    };
    let provider = entry(&mut document, "provider", &provider_name)?;
    if original.is_none_or(|old| old.format != connection.format) {
        provider.insert("format", value(connection.format.as_str()));
    }
    if original.is_none_or(|old| old.base_url != connection.base_url) {
        set_optional(
            provider,
            "base_url",
            (!connection.base_url.is_empty()).then_some(connection.base_url.as_str()),
        );
    }
    if original.and_then(|old| old.options.endpoint_url.as_deref())
        != connection.options.endpoint_url.as_deref()
    {
        set_optional(
            provider,
            "endpoint_url",
            connection.options.endpoint_url.as_deref(),
        );
    }
    if original.and_then(|old| old.options.models_url.as_deref())
        != connection.options.models_url.as_deref()
    {
        set_optional(
            provider,
            "models_url",
            connection.options.models_url.as_deref(),
        );
    }
    if original.is_none_or(|old| old.options.proxy != connection.options.proxy) {
        set_connection_proxy(provider, &connection.options.proxy)?;
    }
    if original.is_none_or(|old| old.options.model_pricing != connection.options.model_pricing)
        && !connection.options.model_pricing.is_empty()
    {
        let pricing = toml_edit::ser::to_document(&connection.options.model_pricing)
            .map_err(|_| invalid_config("could not encode model pricing"))?;
        provider.insert(
            "model_pricing",
            Item::Value(Value::InlineTable(pricing.into_table().into_inline_table())),
        );
    }
    let catalogs = &connection.options.model_price_catalogs;
    if original.map_or(!catalogs.is_empty(), |old| {
        &old.options.model_price_catalogs != catalogs
    }) {
        if catalogs.is_empty() {
            provider.remove("model_price_catalogs");
        } else {
            let catalogs = toml_edit::ser::to_document(catalogs)
                .map_err(|_| invalid_config("could not encode route price quotes"))?;
            provider.insert(
                "model_price_catalogs",
                Item::Value(Value::InlineTable(
                    catalogs.into_table().into_inline_table(),
                )),
            );
        }
    }
    if let ConnectionChoice::Preset(preset) = choice {
        if let toml::Value::String(profile) = toml::Value::try_from(preset.profile())
            .map_err(|_| invalid_config("invalid provider profile"))?
        {
            provider.insert("profile", value(profile));
        }
    }
    if let Some(key_reference) = key_reference {
        if provider.get("api_key").and_then(Item::as_str) != Some(&key_reference) {
            provider.insert("api_key", value(key_reference));
        }
    }
    if original
        .is_none_or(|old| old.options.model_capabilities != connection.options.model_capabilities)
        && !connection.options.model_capabilities.is_empty()
    {
        let capabilities = toml_edit::ser::to_document(&connection.options.model_capabilities)
            .map_err(|_| invalid_config("could not encode model capabilities"))?;
        provider.insert(
            "model_capabilities",
            Item::Value(Value::InlineTable(
                capabilities.into_table().into_inline_table(),
            )),
        );
    }

    let primary = state
        .primary_model()
        .ok_or_else(|| invalid_config("select at least one model"))?;
    let picked_default = ModelRef::new(&provider_name, &primary);
    if settings.default_model().is_none()
        || (state.intent == super::SetupIntent::Default
            && settings.default_model() != Some(&picked_default))
    {
        if let Some(previous) = settings.default_model()
            && !settings.registered(previous)
            && let Some(previous_key) = raw_default_model(&document)
        {
            model_entry(
                &mut document,
                &ModelRef::new(&previous.connection, previous_key.model),
            )?;
        }
        set_default_model(&mut document, &picked_default);
    }
    let connection_changed = original.is_some_and(|previous| {
        previous.format != connection.format
            || previous.base_url != connection.base_url
            || previous.options.endpoint_url != connection.options.endpoint_url
            || previous.options.proxy != connection.options.proxy
            || previous.options.model_pricing != connection.options.model_pricing
            || previous.options.model_capabilities != connection.options.model_capabilities
    });
    let mut validate_routes = Vec::new();
    for model in models {
        let is_primary = model == primary;
        let id = ModelRef::new(&provider_name, &model);
        let old = settings.model(&id);
        let path = model_config_key(&document, &id, environment);
        let fields = model_entry(&mut document, &path)?;
        if save_model(fields, old, &model, state, is_primary, &connection)
            || old.is_none()
            || connection_changed
        {
            validate_routes.push(id);
        }
    }

    let output = document.to_string();
    let validated = parse_service_config(&output, &settings.data_dir, environment)
        .map_err(|_| invalid_config("the edited configuration is invalid"))?
        .resolve()
        .map_err(|error| invalid_config(&error.to_string()))?;
    for id in validated.models.keys() {
        let result = if validate_routes.contains(id) {
            inspect_model(&validated, &model_agent_config(id)).map(|_| ())
        } else if connection_changed && id.connection == provider_name {
            validate_model_config(&validated, id)
        } else {
            continue;
        };
        result.map_err(|error| invalid_config(&format!("{}: {error}", id.config_path())))?;
    }

    write_config(settings, output.as_bytes(), before_compare)?;
    if let Some(staged) = staged.as_mut() {
        staged.keep = true;
    }
    Ok((
        validated,
        format!("{:x}", Sha256::digest(output.as_bytes())),
    ))
}

pub(super) fn save_model_document(
    settings: &Settings,
    document: &DocumentMut,
    routes: &[&ModelRef],
) -> Result<String, String> {
    use sha2::{Digest, Sha256};

    let output = document.to_string();
    let config = parse_service_config(&output, &settings.data_dir, &|name| std::env::var(name))
        .map_err(|_| "The edited configuration is invalid.".to_owned())?
        .resolve()
        .map_err(|error| error.to_string())?;
    for route in routes {
        inspect_model(&config, &model_agent_config(route)).map_err(|error| error.to_string())?;
    }
    write_config(settings, output.as_bytes(), || {}).map_err(|error| error.to_string())?;
    Ok(format!("{:x}", Sha256::digest(output.as_bytes())))
}

fn save_model(
    fields: &mut dyn toml_edit::TableLike,
    old: Option<&ModelConfig>,
    model: &str,
    state: &SetupState,
    primary: bool,
    connection: &kuku::config::ProviderConfig,
) -> bool {
    let changed = old.is_none();
    let manual = primary && state.reasoning_changed;
    let automatic = if !manual
        && (changed || old.is_none_or(|route| !super::models::has_configured_reasoning(route)))
        && (changed
            || !is_environment_reference(fields.get("think"))
                && !is_environment_reference(fields.get("reasoning_effort")))
    {
        kuku::adapter::inspect_connection_model(connection, model)
            .ok()
            .and_then(|caps| super::models::automatic_reasoning(&caps).map(str::to_owned))
    } else {
        None
    };
    if manual || automatic.is_some() {
        fields.insert("think", value(ThinkLevel::ProviderDefault.as_str()));
        let effort = if manual {
            Some(
                state
                    .reasoning
                    .as_deref()
                    .unwrap_or(ThinkLevel::ProviderDefault.as_str()),
            )
        } else {
            automatic.as_deref()
        };
        set_optional(fields, "reasoning_effort", effort);
    }
    if let Some(tier) = primary.then(|| state.service_override(model)).flatten() {
        set_optional(fields, "service_tier", tier);
    } else if changed {
        fields.remove("service_tier");
    }
    if changed || (primary && state.output_changed) {
        set_optional(
            fields,
            "output",
            (primary && state.output == RequestedOutput::Image).then_some("image"),
        );
    }
    let set_anthropic_limit = connection.format == ProviderFormat::Anthropic
        && fields.get("max_output_tokens").is_none()
        && !matches!(
            connection.options.profile,
            ProviderProfile::DeepSeek | ProviderProfile::OpenRouter
        );
    if set_anthropic_limit {
        fields.insert("max_output_tokens", value(8192));
    }
    manual
        || set_anthropic_limit
        || automatic.is_some()
        || (primary && (state.output_changed || state.service_override(model).is_some()))
}

pub(super) fn entry<'a>(
    document: &'a mut DocumentMut,
    section: &str,
    name: &str,
) -> io::Result<&'a mut dyn toml_edit::TableLike> {
    let inline = document.get(section).is_some_and(Item::is_inline_table);
    let table = child_table(document.as_table_mut(), section, false, true)?;
    child_table(table, name, inline, false)
}

fn child_table<'a>(
    parent: &'a mut dyn toml_edit::TableLike,
    name: &str,
    inline: bool,
    implicit: bool,
) -> io::Result<&'a mut dyn toml_edit::TableLike> {
    if !parent.contains_key(name) {
        let item = if inline {
            Item::Value(Value::InlineTable(InlineTable::new()))
        } else {
            let mut table = Table::new();
            table.set_implicit(implicit);
            Item::Table(table)
        };
        parent.insert(name, item);
    }
    parent
        .get_mut(name)
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| invalid_config("the selected configuration table cannot be edited"))
}

pub(crate) fn model_entry<'a>(
    document: &'a mut DocumentMut,
    model: &ModelRef,
) -> io::Result<&'a mut dyn toml_edit::TableLike> {
    let inline = document.get("model").is_some_and(Item::is_inline_table);
    let table = child_table(document.as_table_mut(), "model", false, true)?;
    let connection_inline = inline
        || table
            .get(&model.connection)
            .is_some_and(Item::is_inline_table);
    let models = child_table(table, &model.connection, inline, true)?;
    child_table(models, &model.model, connection_inline, false)
}

pub(super) fn model_config_key(
    document: &DocumentMut,
    id: &ModelRef,
    environment: EnvironmentLookup<'_>,
) -> ModelRef {
    let models = document
        .get("model")
        .and_then(Item::as_table_like)
        .and_then(|table| table.get(&id.connection))
        .and_then(Item::as_table_like);
    let key = models.and_then(|models| {
        models.iter().find_map(|(key, _)| {
            (key == id.model
                || key
                    .strip_prefix('$')
                    .is_some_and(|name| environment(name).is_ok_and(|value| value == id.model)))
            .then_some(key)
        })
    });
    ModelRef::new(&id.connection, key.unwrap_or(&id.model))
}

fn raw_default_model(document: &DocumentMut) -> Option<ModelRef> {
    let default = document.get("default_model")?.as_table_like()?;
    Some(ModelRef::new(
        default.get("connection")?.as_str()?,
        default.get("model")?.as_str()?,
    ))
}

pub(crate) fn set_default_model(document: &mut DocumentMut, model: &ModelRef) {
    if let Some(table) = document
        .get_mut("default_model")
        .and_then(Item::as_table_like_mut)
    {
        table.insert("connection", value(&model.connection));
        table.insert("model", value(&model.model));
        return;
    }
    let mut table = InlineTable::new();
    table.insert("connection", Value::from(model.connection.as_str()));
    table.insert("model", Value::from(model.model.as_str()));
    table.fmt();
    document["default_model"] = value(table);
}

pub(super) fn model_agent_config(model: &ModelRef) -> AgentConfig {
    AgentConfig {
        connection: Some(model.connection.clone()),
        model: Some(model.model.clone()),
        ..Default::default()
    }
}

pub(super) fn set_optional(table: &mut dyn toml_edit::TableLike, name: &str, text: Option<&str>) {
    if let Some(text) = text {
        table.insert(name, value(text));
    } else {
        table.remove(name);
    }
}

pub(super) fn set_connection_proxy(
    provider: &mut dyn toml_edit::TableLike,
    settings: &ProviderProxy,
) -> io::Result<()> {
    if settings.follows_environment() {
        provider.remove("proxy");
    } else {
        let proxy = toml_edit::ser::to_document(settings)
            .map_err(|_| invalid_config("could not encode proxy settings"))?;
        provider.insert(
            "proxy",
            Item::Value(Value::InlineTable(proxy.into_table().into_inline_table())),
        );
    }
    Ok(())
}

fn is_environment_reference(item: Option<&Item>) -> bool {
    item.and_then(Item::as_str)
        .is_some_and(|value| value.starts_with('$'))
}

pub(super) fn write_config(
    settings: &Settings,
    bytes: &[u8],
    before_compare: impl FnOnce(),
) -> io::Result<()> {
    publish_config(
        &settings.config_path,
        settings.original_bytes.as_deref(),
        settings.original_permissions.as_ref(),
        bytes,
        before_compare,
    )
}

fn unsafe_storage() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "credential storage is not a safe private directory",
    )
}

fn invalid_config(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
