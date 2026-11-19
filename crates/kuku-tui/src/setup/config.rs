use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use kuku::config::{generate_default, load_config, ApiKey, Config, ConfigFile};
use kuku::util::{is_valid_private_key, private_directory};
use kuku_client::ConnectionOptions;
use tempfile::{Builder, NamedTempFile};
use toml_edit::{value, DocumentMut, Item, Table};

use super::state::{valid_env_name, valid_stored_key, SetupState};
use super::{ConnectionChoice, CredentialSource, ProviderPreset};

pub(super) struct Settings {
    pub(super) config_path: PathBuf,
    pub(super) data_dir: PathBuf,
    pub(super) credentials_dir: PathBuf,
    pub(super) workspace: PathBuf,
    pub(super) document: DocumentMut,
    pub(super) original_bytes: Option<Vec<u8>>,
    pub(super) original_permissions: Option<fs::Permissions>,
    pub(super) raw: ConfigFile,
    pub(super) resolved: Config,
    pub(super) default_tier: String,
    pub(super) live_service: bool,
    pub(super) existing: bool,
    storage_error: Option<&'static str>,
}

impl Settings {
    pub(super) async fn load(options: ConnectionOptions) -> io::Result<Self> {
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
        let workspace = options
            .workspace()
            .map_err(|_| invalid_config("cannot resolve the workspace path"))?;
        let credentials_dir = data_dir.join("credentials");
        let loaded = load_document(&config_path)?;
        let default_tier = loaded
            .raw
            .default_model
            .clone()
            .unwrap_or_else(|| "balanced".into());
        let mut settings = Self {
            config_path,
            data_dir,
            credentials_dir,
            workspace,
            document: loaded.document,
            original_bytes: loaded.original_bytes,
            original_permissions: loaded.original_permissions,
            raw: loaded.raw,
            resolved: loaded.resolved,
            default_tier,
            live_service: active.is_some(),
            existing: loaded.existing,
            storage_error: None,
        };
        settings.storage_error = if settings.credentials_dir.starts_with(&settings.workspace) {
            Some("Stored keys are disabled because credentials would be inside the workspace; use a different --data-dir or Environment.")
        } else {
            settings.check_storage(false).err().map(|_| {
                "Stored keys are unavailable because the credentials directory is not safe; use Environment or a different --data-dir."
            })
        };
        Ok(settings)
    }

    pub(super) fn connection_choices(&self) -> Vec<ConnectionChoice> {
        let mut choices = Vec::new();
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
        choices
    }

    pub(super) fn initial_choice(&self) -> ConnectionChoice {
        if self.existing {
            if let Some(tier) = self.raw.model.get(&self.default_tier) {
                return ConnectionChoice::Existing(tier.provider.clone());
            }
        }
        ConnectionChoice::Preset(ProviderPreset::Anthropic)
    }

    pub(super) fn choice_label(&self, choice: &ConnectionChoice) -> String {
        match choice {
            ConnectionChoice::Existing(name) => format!("Existing · {name}"),
            ConnectionChoice::Preset(preset) => format!("Preset · {}", preset.label()),
        }
    }

    pub(super) fn model_for(&self, choice: &ConnectionChoice) -> String {
        match choice {
            ConnectionChoice::Preset(preset) => preset.default_model().into(),
            ConnectionChoice::Existing(name) => self
                .raw
                .model
                .get(&self.default_tier)
                .filter(|entry| entry.provider == *name)
                .or_else(|| {
                    self.raw
                        .model
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
                ConnectionChoice::Existing(name) => {
                    format!("{}_API_KEY", name.to_ascii_uppercase())
                }
            },
        }
    }

    pub(super) fn source_for(&self, choice: &ConnectionChoice) -> CredentialSource {
        match self.key_for(choice) {
            Some(ApiKey::Env(_)) | None => CredentialSource::Environment,
            Some(ApiKey::File(_) | ApiKey::Plaintext(_)) => CredentialSource::StoredKey,
        }
    }

    pub(super) fn saved_key_available(&self, choice: &ConnectionChoice) -> bool {
        match self.key_for(choice) {
            Some(ApiKey::Plaintext(_)) => self.can_store_key(),
            Some(ApiKey::File(path)) => self.can_store_key() && is_valid_private_key(path),
            _ => false,
        }
    }

    pub(super) fn file_reference_in_current_store(&self, path: &Path) -> bool {
        if !self.can_store_key() {
            return false;
        }
        let (Ok(path), Ok(credentials), Ok(workspace)) = (
            path.canonicalize(),
            self.credentials_dir.canonicalize(),
            self.workspace.canonicalize(),
        ) else {
            return false;
        };
        path.parent() == Some(credentials.as_path()) && !path.starts_with(workspace)
    }

    pub(super) fn key_for(&self, choice: &ConnectionChoice) -> Option<&ApiKey> {
        match choice {
            ConnectionChoice::Existing(name) => {
                self.resolved.providers.get(name).map(|p| &p.api_key)
            }
            ConnectionChoice::Preset(_) => None,
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
        let candidate = self.credentials_dir.as_path();
        let canonical_data = self.data_dir.canonicalize()?;
        let canonical_workspace = self.workspace.canonicalize()?;
        let resolved_candidate = match fs::symlink_metadata(candidate) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(unsafe_storage());
                }
                candidate.canonicalize()?
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                canonical_data.join("credentials")
            }
            Err(_) => return Err(unsafe_storage()),
        };
        if resolved_candidate.starts_with(&canonical_workspace) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "credential storage must be outside the workspace",
            ));
        }
        if create {
            private_directory(candidate).map_err(|_| unsafe_storage())?;
        } else if candidate.is_dir() {
            private_directory(candidate).map_err(|_| unsafe_storage())?;
        }
        Ok(())
    }
}

