use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use super::manifest::HookDecl;
use super::matcher::MatcherExpr;

use crate::error::Error;

/// Lifecycle event that triggers plugin hook execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HookEvent {
    SessionStart,
    SessionEnd,
    ToolPreExecute,
    ToolPostExecute,
    ModelPreRequest,
    ModelPostResponse,
}

impl std::str::FromStr for HookEvent {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, String> {
        match s {
            "session.start" => Ok(Self::SessionStart),
            "session.end" => Ok(Self::SessionEnd),
            "tool.pre_execute" => Ok(Self::ToolPreExecute),
            "tool.post_execute" => Ok(Self::ToolPostExecute),
            "model.pre_request" => Ok(Self::ModelPreRequest),
            "model.post_response" => Ok(Self::ModelPostResponse),
            _ => Err(format!("unknown hook event '{s}'")),
        }
    }
}

impl HookEvent {
    /// Return the wire-format event name (e.g. `"tool.pre_execute"`).
    #[allow(dead_code)]
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::SessionStart => "session.start",
            Self::SessionEnd => "session.end",
            Self::ToolPreExecute => "tool.pre_execute",
            Self::ToolPostExecute => "tool.post_execute",
            Self::ModelPreRequest => "model.pre_request",
            Self::ModelPostResponse => "model.post_response",
        }
    }
}

const DEFAULT_TIMEOUT: u64 = 30;
const MAX_TIMEOUT: u64 = 600;

/// A resolved hook ready for execution, built from a manifest declaration.
#[derive(Debug, Clone)]
pub(crate) struct HookInstance {
    pub(crate) event: HookEvent,
    pub(crate) command: PathBuf,
    pub(crate) matcher: Option<MatcherExpr>,
    pub(crate) timeout: Duration,
    pub(crate) chain: bool,
    pub(crate) package_name: String,
    pub(crate) package_root: PathBuf,
    pub(crate) env: Vec<String>,
}

/// Build hook instances from a package manifest, resolving events and matchers.
pub(crate) fn build_hook_instances(
    manifest: &super::manifest::PackageManifest,
    package_root: &std::path::Path,
    package_name: &str,
) -> crate::error::Result<Vec<HookInstance>> {
    let mut instances = Vec::new();

    for (i, decl) in manifest.hooks.iter().enumerate() {
        let events = resolve_events(decl).map_err(|e| {
            crate::error::Error::PluginManifest(
                package_root.join("kuku.toml"),
                format!("hook {i}: {e}"),
            )
        })?;

        let matcher = match &decl.matcher {
            Some(expr) => Some(super::matcher::parse(expr).map_err(|e| {
                crate::error::Error::PluginManifest(
                    package_root.join("kuku.toml"),
                    format!("hook {i}: matcher parse error: {e}"),
                )
            })?),
            None => None,
        };

        let timeout_secs = decl
            .timeout_seconds
            .unwrap_or(DEFAULT_TIMEOUT)
            .clamp(1, MAX_TIMEOUT);
        let command = resolve_hook_command(package_root, &decl.command).map_err(|message| {
            Error::PluginManifest(
                package_root.join("kuku.toml"),
                format!("hook {i}: {message}"),
            )
        })?;

        for event in events {
            instances.push(HookInstance {
                event,
                command: command.clone(),
                matcher: matcher.clone(),
                timeout: Duration::from_secs(timeout_secs),
                chain: decl.chain,
                package_name: package_name.to_string(),
                package_root: package_root.to_path_buf(),
                env: decl.env.clone().unwrap_or_default(),
            });
        }
    }

    Ok(instances)
}

fn resolve_hook_command(
    package_root: &Path,
    command: &str,
) -> std::result::Result<PathBuf, String> {
    let command_path = Path::new(command);
    if command_path.is_absolute() {
        return Err("command must stay under the package root".to_string());
    }

    let mut resolved = PathBuf::from(package_root);
    for component in command_path.components() {
        match component {
            Component::Normal(part) => resolved.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("command must stay under the package root".to_string());
            }
        }
    }

    Ok(resolved)
}

