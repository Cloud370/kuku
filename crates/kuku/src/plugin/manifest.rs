use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Parsed contents of a plugin's `kuku.toml` manifest file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PackageManifest {
    pub(crate) package: PackageMeta,
    #[serde(default)]
    pub(crate) hooks: Vec<HookDecl>,
}

/// Package identity metadata from the manifest `[package]` section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PackageMeta {
    pub(crate) name: String,
    pub(crate) version: String,
    #[serde(default)]
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) homepage: Option<String>,
    #[serde(default)]
    pub(crate) repository: Option<String>,
}

/// A single hook declaration from the manifest `[[hooks]]` section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct HookDecl {
    #[serde(default)]
    pub(crate) event: Option<String>,
    #[serde(default)]
    pub(crate) events: Option<Vec<String>>,
    pub(crate) command: String,
    #[serde(default)]
    pub(crate) matcher: Option<String>,
    #[serde(default)]
    pub(crate) timeout_seconds: Option<u64>,
    #[serde(default)]
    pub(crate) chain: bool,
    #[serde(default)]
    pub(crate) env: Option<Vec<String>>,
}

const NAME_PATTERN: &str = r"^[a-z][a-z0-9-]{0,63}$";
const VALID_EVENTS: &[&str] = &[
    "session.start",
    "session.end",
    "tool.pre_execute",
    "tool.post_execute",
    "model.pre_request",
    "model.post_response",
];

static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(NAME_PATTERN).unwrap());

/// Parse and validate a TOML manifest string into a `PackageManifest`.
pub(crate) fn parse_manifest(content: &str, path: &std::path::Path) -> Result<PackageManifest> {
    let manifest: PackageManifest = toml::from_str(content)
        .map_err(|e| Error::PluginManifest(path.to_path_buf(), format!("TOML parse error: {e}")))?;
    validate_manifest(&manifest, path)?;
    Ok(manifest)
}

/// Validate package name format and hook event declarations.
pub(crate) fn validate_manifest(manifest: &PackageManifest, path: &std::path::Path) -> Result<()> {
    let name = &manifest.package.name;
    if !RE.is_match(name) {
        return Err(Error::PluginManifest(
            path.to_path_buf(),
            format!("invalid package name '{name}': must match {NAME_PATTERN}"),
        ));
    }

    for (i, hook) in manifest.hooks.iter().enumerate() {
        match (&hook.event, &hook.events) {
            (Some(_), Some(_)) => {
                return Err(Error::PluginManifest(
                    path.to_path_buf(),
                    format!("hook {i}: 'event' and 'events' are mutually exclusive"),
                ));
            }
            (None, None) => {
                return Err(Error::PluginManifest(
                    path.to_path_buf(),
                    format!("hook {i}: must have either 'event' or 'events'"),
                ));
            }
            _ => {}
        }

        if let Some(events) = &hook.events {
            for ev in events {
                if !VALID_EVENTS.contains(&ev.as_str()) {
                    return Err(Error::PluginManifest(
                        path.to_path_buf(),
                        format!("hook {i}: unknown event '{ev}'"),
                    ));
                }
            }
        }
        if let Some(ev) = &hook.event {
            if !VALID_EVENTS.contains(&ev.as_str()) {
                return Err(Error::PluginManifest(
                    path.to_path_buf(),
                    format!("hook {i}: unknown event '{ev}'"),
                ));
            }
        }
    }

    Ok(())
}

/// Compute a deterministic SHA-256 hash of the manifest for change detection.
pub(crate) fn compute_manifest_hash(manifest: &PackageManifest) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(manifest.package.name.as_bytes());
    hasher.update(b"|");
    hasher.update(manifest.package.version.as_bytes());
    for hook in &manifest.hooks {
        hasher.update(b"|");
        hasher.update(hook.command.as_bytes());
        if let Some(ref ev) = hook.event {
            hasher.update(b":");
            hasher.update(ev.as_bytes());
        }
        if let Some(ref events) = hook.events {
            for ev in events {
                hasher.update(b":");
                hasher.update(ev.as_bytes());
            }
        }
    }
    format!("sha256:{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn valid_toml() -> &'static str {
        r#"
[package]
name = "test-pkg"
version = "1.0.0"

[[hooks]]
event = "tool.pre_execute"
command = "hooks/check.sh"
"#
    }

    #[test]
    fn parse_valid_manifest() {
        let m = parse_manifest(valid_toml(), &PathBuf::from("test")).unwrap();
        assert_eq!(m.package.name, "test-pkg");
        assert_eq!(m.hooks.len(), 1);
        assert_eq!(m.hooks[0].event, Some("tool.pre_execute".into()));
        assert_eq!(m.hooks[0].timeout_seconds, None);
        assert!(!m.hooks[0].chain);
    }

    #[test]
    fn reject_invalid_name() {
        let toml = r#"
[package]
name = "Invalid_Name"
version = "1.0.0"
"#;
        assert!(parse_manifest(toml, &PathBuf::from("test")).is_err());
    }

    #[test]
    fn reject_mutual_exclusive_event_events() {
        let toml = r#"
[package]
name = "test-pkg"
version = "1.0.0"

[[hooks]]
event = "session.start"
events = ["session.end"]
command = "hooks/a.sh"
"#;
        assert!(parse_manifest(toml, &PathBuf::from("test")).is_err());
    }

    #[test]
    fn reject_missing_event() {
        let toml = r#"
[package]
name = "test-pkg"
version = "1.0.0"

[[hooks]]
command = "hooks/a.sh"
"#;
        assert!(parse_manifest(toml, &PathBuf::from("test")).is_err());
    }

    #[test]
    fn hash_is_deterministic() {
        let m = parse_manifest(valid_toml(), &PathBuf::from("test")).unwrap();
        let h1 = compute_manifest_hash(&m);
        let h2 = compute_manifest_hash(&m);
        assert_eq!(h1, h2);
        assert!(h1.starts_with("sha256:"));
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let toml = r#"
[package]
name = "test-pkg"
version = "1.0.0"
future_field = "ignored"

[[hooks]]
event = "session.start"
command = "hooks/a.sh"
unknown_hook_field = 42
"#;
        let m = parse_manifest(toml, &PathBuf::from("test")).unwrap();
        assert_eq!(m.hooks.len(), 1);
    }

    #[test]
    fn multi_event_hook() {
        let toml = r#"
[package]
name = "test-pkg"
version = "1.0.0"

[[hooks]]
events = ["session.start", "session.end"]
command = "hooks/lifecycle.sh"
"#;
        let m = parse_manifest(toml, &PathBuf::from("test")).unwrap();
        assert_eq!(m.hooks[0].events.as_ref().unwrap().len(), 2);
    }
}
