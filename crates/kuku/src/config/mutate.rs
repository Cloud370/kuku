use std::collections::BTreeSet;
use std::fs::{self, File, Permissions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use toml_edit::{DocumentMut, Item, Key, Table, TableLike};

use super::resolve::{parse_config_text, parse_config_text_with_env};
use super::types::ModelRef;
use crate::config::types::{ConfigFile, HandoffConfig, ThinkLevel, UpdateConfig};
use crate::error::{Error, Result};

/// Generate a default config file content as a TOML string.
/// Used by the host on first run.
pub fn generate_default() -> &'static str {
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/default-config.toml"
    ))
}

/// Detect missing config sections via struct deserialization, then inject
/// defaults using `toml_edit` to preserve user comments and formatting.
pub fn config_patch_defaults(raw: &str) -> Result<(String, bool)> {
    let file: ConfigFile = parse_config_text(raw)?;

    if file.handoff.is_some()
        && file.logs.is_some()
        && file.plugin.is_some()
        && file.update.is_some()
    {
        return Ok((raw.to_string(), false));
    }

    let mut doc: toml_edit::DocumentMut = raw
        .parse()
        .map_err(|error| Error::ConfigLoad(format!("invalid config: {error}")))?;

    if file.handoff.is_none() {
        inject_handoff_section(&mut doc);
    }
    if file.logs.is_none() {
        inject_logs_section(&mut doc);
    }
    if file.plugin.is_none() {
        inject_plugin_section(&mut doc);
    }
    if file.update.is_none() {
        inject_update_section(&mut doc);
    }

    Ok((doc.to_string(), true))
}

fn inject_handoff_section(doc: &mut toml_edit::DocumentMut) {
    let mut section = toml_edit::Table::new();
    *section.decor_mut() = toml_edit::Decor::new(
        "\n\n# Context handoff: inject summary when context usage exceeds threshold.\n",
        "",
    );
    let defaults = HandoffConfig::default();
    section["enabled"] = toml_edit::value(defaults.enabled);
    section["threshold"] = toml_edit::value(defaults.threshold);
    section["keep_turns"] = toml_edit::value(defaults.keep_turns as i64);

    doc["handoff"] = toml_edit::Item::Table(section);
}

fn inject_plugin_section(doc: &mut toml_edit::DocumentMut) {
    let mut section = toml_edit::Table::new();
    *section.decor_mut() = toml_edit::Decor::new(
        "\n\n# Plugin system: enable/disable hook execution from .kuku/packages/.\n",
        "",
    );
    // Default to false for patched configs — existing users opt in explicitly.
    // New installs use assets/default-config.toml which sets enabled = true.
    section["enabled"] = toml_edit::value(false);
    doc["plugin"] = toml_edit::Item::Table(section);
}

fn inject_logs_section(doc: &mut toml_edit::DocumentMut) {
    let mut section = toml_edit::Table::new();
    *section.decor_mut() = toml_edit::Decor::new(
        "\n\n# Logs: retention policy for session, runtime, and host JSONL logs.\n",
        "",
    );
    section["max_age_days"] = toml_edit::value(14);
    section["max_total_size_mb"] = toml_edit::value(512);
    doc["logs"] = toml_edit::Item::Table(section);
}

fn inject_update_section(doc: &mut toml_edit::DocumentMut) {
    let mut section = toml_edit::Table::new();
    *section.decor_mut() = toml_edit::Decor::new(
        "\n\n# Update system: self-update source, channel, and custom mirrors.\n",
        "",
    );
    let defaults = UpdateConfig::default();
    section["source"] = toml_edit::value(defaults.source);
    section["channel"] = toml_edit::value(defaults.channel);
    doc["update"] = toml_edit::Item::Table(section);
}

/// Load config from disk, patch missing sections, write back atomically if changed.
/// Returns the resolved ConfigFile. Preserves user comments via `toml_edit`.
pub fn load_and_patch_config(path: &Path) -> Result<ConfigFile> {
    load_and_patch_config_with_hook(path, || {})
}

