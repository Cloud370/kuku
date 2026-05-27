use std::path::{Path, PathBuf};

use crate::config::DiscoveryConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    User,
    Project,
}

#[derive(Debug, Clone)]
pub struct DiscoveredEntry {
    pub path: PathBuf,
    pub scope: Scope,
}

#[derive(Debug, Default)]
pub struct DiscoveryResult {
    pub skills: Vec<DiscoveredEntry>,
    pub agents: Vec<DiscoveredEntry>,
}

const SKIP_DIRS: &[&str] = &[
    ".cache",
    ".cargo",
    ".local",
    ".npm",
    ".nvm",
    ".pyenv",
    ".rustup",
    ".ssh",
    ".gnupg",
    ".gconf",
    ".dbus",
    ".pki",
    ".icons",
    ".themes",
    ".fonts",
    ".mozilla",
    ".steam",
    "node_modules",
    ".Trash",
];

const SUBDIR_NAMES: &[&str] = &["skills", "agents", "agent"];

pub fn discover(workspace: &Path, config: &DiscoveryConfig) -> DiscoveryResult {
    let mut result = DiscoveryResult::default();

    if config.auto_discover {
        scan_xdg_user(&mut result);
        scan_dotfile_user(&mut result);
        scan_project(workspace, &mut result);
    }

    for p in &config.extra_user_paths {
        scan_dir_with_kinds(p, Scope::User, &mut result);
    }
    for p in &config.extra_project_paths {
        scan_dir_with_kinds(p, Scope::Project, &mut result);
    }

    result
}

fn home_dir() -> Option<PathBuf> {
    dirs_next()
}

fn scan_xdg_user(result: &mut DiscoveryResult) {
    let Some(home) = home_dir() else {
        return;
    };
    let config_dir = home.join(".config");
    let Ok(entries) = std::fs::read_dir(&config_dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.path().is_dir() {
            scan_dir_with_kinds(&entry.path(), Scope::User, result);
        }
    }
}

fn scan_dotfile_user(result: &mut DiscoveryResult) {
    let Some(home) = home_dir() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&home) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with('.') || !path.is_dir() {
            continue;
        }
        if SKIP_DIRS.contains(&name) {
            continue;
        }
        scan_dir_with_kinds(&path, Scope::User, result);
    }
}

fn scan_project(workspace: &Path, result: &mut DiscoveryResult) {
    let Ok(entries) = std::fs::read_dir(workspace) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with('.') || !path.is_dir() {
            continue;
        }
        scan_dir_with_kinds(&path, Scope::Project, result);
    }
}

fn scan_dir_with_kinds(dir: &Path, scope: Scope, result: &mut DiscoveryResult) {
    for kind in SUBDIR_NAMES {
        let candidate = dir.join(kind);
        if !candidate.is_dir() {
            continue;
        }
        let entry = DiscoveredEntry {
            path: candidate.clone(),
            scope,
        };
        if *kind == "skills" {
            result.skills.push(entry);
        } else {
            result.agents.push(entry);
        }
    }
}

fn dirs_next() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .or_else(|| {
            let drive = std::env::var_os("HOMEDRIVE")?;
            let path = std::env::var_os("HOMEPATH")?;
            Some(PathBuf::from(drive).join(path))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup_dir(base: &Path, paths: &[&str]) {
        for p in paths {
            fs::create_dir_all(base.join(p)).unwrap();
        }
    }

    #[test]
    fn scan_project_finds_dotdir_skills_and_agents() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        setup_dir(ws, &[".claude/skills", ".claude/agents", ".opencode/agent"]);

        let result = discover(
            ws,
            &DiscoveryConfig {
                auto_discover: true,
                ..Default::default()
            },
        );

        let project_skills: Vec<_> = result
            .skills
            .iter()
            .filter(|e| e.scope == Scope::Project)
            .collect();
        let project_agents: Vec<_> = result
            .agents
            .iter()
            .filter(|e| e.scope == Scope::Project)
            .collect();

        assert_eq!(project_skills.len(), 1);
        assert_eq!(project_agents.len(), 2);
    }

    #[test]
    fn extra_paths_are_included() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let extra = tmp.path().join("custom");
        setup_dir(&extra, &["skills"]);

        let config = DiscoveryConfig {
            auto_discover: false,
            extra_user_paths: vec![extra],
            extra_project_paths: vec![],
        };
        let result = discover(ws, &config);

        assert_eq!(result.skills.len(), 1);
        assert_eq!(result.skills[0].scope, Scope::User);
    }

    #[test]
    fn auto_discover_false_disables_builtin_rules() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        setup_dir(ws, &[".claude/skills"]);

        let config = DiscoveryConfig {
            auto_discover: false,
            ..Default::default()
        };
        let result = discover(ws, &config);

        assert!(result.skills.is_empty());
        assert!(result.agents.is_empty());
    }

    #[test]
    fn nonexistent_extra_path_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let config = DiscoveryConfig {
            auto_discover: false,
            extra_user_paths: vec![PathBuf::from("/nonexistent/path")],
            extra_project_paths: vec![],
        };
        let result = discover(tmp.path(), &config);
        assert!(result.skills.is_empty());
    }

    #[test]
    fn auto_discover_false_empty_extras_yields_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        setup_dir(tmp.path(), &[".claude/skills", ".claude/agents"]);
        let config = DiscoveryConfig {
            auto_discover: false,
            ..Default::default()
        };
        let result = discover(tmp.path(), &config);
        assert!(result.skills.is_empty());
        assert!(result.agents.is_empty());
    }

    #[test]
    fn skip_dirs_are_excluded_in_dotfile_scan() {
        let tmp = tempfile::tempdir().unwrap();
        setup_dir(tmp.path(), &[".cache/skills", ".cargo/agents"]);

        let config = DiscoveryConfig {
            auto_discover: true,
            extra_user_paths: vec![],
            extra_project_paths: vec![],
        };
        let result = discover(tmp.path(), &config);

        let user_skills: Vec<_> = result
            .skills
            .iter()
            .filter(|e| e.scope == Scope::User)
            .collect();
        let user_agents: Vec<_> = result
            .agents
            .iter()
            .filter(|e| e.scope == Scope::User)
            .collect();

        assert!(user_skills.is_empty());
        assert!(user_agents.is_empty());
    }
}