struct LoadedDocument {
    document: DocumentMut,
    original_bytes: Option<Vec<u8>>,
    original_permissions: Option<fs::Permissions>,
    raw: ConfigFile,
    resolved: Config,
    existing: bool,
}

fn load_document(path: &Path) -> io::Result<LoadedDocument> {
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
    let (document, raw, resolved, permissions) = if let Some(bytes) = &original_bytes {
        let contents = std::str::from_utf8(bytes)
            .map_err(|_| invalid_config("the existing configuration is invalid"))?;
        let document = contents
            .parse::<DocumentMut>()
            .map_err(|_| invalid_config("the existing configuration is invalid"))?;
        let raw = load_config(path)
            .map_err(|_| invalid_config("the existing configuration is invalid"))?;
        let resolved = raw
            .resolve()
            .map_err(|_| invalid_config("the existing configuration is invalid"))?;
        let permissions = fs::metadata(path)
            .map_err(|_| invalid_config("cannot inspect the configuration file"))?
            .permissions();
        (document, raw, resolved, Some(permissions))
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
        (document, raw, resolved, None)
    };
    let existing = original_bytes.is_some();
    Ok(LoadedDocument {
        document,
        original_bytes,
        original_permissions: permissions,
        raw,
        resolved,
        existing,
    })
}

pub(super) fn save(settings: &Settings, state: &SetupState) -> io::Result<()> {
    save_with_hook(settings, state, || {})
}