pub(crate) fn load_and_patch_config_with_hook(
    path: &Path,
    before_guarded_write: impl FnOnce(),
) -> Result<ConfigFile> {
    let raw = fs::read_to_string(path)
        .map_err(|error| Error::ConfigLoad(format!("cannot read config file {path:?}: {error}")))?;
    let (patched, changed) = config_patch_defaults(&raw)?;
    let file = parse_config_text(&patched)?;
    if changed {
        match publish_config(
            path,
            Some(raw.as_bytes()),
            None,
            patched.as_bytes(),
            before_guarded_write,
        ) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                return parse_config_text(&fs::read_to_string(path)?);
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(file)
}

/// SHA-256 of the exact bytes of a configuration snapshot.
pub fn config_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Publish a staged configuration if the file still has the expected contents.
///
/// Use `None` for the original bytes only when creating an absent file. Writers
/// using this API share a non-blocking lock. A conflicting write returns
/// `AlreadyExists` after comparing the bytes, or `WouldBlock` if another writer
/// still holds the lock. The pre-comparison hook runs with the lock held.
pub fn publish_config(
    config_path: &Path,
    original_bytes: Option<&[u8]>,
    original_permissions: Option<&Permissions>,
    bytes: &[u8],
    before_compare: impl FnOnce(),
) -> io::Result<()> {
    let parent = config_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = config_path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "configuration path has no file name",
        )
    })?;
    fs::create_dir_all(parent)?;
    let parent = fs::canonicalize(parent)?;
    let path = parent.join(name);
    let mut lock_path = path.as_os_str().to_os_string();
    lock_path.push(".lock");
    let mut lock = fslock::LockFile::open(&PathBuf::from(lock_path))?;
    if !lock.try_lock()? {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "configuration update is already in progress. Retry the update",
        ));
    }

    let permissions = match fs::symlink_metadata(&path) {
        Ok(metadata) if original_bytes.is_some() && metadata.is_file() => Some(
            original_permissions
                .cloned()
                .unwrap_or_else(|| metadata.permissions()),
        ),
        Err(error) if original_bytes.is_none() && error.kind() == io::ErrorKind::NotFound => None,
        Ok(_) => return Err(config_changed()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(config_changed()),
        Err(error) => return Err(error),
    };
    let mut temporary = NamedTempFile::new_in(&parent)?;
    if let Some(permissions) = permissions {
        temporary.as_file().set_permissions(permissions)?;
    }
    #[cfg(windows)]
    crate::util::set_config_temp_security(
        original_bytes.map(|_| path.as_path()),
        temporary.path(),
        temporary.as_file(),
    )?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    before_compare();

    if let Some(original) = original_bytes {
        let current = fs::symlink_metadata(&path).and_then(|metadata| {
            if metadata.is_file() {
                fs::read(&path)
            } else {
                Err(config_changed())
            }
        });
        match current {
            Ok(bytes) if bytes == original => {}
            Ok(_) => return Err(config_changed()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(config_changed()),
            Err(error) => return Err(error),
        }
        temporary.persist(&path).map_err(|error| error.error)?;
    } else {
        temporary.persist_noclobber(&path).map_err(|error| {
            if error.error.kind() == io::ErrorKind::AlreadyExists {
                config_changed()
            } else {
                error.error
            }
        })?;
    }
    if let Ok(directory) = File::open(&parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

fn config_changed() -> io::Error {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        "configuration changed since it was read. Retry the update",
    )
}

/// Apply a literal TOML patch, preserving unrelated comments and fields.
///
/// Remove the TOML dotted paths in the top-level `delete` array, then merge
/// tables with the remaining keys. An explicitly supplied `default_model`
/// replaces the entire pair. Callers validate the resulting text before publishing.
pub fn patch_config_text_with_env(
    raw: &str,
    patch: &str,
    environment: impl Fn(&str) -> std::result::Result<String, std::env::VarError>,
) -> Result<String> {
    let mut doc = parse_document(raw)?;
    let mut patch: DocumentMut = patch
        .parse::<DocumentMut>()
        .map_err(|error| config_field("patch", error.message()))?;
    let previous = read_default(&doc, &environment);
    let delete = patch.remove("delete");
    let delete = delete
        .as_ref()
        .map(|value| {
            value
                .as_array()
                .ok_or_else(|| config_field("delete", "expected an array of TOML paths"))
        })
        .transpose()?;
    let mut removed_previous = false;
    if let Some(paths) = delete {
        for (index, path) in paths.iter().enumerate() {
            let path = path.as_str().ok_or_else(|| {
                config_field(format!("delete[{index}]"), "expected a TOML dotted path")
            })?;
            let keys = Key::parse(path).map_err(|error| config_field(path, error.message()))?;
            let covers_previous = previous.as_ref().is_some_and(|(pair, raw_model)| {
                removes_model(&keys, pair, raw_model, &environment)
            });
            let keys = resolve_delete_path(&doc, keys, &environment);
            if !remove_path(doc.as_table_mut(), &keys) && !covers_previous {
                return Err(config_field(path, "configuration path does not exist"));
            }
            removed_previous |= covers_previous;
        }
    }

    resolve_patch_model_keys(&mut doc, &mut patch, &environment)?;
    merge_tables(doc.as_table_mut(), patch.as_table(), true);
    if let Some((previous, raw_model)) = previous {
        let is_still_default =
            read_default(&doc, &environment).is_some_and(|(pair, _)| pair == previous);
        let registered = model_table(&doc, &previous.connection).is_some_and(|models| {
            registered_model_key(models, &previous.model, &environment).is_some()
        });
        if removed_previous && is_still_default && !registered {
            return Err(config_field(
                "default_model.model",
                "cannot delete the default model without selecting a replacement",
            ));
        }
        if !removed_previous
            && !is_still_default
            && !registered
            && has_connection(&doc, &previous.connection)
        {
            let model = ensure_table(doc.as_table_mut(), "model")?;
            let connection = ensure_table(model, &previous.connection)?;
            connection.insert(&raw_model, Item::Table(Table::new()));
        }
    }
    Ok(doc.to_string())
}

fn read_default(
    doc: &DocumentMut,
    environment: &impl Fn(&str) -> EnvValue,
) -> Option<(ModelRef, String)> {
    let item = doc.get("default_model")?;
    let raw_model = item.as_table_like()?.get("model")?.as_str()?;
    let mut pair = DocumentMut::new();
    pair["default_model"] = item.clone();
    let resolved = parse_config_text_with_env(&pair.to_string(), environment)
        .ok()?
        .default_model?;
    Some((resolved, raw_model.to_owned()))
}

fn has_connection(doc: &DocumentMut, name: &str) -> bool {
    doc.get("provider")
        .and_then(Item::as_table_like)
        .is_some_and(|providers| providers.contains_key(name))
}

type EnvValue = std::result::Result<String, std::env::VarError>;

fn model_table<'a>(doc: &'a DocumentMut, connection: &str) -> Option<&'a dyn TableLike> {
    doc.get("model")?
        .as_table_like()?
        .get(connection)?
        .as_table_like()
}

