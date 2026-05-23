use std::path::{Component, Path, PathBuf, Prefix};

use home::env::{self, Env};

use crate::error::{Error, Result};

/// Resolve the kuku home directory from KUKU_HOME env or platform default.
pub fn kuku_home() -> Result<PathBuf> {
    kuku_home_with_env(&env::OS_ENV)
}

/// Resolve and canonicalize the current working directory as the workspace root.
pub fn current_workspace() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    std::fs::canonicalize(&cwd).map_err(|_| Error::InvalidWorkspacePath(cwd.display().to_string()))
}

fn kuku_home_with_env(env: &dyn Env) -> Result<PathBuf> {
    if let Some(value) = env.var_os("KUKU_HOME") {
        if value.is_empty() {
            return Err(Error::InvalidKukuHome(String::new()));
        }
        return Ok(PathBuf::from(value));
    }

    env::home_dir_with_env(env)
        .map(|home| home.join(".kuku"))
        .ok_or(Error::MissingHomeDirectory)
}

/// Compute the project-scoped home directory for a workspace.
pub fn project_home(kuku_home: &Path, workspace: &Path) -> Result<PathBuf> {
    let mut path = PathBuf::from(kuku_home);
    path.push("p");

    for component in workspace.components() {
        match component {
            Component::RootDir => {}
            Component::Prefix(prefix) => push_prefix(&mut path, &prefix),
            Component::Normal(part) => path.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(Error::InvalidWorkspacePath(workspace.display().to_string()));
            }
        }
    }

    Ok(path)
}

/// Reconstruct a workspace path from a project_home directory.
/// Inverse of `project_home` — best-effort, lossy for DeviceNS/Verbatim prefixes.
pub(crate) fn workspace_from_project_home(p_dir: &Path, project_home: &Path) -> PathBuf {
    let rel = project_home.strip_prefix(p_dir).unwrap_or(project_home);
    let mut components = rel.components().peekable();
    let Some(first) = components.peek() else {
        return PathBuf::from("/");
    };
    let _first_str = first.as_os_str().to_string_lossy();

    #[cfg(windows)]
    {
        if _first_str.len() == 1 && _first_str.chars().all(|c| c.is_ascii_uppercase()) {
            let _ = components.next();
            let mut result = PathBuf::from(format!("{first_str}:\\"));
            for c in components {
                result.push(c);
            }
            return result;
        }
        if components.clone().count() >= 2 {
            let _first = components.next();
            let second = components.next().unwrap();
            let second_str = second.as_os_str().to_string_lossy();
            if !second_str.is_empty() {
                let mut result = PathBuf::from(format!("\\\\{first_str}\\{second_str}"));
                for c in components {
                    result.push(c);
                }
                return result;
            }
        }
    }

    let mut result = PathBuf::from("/");
    for c in components {
        result.push(c);
    }
    result
}

fn push_prefix(path: &mut PathBuf, prefix: &std::path::PrefixComponent<'_>) {
    match prefix.kind() {
        Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
            path.push(format!("{}", letter as char));
        }
        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
            path.push(server);
            path.push(share);
        }
        Prefix::DeviceNS(_) | Prefix::Verbatim(_) => {
            let id: String = prefix
                .as_os_str()
                .to_string_lossy()
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect();
            if !id.is_empty() {
                path.push(id);
            }
        }
    }
}

/// Resolve the events.jsonl path for a session within a workspace.
pub fn session_events_path(
    kuku_home: &Path,
    workspace: &Path,
    session_id: &str,
) -> Result<PathBuf> {
    let mut path = project_home(kuku_home, workspace)?;
    path.push("sessions");
    path.push(session_id);
    path.push("events.jsonl");
    Ok(path)
}

/// Resolve the policy.md path for a workspace.
pub fn project_policy_path(kuku_home: &Path, workspace: &Path) -> Result<PathBuf> {
    let mut path = project_home(kuku_home, workspace)?;
    path.push("policy.md");
    Ok(path)
}

/// Resolve the global memory.md path within kuku home.
pub fn global_memory_path(kuku_home: &Path) -> PathBuf {
    kuku_home.join("memory.md")
}

