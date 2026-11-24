use std::collections::BTreeMap;
use std::env::VarError;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::state::{valid_env_name, valid_stored_key, SetupState};
use super::{ConnectionChoice, CredentialSource, ProviderPreset};
use kuku::adapter::{inspect_model, SubscriptionAccountInfo};
use kuku::config::{
    generate_default, parse_config_text_with_env, ApiKey, Config, ConfigFile, ProviderFormat,
    ProviderProfile, ProviderProxy, RequestedOutput, SubscriptionChannel, ThinkLevel,
};
use kuku::session::AgentConfig;
use kuku::util::{is_valid_private_key, private_directory};
use kuku_client::ConnectionOptions;
use toml_edit::{value, DocumentMut, InlineTable, Item, Table, Value};

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
    pub(super) resolved: Config,
    pub(super) default_tier: String,
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
        let default_tier = loaded.resolved.default_tier.clone();
        let subscription_info = super::subscription::account_info(&loaded.resolved);
        let mut settings = Self {
            config_path,
            data_dir,
            credentials_dir,
            document: loaded.document,
            original_bytes: loaded.original_bytes,
            original_permissions: loaded.original_permissions,
            raw: loaded.raw,
            resolved: loaded.resolved,
            default_tier,
            existing: loaded.existing,
            subscription_info,
            storage_error: None,
        };
        settings.storage_error = settings.check_storage(false).err().map(|_| {
            "Stored keys are unavailable because the credentials directory is not safe; use Environment or a different --data-dir."
        });
        Ok(settings)
    }

    pub(super) fn connection_choices(&self) -> Vec<ConnectionChoice> {
        let mut choices = vec![ConnectionChoice::Custom];
        if self.existing {
            choices.extend(
                self.raw
                    .provider
                    .keys()
                    .cloned()
                    .map(ConnectionChoice::Existing),
            );
        }
        choices.extend(
            ProviderPreset::ALL
                .into_iter()
                .map(ConnectionChoice::Preset),
        );
        choices.push(ConnectionChoice::Subscription(SubscriptionChannel::ChatGpt));
        choices
    }

    pub(super) fn protocol_for(&self, choice: &ConnectionChoice) -> ProviderFormat {
        match choice {
            ConnectionChoice::Existing(name) => self.resolved.providers[name].format,
            ConnectionChoice::Preset(preset) => preset.format(),
            ConnectionChoice::Subscription(SubscriptionChannel::ChatGpt) => {
                ProviderFormat::OpenAiResponses
            }
            ConnectionChoice::Custom => ProviderFormat::OpenAiChat,
        }
    }

    pub(super) fn address_for(&self, choice: &ConnectionChoice) -> (String, bool, String) {
        if let ConnectionChoice::Existing(name) = choice {
            let config = &self.resolved.providers[name];
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
        if self.existing {
            if let Some(tier) = self.resolved.tiers.get(&self.default_tier) {
                return ConnectionChoice::Existing(tier.provider.clone());
            }
        }
        ConnectionChoice::Custom
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
            ConnectionChoice::Existing(name) => self
                .resolved
                .tiers
                .get(&self.default_tier)
                .filter(|entry| entry.provider == *name)
                .or_else(|| {
                    self.resolved
                        .tiers
                        .values()
                        .find(|entry| entry.provider == *name)
                })
                .map_or_else(String::new, |entry| entry.model.clone()),
        }
    }

    pub(super) fn environment_for(&self, choice: &ConnectionChoice) -> String {
        match self.key_for(choice) {
            Some(ApiKey::Env(name)) => name.clone(),
            _ => match choice {
                ConnectionChoice::Preset(preset) => preset.default_env().into(),
                ConnectionChoice::Custom => "KUKU_CUSTOM_API_KEY".into(),
                ConnectionChoice::Subscription(_) => String::new(),
                ConnectionChoice::Existing(name) => {
                    format!("{}_API_KEY", name.to_ascii_uppercase())
                }
            },
        }
    }

    pub(super) fn source_for(&self, choice: &ConnectionChoice) -> CredentialSource {
        let key = self.key_for(choice);
        let environment_available = match key {
            Some(key @ ApiKey::Env(_)) => resolved_credential_available(key),
            None => {
                let key = ApiKey::Env(self.environment_for(choice));
                resolved_credential_available(&key)
            }
            Some(ApiKey::File(_) | ApiKey::Plaintext(_)) => false,
        };
        preferred_source(key, environment_available, self.can_store_key())
    }

    pub(super) fn saved_key_available(&self, choice: &ConnectionChoice) -> bool {
        match self.key_for(choice) {
            Some(ApiKey::Plaintext(value)) => self.can_store_key() && !value.trim().is_empty(),
            Some(ApiKey::File(path)) => self.can_store_key() && is_valid_private_key(path),
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
        let Some((tier, provider)) = self
            .resolved
            .tiers
            .get(&self.default_tier)
            .and_then(|tier| {
                self.resolved
                    .providers
                    .get(&tier.provider)
                    .map(|provider| (tier, provider))
            })
        else {
            return false;
        };
        if provider.options.subscription.is_some() {
            self.subscription_available(&ConnectionChoice::Existing(tier.provider.clone()))
        } else {
            resolve(&provider.api_key).is_some_and(|value| !value.trim().is_empty())
        }
    }

    pub(super) fn file_reference_in_current_store(&self, path: &Path) -> bool {
        if !self.can_store_key() {
            return false;
        }
        let (Ok(path), Ok(credentials)) =
            (path.canonicalize(), self.credentials_dir.canonicalize())
        else {
            return false;
        };
        path.parent() == Some(credentials.as_path())
    }

    pub(super) fn key_for(&self, choice: &ConnectionChoice) -> Option<&ApiKey> {
        match choice {
            ConnectionChoice::Existing(name) => {
                self.resolved.providers.get(name).map(|p| &p.api_key)
            }
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

fn preferred_source(
    key: Option<&ApiKey>,
    environment_available: bool,
    can_store_key: bool,
) -> CredentialSource {
    match key {
        Some(ApiKey::File(_) | ApiKey::Plaintext(_)) => CredentialSource::StoredKey,
        _ if environment_available => CredentialSource::Environment,
        _ if can_store_key => CredentialSource::StoredKey,
        _ => CredentialSource::Environment,
    }
}

fn resolved_credential_available(key: &ApiKey) -> bool {
    key.resolve().is_ok_and(|value| !value.trim().is_empty())
}

struct LoadedDocument {
    document: DocumentMut,
    original_bytes: Option<Vec<u8>>,
    original_permissions: Option<fs::Permissions>,
    raw: ConfigFile,
    resolved: Config,
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
            ))
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
    let (document, raw, resolved) =
        if let Some(bytes) = original_bytes.as_ref().filter(|_| existing) {
            let contents = std::str::from_utf8(bytes)
                .map_err(|_| invalid_config("the existing configuration is invalid"))?;
            let document = contents
                .parse::<DocumentMut>()
                .map_err(|_| invalid_config("the existing configuration is invalid"))?;
            let raw = parse_service_config(contents, data_dir, environment)
                .map_err(|_| invalid_config("the existing configuration is invalid"))?;
            let resolved = raw
                .resolve()
                .map_err(|_| invalid_config("the existing configuration is invalid"))?;
            (document, raw, resolved)
        } else {
            let contents = generate_default();
            let document = contents
                .parse::<DocumentMut>()
                .map_err(|_| invalid_config("cannot create the default configuration"))?;
            let raw: ConfigFile = toml::from_str(contents)
                .map_err(|_| invalid_config("cannot create the default configuration"))?;
            let resolved = raw
                .resolve()
                .map_err(|_| invalid_config("cannot create the default configuration"))?;
            (document, raw, resolved)
        };
    Ok(LoadedDocument {
        document,
        original_bytes,
        original_permissions: permissions,
        raw,
        resolved,
        existing,
    })
}

pub(super) fn save(settings: &Settings, state: &SetupState) -> io::Result<Config> {
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
) -> io::Result<Config> {
    let environment = |name: &str| std::env::var(name);
    save_with_lookup(settings, state, &environment, before_compare)
}

fn save_with_lookup(
    settings: &Settings,
    state: &SetupState,
    environment: EnvironmentLookup<'_>,
    before_compare: impl FnOnce(),
) -> io::Result<Config> {
    let models = state.model_ids().map_err(|error| invalid_config(&error))?;
    let choice = state.choice();
    let existing_key = settings.key_for(choice).cloned();
    let mut staged = None;
    let key_reference = if settings.subscription_for(choice).is_some() {
        None
    } else {
        Some(match state.credential_source {
            CredentialSource::Environment => {
                let name = valid_env_name(state.environment.text())
                    .map_err(|_| invalid_config("invalid environment-variable name"))?;
                format!("${name}")
            }
            CredentialSource::StoredKey => {
                let entered = if state.stored_key.text().trim().is_empty() {
                    None
                } else {
                    Some(valid_stored_key(state.stored_key.text()).map_err(|_| {
                        invalid_config("stored keys must be a non-empty single line")
                    })?)
                };
                if entered.is_some() && !settings.can_store_key() {
                    return Err(invalid_config(settings.storage_problem()));
                }
                let contents = match (entered, existing_key.as_ref()) {
                    (Some(key), _) => Some(key),
                    (None, Some(ApiKey::Plaintext(key))) => Some(key.clone()),
                    (None, Some(ApiKey::File(path))) => {
                        if !is_valid_private_key(path) {
                            return Err(invalid_config(
                                "the saved credential file is unavailable or unsafe",
                            ));
                        }
                        if settings.file_reference_in_current_store(path) {
                            None
                        } else {
                            Some(
                                existing_key
                                    .as_ref()
                                    .expect("file key source")
                                    .resolve()
                                    .map_err(|_| {
                                        invalid_config(
                                            "the saved credential file is unavailable or unsafe",
                                        )
                                    })?,
                            )
                        }
                    }
                    (None, _) => {
                        return Err(invalid_config(
                            "enter a key; Environment is a separate source",
                        ));
                    }
                };
                if let Some(contents) = contents {
                    staged = Some(stage_key(settings, &contents)?);
                    let path = staged.as_ref().expect("staged key").path();
                    format!(
                        "file:{}",
                        path.to_str()
                            .ok_or_else(|| invalid_config("credential path is not valid UTF-8"))?
                    )
                } else if let Some(ApiKey::File(path)) = existing_key.as_ref() {
                    format!(
                        "file:{}",
                        path.to_str()
                            .ok_or_else(|| invalid_config("credential path is not valid UTF-8"))?
                    )
                } else {
                    return Err(invalid_config("saved credential source is unavailable"));
                }
            }
        })
    };

    let mut document = settings.document.clone();
    let mut connection = state
        .connection(settings)
        .map_err(|error| invalid_config(&error))?;
    let provider_name = super::connection_name::for_save(settings, choice, &connection);
    state.update_capabilities(&mut connection);
    state.update_price_catalogs(&mut connection);
    if !settings.existing {
        edit_table(&mut document, "provider")?.clear();
        edit_table(&mut document, "model")?.clear();
    }
    let original = match choice {
        ConnectionChoice::Existing(name) => settings.resolved.provider(name),
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
    let edit_route = state.route_name(settings).map(str::to_owned);
    if !settings.existing {
        document["default_model"] = value("default");
    } else if settings.raw.default_model.is_none()
        && settings.raw.model.len() == 1
        && models.len() > usize::from(edit_route.is_some())
    {
        document["default_model"] = value(settings.default_tier.as_str());
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
        let routes = edit_table(&mut document, "model")?;
        let tier = match (is_primary, &edit_route, settings.existing) {
            (true, Some(name), _) => name.clone(),
            (true, None, false) => "default".into(),
            _ => unique_route(routes, &provider_name, &model),
        };
        let old = settings.resolved.tier(&tier);
        let fields = entry(&mut document, "model", &tier)?;
        if save_route(
            fields,
            old,
            &provider_name,
            &model,
            state,
            is_primary,
            &connection,
        ) {
            validate_routes.push(tier);
        }
    }

    let output = document.to_string();
    let validated = parse_service_config(&output, &settings.data_dir, environment)
        .map_err(|_| invalid_config("the edited configuration is invalid"))?
        .resolve()
        .map_err(|error| invalid_config(&error.to_string()))?;
    for (tier, route) in &validated.tiers {
        if validate_routes.contains(tier) || (connection_changed && route.provider == provider_name)
        {
            inspect_model(
                &validated,
                &AgentConfig {
                    tier: Some(tier.clone()),
                    ..AgentConfig::default()
                },
            )
            .map_err(|error| invalid_config(&format!("Model {tier}: {error}")))?;
        }
    }

    write_config(settings, output.as_bytes(), before_compare)?;
    if let Some(staged) = staged.as_mut() {
        staged.keep = true;
    }
    Ok(validated)
}

pub(super) fn save_model_document(
    settings: &Settings,
    document: &DocumentMut,
    routes: &[&str],
) -> Result<Config, String> {
    let output = document.to_string();
    let config = parse_service_config(&output, &settings.data_dir, &|name| std::env::var(name))
        .map_err(|_| "The edited configuration is invalid.".to_owned())?
        .resolve()
        .map_err(|error| error.to_string())?;
    for route in routes {
        inspect_model(
            &config,
            &AgentConfig {
                tier: Some((*route).into()),
                ..AgentConfig::default()
            },
        )
        .map_err(|error| error.to_string())?;
    }
    write_config(settings, output.as_bytes(), || {}).map_err(|error| error.to_string())?;
    Ok(config)
}

fn save_route(
    fields: &mut dyn toml_edit::TableLike,
    old: Option<&kuku::config::TierConfig>,
    provider: &str,
    model: &str,
    state: &SetupState,
    primary: bool,
    connection: &kuku::config::ProviderConfig,
) -> bool {
    let changed = old.is_none_or(|route| route.provider != provider || route.model != model);
    if changed || !is_environment_reference(fields.get("provider")) {
        fields.insert("provider", value(provider));
    }
    if changed || !is_environment_reference(fields.get("model")) {
        fields.insert("model", value(model));
    }
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
    if changed || manual || automatic.is_some() {
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
    if connection.format == ProviderFormat::Anthropic
        && fields.get("max_output_tokens").is_none()
        && !matches!(
            connection.options.profile,
            ProviderProfile::DeepSeek | ProviderProfile::OpenRouter
        )
    {
        fields.insert("max_output_tokens", value(8192));
    }
    changed
        || manual
        || automatic.is_some()
        || (primary && (state.output_changed || state.service_override(model).is_some()))
}

fn edit_table<'a>(
    document: &'a mut DocumentMut,
    name: &str,
) -> io::Result<&'a mut dyn toml_edit::TableLike> {
    document
        .as_table_mut()
        .entry(name)
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_like_mut()
        .ok_or_else(|| invalid_config("the configuration table cannot be edited"))
}

pub(super) fn entry<'a>(
    document: &'a mut DocumentMut,
    section: &str,
    name: &str,
) -> io::Result<&'a mut dyn toml_edit::TableLike> {
    let inline = document.get(section).is_some_and(Item::is_inline_table);
    let table = edit_table(document, section)?;
    if !table.contains_key(name) {
        let item = if inline {
            Item::Value(Value::InlineTable(InlineTable::new()))
        } else {
            Item::Table(Table::new())
        };
        table.insert(name, item);
    }
    table
        .get_mut(name)
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| invalid_config("the selected configuration entry cannot be edited"))
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

fn unique_route(routes: &dyn toml_edit::TableLike, provider: &str, model: &str) -> String {
    if !routes.contains_key(model) {
        return model.to_owned();
    }
    let base = format!("{provider}/{model}");
    if !routes.contains_key(&base) {
        return base;
    }
    (2..)
        .map(|index| format!("{base}-{index}"))
        .find(|name| !routes.contains_key(name))
        .expect("a unique model route")
}

pub(super) fn write_config(
    settings: &Settings,
    bytes: &[u8],
    before_compare: impl FnOnce(),
) -> io::Result<()> {
    crate::config_file::publish_config(
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