fn registered_model_key<'a>(
    models: &'a dyn TableLike,
    model: &str,
    environment: &impl Fn(&str) -> EnvValue,
) -> Option<&'a str> {
    models
        .iter()
        .find_map(|(name, _)| model_key_matches(name, model, environment).then_some(name))
}

fn model_key_matches(key: &str, model: &str, environment: &impl Fn(&str) -> EnvValue) -> bool {
    if key.starts_with('$') {
        resolve_model_key(key, environment).as_deref() == Some(model)
    } else {
        key == model
    }
}

fn resolve_model_key(key: &str, environment: &impl Fn(&str) -> EnvValue) -> Option<String> {
    key.strip_prefix('$')
        .and_then(|name| environment(name).ok())
}

fn removes_model(
    keys: &[Key],
    model: &ModelRef,
    raw_model: &str,
    environment: &impl Fn(&str) -> EnvValue,
) -> bool {
    let path = keys.iter().map(Key::get).collect::<Vec<_>>();
    match path.as_slice() {
        ["model"] => true,
        ["model", connection] => *connection == model.connection,
        ["model", connection, id] => {
            *connection == model.connection
                && (*id == raw_model || model_key_matches(id, &model.model, environment))
        }
        _ => false,
    }
}

fn resolve_delete_path(
    doc: &DocumentMut,
    mut keys: Vec<Key>,
    environment: &impl Fn(&str) -> EnvValue,
) -> Vec<Key> {
    if let [root, connection, id, ..] = keys.as_mut_slice()
        && root.get() == "model"
        && let Some(models) = model_table(doc, connection.get())
        && !models.contains_key(id.get())
    {
        let model = resolve_model_key(id.get(), environment).unwrap_or_else(|| id.get().to_owned());
        if let Some(raw_key) = registered_model_key(models, &model, environment) {
            *id = Key::new(raw_key);
        }
    }
    keys
}

fn resolve_patch_model_keys(
    doc: &mut DocumentMut,
    patch: &mut DocumentMut,
    environment: &impl Fn(&str) -> EnvValue,
) -> Result<()> {
    let (Some(models), Some(changes)) = (
        doc.get_mut("model").and_then(Item::as_table_like_mut),
        patch.get_mut("model").and_then(Item::as_table_like_mut),
    ) else {
        return Ok(());
    };
    for (connection, entry) in changes.iter_mut() {
        let Some(registrations) = entry.as_table_like_mut() else {
            continue;
        };
        let Some(existing) = models
            .get_mut(connection.get())
            .and_then(Item::as_table_like_mut)
        else {
            continue;
        };
        let aliases = registrations
            .iter()
            .filter_map(|(id, _)| {
                if existing.contains_key(id) {
                    return None;
                }
                let model = resolve_model_key(id, environment).unwrap_or_else(|| id.to_owned());
                registered_model_key(existing, &model, environment)
                    .map(|key| (id.to_owned(), key.to_owned()))
            })
            .collect::<Vec<_>>();
        let mut targets = BTreeSet::new();
        for (incoming, target) in &aliases {
            if registrations.contains_key(target) || !targets.insert(target) {
                return Err(config_field(
                    ModelRef::new(connection.get(), incoming).config_path(),
                    format!("patch names the same model twice through {target:?}"),
                ));
            }
            if incoming.starts_with('$') {
                // Keep the requested reference so later config loads resolve the same table.
                if let Some(item) = existing.remove(target) {
                    existing.insert(incoming, item);
                }
            } else if let Some(item) = registrations.remove(incoming) {
                registrations.insert(target, item);
            }
        }
    }
    Ok(())
}