/// Resolve the project-scoped memory.md path for a workspace.
pub fn project_memory_path(kuku_home: &Path, workspace: &Path) -> Result<PathBuf> {
    let mut path = project_home(kuku_home, workspace)?;
    path.push("memory.md");
    Ok(path)
}

/// Resolve the lock file path for a session.
pub(crate) fn session_lock_path(kuku_home: &Path, workspace: &Path, session_id: &str) -> PathBuf {
    let Ok(mut path) = project_home(kuku_home, workspace) else {
        return PathBuf::new();
    };
    path.push("sessions");
    path.push(session_id);
    path.push("lock");
    path
}

#[cfg(test)]
mod tests {
    use super::kuku_home_with_env;
    use crate::error::Error;
    use home::env::Env;
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::io;
    use std::path::PathBuf;

    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt;

    #[derive(Default)]
    struct MockEnv {
        home_dir: Option<PathBuf>,
        vars: HashMap<&'static str, OsString>,
    }

    impl Env for MockEnv {
        fn home_dir(&self) -> Option<PathBuf> {
            self.home_dir.clone()
        }

        fn current_dir(&self) -> io::Result<PathBuf> {
            Ok(std::env::temp_dir())
        }

        fn var_os(&self, key: &str) -> Option<OsString> {
            self.vars.get(key).cloned()
        }
    }

    #[test]
    fn kuku_home_rejects_empty_kuku_home() {
        let mut env = MockEnv::default();
        env.vars.insert("KUKU_HOME", OsString::new());

        let error = kuku_home_with_env(&env).unwrap_err();

        assert!(matches!(error, Error::InvalidKukuHome(value) if value.is_empty()));
    }

    #[cfg(unix)]
    #[test]
    fn kuku_home_preserves_non_utf8_kuku_home() {
        let raw = OsString::from_vec(vec![
            b'/', b't', b'm', b'p', b'/', 0x80, b'h', b'o', b'm', b'e',
        ]);
        let mut env = MockEnv::default();
        env.vars.insert("KUKU_HOME", raw.clone());
        env.home_dir = Some(PathBuf::from("/should/not/use"));

        assert_eq!(kuku_home_with_env(&env).unwrap(), PathBuf::from(raw));
    }

    #[test]
    fn kuku_home_uses_platform_home_lookup_when_kuku_home_is_unset() {
        let env = MockEnv {
            home_dir: Some(PathBuf::from("/tmp/mock-home")),
            vars: HashMap::new(),
        };

        assert_eq!(
            kuku_home_with_env(&env).unwrap(),
            PathBuf::from("/tmp/mock-home").join(".kuku")
        );
    }

    #[cfg(unix)]
    #[test]
    fn kuku_home_uses_non_utf8_platform_home_losslessly() {
        let raw = OsString::from_vec(vec![
            b'/', b't', b'm', b'p', b'/', 0x80, b'u', b's', b'e', b'r',
        ]);
        let env = MockEnv {
            home_dir: Some(PathBuf::from(raw.clone())),
            vars: HashMap::new(),
        };

        assert_eq!(
            kuku_home_with_env(&env).unwrap(),
            PathBuf::from(raw).join(".kuku")
        );
    }

    #[test]
    fn kuku_home_returns_missing_home_directory_when_platform_lookup_fails() {
        let error = kuku_home_with_env(&MockEnv::default()).unwrap_err();

        assert!(matches!(error, Error::MissingHomeDirectory));
    }

    #[test]
    fn project_home_discards_root_dir_and_cur_dir() {
        let kuku_home = std::env::temp_dir().join("kuku-home");
        let workspace = std::path::PathBuf::from("/code/kuku/example");

        let path = super::project_home(&kuku_home, &workspace).unwrap();

        assert_eq!(path, kuku_home.join("p").join("code/kuku/example"));
    }

    #[test]
    fn project_home_rejects_parent_dir_in_workspace() {
        let kuku_home = std::env::temp_dir().join("kuku-home");
        let workspace = std::path::PathBuf::from("/code/../escape");

        let err = super::project_home(&kuku_home, &workspace).unwrap_err();

        assert!(matches!(err, crate::error::Error::InvalidWorkspacePath(_)));
    }
}