fn resolve_events(decl: &HookDecl) -> std::result::Result<Vec<HookEvent>, String> {
    if let Some(ref ev) = decl.event {
        let event: HookEvent = ev.parse()?;
        Ok(vec![event])
    } else if let Some(ref events) = decl.events {
        let mut result = Vec::new();
        for ev in events {
            let event: HookEvent = ev.parse()?;
            result.push(event);
        }
        Ok(result)
    } else {
        Err("must have either 'event' or 'events'".into())
    }
}

#[cfg(test)]
mod tests {
    use super::super::manifest::{HookDecl, PackageManifest, PackageMeta};
    use super::*;

    fn manifest_with_hook(event: &str, command: &str) -> PackageManifest {
        PackageManifest {
            package: PackageMeta {
                name: "test".into(),
                version: "1.0.0".into(),
                description: None,
                homepage: None,
                repository: None,
            },
            hooks: vec![HookDecl {
                event: Some(event.into()),
                events: None,
                command: command.into(),
                matcher: None,
                timeout_seconds: None,
                chain: false,
                env: None,
            }],
        }
    }

    #[test]
    fn build_single_event() {
        let m = manifest_with_hook("tool.pre_execute", "hooks/check.sh");
        let instances = build_hook_instances(&m, std::path::Path::new("/pkg"), "test").unwrap();
        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0].event, HookEvent::ToolPreExecute);
        assert_eq!(instances[0].command, PathBuf::from("/pkg/hooks/check.sh"));
        assert_eq!(instances[0].timeout, Duration::from_secs(30));
    }

    #[test]
    fn build_multi_event() {
        let m = PackageManifest {
            package: PackageMeta {
                name: "test".into(),
                version: "1.0.0".into(),
                description: None,
                homepage: None,
                repository: None,
            },
            hooks: vec![HookDecl {
                event: None,
                events: Some(vec!["session.start".into(), "session.end".into()]),
                command: "hooks/lifecycle.sh".into(),
                matcher: None,
                timeout_seconds: None,
                chain: false,
                env: None,
            }],
        };
        let instances = build_hook_instances(&m, std::path::Path::new("/pkg"), "test").unwrap();
        assert_eq!(instances.len(), 2);
        assert_eq!(instances[0].event, HookEvent::SessionStart);
        assert_eq!(instances[1].event, HookEvent::SessionEnd);
    }

    #[test]
    fn timeout_clamping() {
        let mut m = manifest_with_hook("session.start", "hooks/a.sh");
        m.hooks[0].timeout_seconds = Some(0);
        let instances = build_hook_instances(&m, std::path::Path::new("/pkg"), "t").unwrap();
        assert_eq!(instances[0].timeout, Duration::from_secs(1));

        m.hooks[0].timeout_seconds = Some(999999);
        let instances = build_hook_instances(&m, std::path::Path::new("/pkg"), "t").unwrap();
        assert_eq!(instances[0].timeout, Duration::from_secs(600));
    }

    #[test]
    fn hook_event_from_str_round_trip() {
        let events = [
            "session.start",
            "session.end",
            "tool.pre_execute",
            "tool.post_execute",
            "model.pre_request",
            "model.post_response",
        ];
        for s in &events {
            let ev: HookEvent = s.parse().unwrap();
            assert_eq!(ev.as_str(), *s);
        }
        assert!("unknown.event".parse::<HookEvent>().is_err());
    }

    #[test]
    fn rejects_relative_hook_command_that_escapes_package_root() {
        let m = manifest_with_hook("tool.pre_execute", "../../run-me.sh");

        let error = build_hook_instances(&m, std::path::Path::new("/pkg/plugins/test"), "test")
            .unwrap_err();

        assert!(matches!(
            error,
            crate::error::Error::PluginManifest(_, message)
                if message.contains("hook 0") && message.contains("package root")
        ));
    }

    #[test]
    fn rejects_absolute_hook_command_path() {
        let m = manifest_with_hook("tool.pre_execute", "/tmp/run-me.sh");

        let error = build_hook_instances(&m, std::path::Path::new("/pkg/plugins/test"), "test")
            .unwrap_err();

        assert!(matches!(
            error,
            crate::error::Error::PluginManifest(_, message)
                if message.contains("hook 0") && message.contains("package root")
        ));
    }
}