fn ensure_table<'a>(parent: &'a mut dyn TableLike, key: &str) -> Result<&'a mut dyn TableLike> {
    if !parent.contains_key(key) {
        let mut table = Table::new();
        table.set_implicit(true);
        parent.insert(key, Item::Table(table));
    }
    parent
        .get_mut(key)
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| config_field(key, "expected a table"))
}

fn remove_path(table: &mut dyn TableLike, keys: &[Key]) -> bool {
    match keys.split_first() {
        Some((key, [])) => table.remove(key.get()).is_some(),
        Some((key, rest)) => table
            .get_mut(key.get())
            .and_then(Item::as_table_like_mut)
            .is_some_and(|child| remove_path(child, rest)),
        None => false,
    }
}

fn merge_tables(target: &mut dyn TableLike, patch: &dyn TableLike, root: bool) {
    for (key, item) in patch.iter() {
        if !(root && key == "default_model")
            && let Some(child) = target.get_mut(key).and_then(Item::as_table_like_mut)
            && let Some(changes) = item.as_table_like()
        {
            merge_tables(child, changes, false);
        } else {
            replace_item(target, key, item.clone());
        }
    }
}

fn replace_item(table: &mut dyn TableLike, key: &str, mut item: Item) {
    if let Some(previous) = table.get_mut(key).and_then(Item::as_value_mut)
        && let Some(replacement) = item.as_value_mut()
    {
        *replacement.decor_mut() = previous.decor().clone();
        *previous = replacement.clone();
    } else {
        table.insert(key, item);
    }
}

fn parse_document(raw: &str) -> Result<DocumentMut> {
    raw.parse::<DocumentMut>()
        .map_err(|error| Error::ConfigLoad(error.message().to_owned()))
}

fn config_field(field: impl Into<String>, message: impl Into<String>) -> Error {
    Error::ConfigField {
        field: field.into(),
        message: message.into(),
    }
}

/// Edit one scalar value using a TOML dotted path, preserving comments and formatting.
pub fn config_text_with_value(raw: &str, dot_key: &str, value: &str) -> Result<String> {
    let mut doc = parse_document(raw)?;
    let keys = Key::parse(dot_key).map_err(|error| config_field(dot_key, error.message()))?;
    let (leaf, parents) = keys
        .split_last()
        .ok_or_else(|| config_field(dot_key, "empty config key"))?;
    let mut table: &mut dyn TableLike = doc.as_table_mut();
    for key in parents {
        table = table
            .get_mut(key.get())
            .and_then(Item::as_table_like_mut)
            .ok_or_else(|| {
                config_field(
                    dot_key,
                    format!("unknown config key: table {key} does not exist"),
                )
            })?;
    }
    let item = match leaf.get() {
        "context_window" | "max_output_tokens" | "max_age_days" | "max_total_size_mb"
        | "keep_turns" => {
            let n = value
                .parse::<i64>()
                .ok()
                .filter(|n| *n > 0)
                .ok_or_else(|| {
                    config_field(
                        dot_key,
                        format!("expected a positive integer, got {value:?}"),
                    )
                })?;
            toml_edit::value(n)
        }
        "think" => {
            ThinkLevel::from_str(value)
                .map_err(|error| config_field(dot_key, error.to_string()))?;
            toml_edit::value(value)
        }
        "format" => {
            value
                .parse::<super::types::ProviderFormat>()
                .map_err(|message| config_field(dot_key, message))?;
            toml_edit::value(value)
        }
        _ => toml_edit::value(value),
    };
    if keys.first().is_some_and(|key| key.get() == "default_model") {
        return Err(config_field(
            dot_key,
            "use a patch to update the complete default model pair",
        ));
    }
    replace_item(table, leaf.get(), item);
    Ok(doc.to_string())
}

/// Validate and publish a single scalar edit to an existing configuration file.
pub fn set_value(path: &Path, dot_key: &str, value: &str) -> Result<()> {
    let raw = fs::read_to_string(path)?;
    let modified = config_text_with_value(&raw, dot_key, value)?;
    parse_config_text(&modified)?.resolve()?;
    publish_config(path, Some(raw.as_bytes()), None, modified.as_bytes(), || {})?;
    Ok(())
}
