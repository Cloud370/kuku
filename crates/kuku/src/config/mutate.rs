use std::io::Write as _;
use std::path::Path;
use std::str::FromStr;

use super::resolve::parse_config_file;
use crate::config::types::{ConfigFile, ThinkLevel};
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
    let file: ConfigFile = parse_config_file(raw)?;

    if file.handoff.is_some() && file.plugin.is_some() && file.update.is_some() {
        return Ok((raw.to_string(), false));
    }

    let mut doc: toml_edit::DocumentMut = raw
        .parse()
        .map_err(|error| Error::ConfigLoad(format!("invalid config: {error}")))?;

    if file.handoff.is_none() {
        inject_handoff_section(&mut doc);
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
    section["enabled"] = toml_edit::value(true);
    section["threshold"] = toml_edit::value(0.7);
    section["keep_turns"] = toml_edit::value(2);

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

fn inject_update_section(doc: &mut toml_edit::DocumentMut) {
    let mut section = toml_edit::Table::new();
    *section.decor_mut() = toml_edit::Decor::new(
        "\n\n# Update system: self-update source, channel, and custom mirrors.\n",
        "",
    );
    section["source"] = toml_edit::value("github");
    section["channel"] = toml_edit::value("stable");
    doc["update"] = toml_edit::Item::Table(section);
}

/// Load config from disk, patch missing sections, write back atomically if changed.
/// Returns the resolved ConfigFile. Preserves user comments via `toml_edit`.
pub fn load_and_patch_config(path: &Path) -> Result<ConfigFile> {
    let raw = std::fs::read_to_string(path)
        .map_err(|error| Error::ConfigLoad(format!("cannot read config file {path:?}: {error}")))?;
    let (patched, changed) = config_patch_defaults(&raw)?;
    if changed {
        // Guard against concurrent edits: only write back if the file hasn't
        // changed since we read it, so we don't overwrite user modifications.
        if let Ok(current) = std::fs::read_to_string(path) {
            if current == raw {
                let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
                let mut temp = tempfile::NamedTempFile::new_in(dir).map_err(|error| {
                    Error::ConfigLoad(format!("cannot create temp file: {error}"))
                })?;
                std::io::Write::write_all(&mut temp, patched.as_bytes())
                    .map_err(|error| Error::ConfigLoad(format!("cannot write config: {error}")))?;
                temp.persist(path)
                    .map_err(|error| Error::ConfigLoad(format!("cannot save config: {error}")))?;
            }
        }
    }
    parse_config_file(&patched)
}

/// Modify a single value in a config file by dot-notation key.
///
/// Validates the modified config structurally (cross-references, types, enum values).
/// Does NOT validate runtime constraints (env var existence).
/// Preserves comments and formatting via `toml_edit`.
pub fn set_value(path: &Path, dot_key: &str, value: &str) -> Result<()> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| Error::ConfigLoad(format!("cannot read config: {error}")))?;

    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|error| Error::ConfigLoad(format!("invalid config: {error}")))?;

    let parts: Vec<&str> = dot_key.split('.').collect();
    if parts.is_empty() {
        return Err(Error::ConfigLoad("empty config key".to_string()));
    }

    let (parent_parts, leaf_key) = parts.split_at(parts.len() - 1);
    let leaf = leaf_key[0];

    let mut table = doc.as_table_mut();
    for part in parent_parts {
        let item = table.get_mut(part).ok_or_else(|| {
            Error::ConfigLoad(format!("unknown config key: segment '{part}' not found"))
        })?;
        table = item.as_table_mut().ok_or_else(|| {
            Error::ConfigLoad(format!(
                "unknown config key: segment '{part}' is not a table"
            ))
        })?;
    }

    let new_value = match leaf {
        "context_window" | "max_output_tokens" => {
            let n: i64 = value.parse().map_err(|_| {
                Error::ConfigLoad(format!(
                    "'{dot_key}' must be a positive integer, got '{value}'"
                ))
            })?;
            if n <= 0 {
                return Err(Error::ConfigLoad(format!(
                    "'{dot_key}' must be a positive integer, got '{value}'"
                )));
            }
            toml_edit::value(n)
        }
        "think" => {
            ThinkLevel::from_str(value)?;
            toml_edit::value(value)
        }
        "format" => {
            value
                .parse::<super::types::ProviderFormat>()
                .map_err(|msg| Error::ConfigLoad(format!("{msg}, got '{value}'")))?;
            toml_edit::value(value)
        }
        _ => toml_edit::value(value),
    };

    table[leaf] = new_value;

    let modified_text = doc.to_string();
    let config_file: ConfigFile = toml::from_str(&modified_text)
        .map_err(|error| Error::ConfigLoad(format!("modified config is invalid: {error}")))?;
    config_file.validate_all()?;

    let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let mut temp = tempfile::NamedTempFile::new_in(dir)
        .map_err(|error| Error::ConfigLoad(format!("cannot create temp file: {error}")))?;
    temp.write_all(modified_text.as_bytes())
        .map_err(|error| Error::ConfigLoad(format!("cannot write config: {error}")))?;
    temp.persist(path)
        .map_err(|error| Error::ConfigLoad(format!("cannot save config: {error}")))?;

    Ok(())
}