fn save_with_hook(
    settings: &Settings,
    state: &SetupState,
    before_compare: impl FnOnce(),
) -> io::Result<()> {
    let model = state
        .model_text()
        .map_err(|_| invalid_config("invalid model ID"))?;
    let choice = state.choice();
    let provider_name = provider_name(settings, choice);
    let existing_key = settings.key_for(choice).cloned();
    let mut staged = None;
    let key_reference = match state.credential_source {
        CredentialSource::Environment => {
            let name = valid_env_name(state.environment.text())
                .map_err(|_| invalid_config("invalid environment-variable name"))?;
            format!("${name}")
        }
        CredentialSource::StoredKey => {
            let entered =
                if state.stored_key.text().trim().is_empty() {
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
    };

    let mut document = settings.document.clone();
    if !settings.existing {
        if let Some(providers) = document.get_mut("provider").and_then(Item::as_table_mut) {
            providers.clear();
        }
        for tier in ["strong", "balanced", "light"] {
            if let Some(model_entry) = document
                .get_mut("model")
                .and_then(Item::as_table_mut)
                .and_then(|table| table.get_mut(tier))
                .and_then(Item::as_table_mut)
            {
                model_entry["provider"] = value(provider_name.as_str());
                model_entry["model"] = value(model.as_str());
            }
        }
        document["default_model"] = value("balanced");
    } else if let Some(model_entry) = document
        .get_mut("model")
        .and_then(Item::as_table_mut)
        .and_then(|table| table.get_mut(&settings.default_tier))
        .and_then(Item::as_table_mut)
    {
        let route_changed = settings
            .raw
            .model
            .get(&settings.default_tier)
            .is_none_or(|entry| entry.provider != provider_name || entry.model != model);
        model_entry["provider"] = value(provider_name.as_str());
        model_entry["model"] = value(model.as_str());
        if route_changed {
            model_entry.remove("reasoning_effort");
            model_entry.remove("service_tier");
        }
    } else {
        return Err(invalid_config("the selected default tier cannot be edited"));
    }

    match choice {
        ConnectionChoice::Existing(name) => {
            let api_key = document
                .get_mut("provider")
                .and_then(Item::as_table_mut)
                .and_then(|table| table.get_mut(name))
                .and_then(Item::as_table_mut)
                .and_then(|table| table.get("api_key"))
                .and_then(Item::as_str)
                .unwrap_or_default();
            if api_key != key_reference {
                document["provider"][name]["api_key"] = value(key_reference.as_str());
            }
        }
        ConnectionChoice::Preset(preset) => {
            let mut table = Table::new();
            table["format"] = value(preset.format());
            table["base_url"] = value(preset.base_url());
            table["api_key"] = value(key_reference.as_str());
            table["profile"] = value(preset.profile());
            document["provider"][provider_name.as_str()] = Item::Table(table);
        }
    }

    let output = document.to_string();
    let validated: ConfigFile = toml::from_str(&output)
        .map_err(|_| invalid_config("the edited configuration is invalid"))?;
    validated
        .resolve()
        .map_err(|_| invalid_config("the edited configuration is invalid"))?;

    let mut stage_guard = staged;
    write_config(settings, output.as_bytes(), before_compare)?;
    if let Some(staged) = stage_guard.as_mut() {
        staged.keep = true;
    }
    Ok(())
}

fn provider_name(settings: &Settings, choice: &ConnectionChoice) -> String {
    match choice {
        ConnectionChoice::Existing(name) => name.clone(),
        ConnectionChoice::Preset(preset) if !settings.existing => preset.connection().into(),
        ConnectionChoice::Preset(preset) => {
            let providers = settings.document.get("provider").and_then(Item::as_table);
            let base = format!("{}_setup", preset.connection());
            if providers.is_none_or(|table| !table.contains_key(&base)) {
                base
            } else {
                (2..)
                    .map(|index| format!("{base}_{index}"))
                    .find(|name| providers.is_some_and(|table| !table.contains_key(name)))
                    .expect("a unique connection name")
            }
        }
    }
}

struct StagedKey {
    path: PathBuf,
    keep: bool,
}

impl StagedKey {
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for StagedKey {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn stage_key(settings: &Settings, key: &str) -> io::Result<StagedKey> {
    settings.check_storage(true).map_err(|_| {
        invalid_config("stored keys are unavailable; use Environment or a different --data-dir")
    })?;
    let mut file = Builder::new()
        .prefix("key-")
        .suffix(".credential")
        .tempfile_in(&settings.credentials_dir)
        .map_err(|_| invalid_config("cannot create a private credential file"))?;
    file.write_all(key.as_bytes())
        .and_then(|()| file.as_file().sync_all())
        .map_err(|_| invalid_config("cannot save the private credential file"))?;
    let (_, path) = file
        .keep()
        .map_err(|_| invalid_config("cannot save the private credential file"))?;
    Ok(StagedKey { path, keep: false })
}

fn write_config(
    settings: &Settings,
    bytes: &[u8],
    before_compare: impl FnOnce(),
) -> io::Result<()> {
    let parent = settings
        .config_path
        .parent()
        .ok_or_else(|| invalid_config("configuration path has no parent directory"))?;
    fs::create_dir_all(parent)
        .map_err(|_| invalid_config("cannot create configuration directory"))?;
    let canonical_path = canonical_config_path(&settings.config_path)?;
    let mut lock_path = canonical_path.as_os_str().to_os_string();
    lock_path.push(".lock");
    let mut lock = fslock::LockFile::open(&PathBuf::from(lock_path))
        .map_err(|_| invalid_config("cannot lock configuration for update"))?;
    lock.lock()
        .map_err(|_| invalid_config("cannot lock configuration for update"))?;
    let mut before_compare = Some(before_compare);
    if let Some(original) = &settings.original_bytes {
        before_compare.take().expect("single concurrency hook")();
        let metadata = fs::symlink_metadata(&settings.config_path)
            .map_err(|_| invalid_config("configuration changed while settings were open"))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(invalid_config(
                "configuration changed while settings were open",
            ));
        }
        let current = fs::read(&settings.config_path)
            .map_err(|_| invalid_config("configuration changed while settings were open"))?;
        if current != *original {
            return Err(invalid_config(
                "configuration changed while settings were open; reopen settings",
            ));
        }
        let mut temporary = NamedTempFile::new_in(parent)
            .map_err(|_| invalid_config("cannot stage the updated configuration"))?;
        if let Some(permissions) = &settings.original_permissions {
            temporary
                .as_file()
                .set_permissions(permissions.clone())
                .map_err(|_| invalid_config("cannot preserve configuration permissions"))?;
        }
        temporary
            .write_all(bytes)
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|_| invalid_config("cannot write the updated configuration"))?;
        temporary
            .persist(&settings.config_path)
            .map_err(|_| invalid_config("cannot publish the updated configuration"))?;
    } else {
        before_compare.take().expect("single concurrency hook")();
        let mut temporary = NamedTempFile::new_in(parent)
            .map_err(|_| invalid_config("cannot stage the new configuration"))?;
        temporary
            .write_all(bytes)
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|_| invalid_config("cannot write the new configuration"))?;
        temporary
            .persist_noclobber(&settings.config_path)
            .map_err(|_| invalid_config("configuration already exists; it was not overwritten"))?;
    }
    if let Ok(directory) = File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

fn canonical_config_path(path: &Path) -> io::Result<PathBuf> {
    match fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .ok_or_else(|| invalid_config("configuration path has no parent directory"))?;
            let name = path
                .file_name()
                .ok_or_else(|| invalid_config("configuration path has no file name"))?;
            Ok(fs::canonicalize(parent)?.join(name))
        }
        Err(error) => Err(error),
    }
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
