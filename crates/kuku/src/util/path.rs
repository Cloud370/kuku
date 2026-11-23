use std::collections::VecDeque;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::{Error, Result};

const MAX_WORKSPACE_SCAN: usize = 20_000;

pub(crate) fn normalize_path_sep(path: &str) -> String {
    path.replace('\\', "/")
}

pub(crate) fn is_blocked_relative_path(path: &str) -> bool {
    path.split(['/', '\\'])
        .any(|part| part == ".git" || part == ".ssh")
        || path
            .rsplit(['/', '\\'])
            .next()
            .is_some_and(is_sensitive_file_name)
}

pub(crate) fn is_sensitive_file_name(name: &str) -> bool {
    matches!(
        name,
        ".env"
            | "credentials.json"
            | "credentials.toml"
            | "id_rsa"
            | "id_dsa"
            | "id_ecdsa"
            | "id_ed25519"
    ) || name.starts_with(".env.")
        || name.ends_with(".pem")
        || name.ends_with(".key")
        || name.ends_with(".p12")
        || name.ends_with(".pfx")
}

pub(crate) fn is_default_excluded_dir(name: &str) -> bool {
    matches!(
        name,
        ".git" | "target" | "node_modules" | "__pycache__" | ".venv" | "venv" | "dist" | "build"
    )
}

/// Find up to `limit` visible relative path suggestions within a server-side workspace.
pub fn visible_workspace_paths(workspace: &Path, query: &str, limit: usize) -> Result<Vec<String>> {
    let query = Path::new(query);
    if query
        .components()
        .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
    {
        return Err(Error::InvalidArgument(
            "path query must be relative to the workspace".into(),
        ));
    }
    let workspace = workspace.canonicalize()?;
    let query = display_path(query).unwrap_or_default().to_lowercase();
    let mut directories = VecDeque::from([workspace.clone()]);
    let mut results = Vec::new();
    let mut remaining = MAX_WORKSPACE_SCAN;
    while let Some(directory) = directories.pop_front() {
        if remaining == 0 || results.len() >= limit {
            break;
        }
        if directory
            .canonicalize()
            .is_ok_and(|resolved| resolved.starts_with(&workspace))
        {
            find_visible_entries(
                &directory,
                &workspace,
                &query,
                &mut directories,
                &mut results,
                &mut remaining,
                limit,
            )?;
        }
    }
    results.sort_unstable();
    Ok(results)
}

fn find_visible_entries(
    directory: &Path,
    workspace: &Path,
    query: &str,
    directories: &mut VecDeque<PathBuf>,
    results: &mut Vec<String>,
    remaining: &mut usize,
    limit: usize,
) -> Result<()> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if directory == workspace => return Err(error.into()),
        Err(_) => return Ok(()),
    };
    for entry in entries.take(*remaining) {
        if results.len() >= limit {
            break;
        }
        *remaining -= 1;
        let Ok(entry) = entry else {
            continue;
        };
        let kind = match entry.file_type() {
            Ok(kind) if kind.is_dir() || kind.is_file() => kind,
            _ => continue,
        };
        let path = entry.path();
        let Some(relative) = path.strip_prefix(workspace).ok().and_then(display_path) else {
            continue;
        };
        if is_blocked_relative_path(&relative) {
            continue;
        }
        if kind.is_dir() {
            if entry
                .file_name()
                .to_str()
                .is_some_and(is_default_excluded_dir)
            {
                continue;
            }
            directories.push_back(path);
        }
        if relative.to_lowercase().contains(query) {
            results.push(if kind.is_dir() {
                format!("{relative}/")
            } else {
                relative
            });
        }
    }
    Ok(())
}

fn display_path(path: &Path) -> Option<String> {
    path.components()
        .filter_map(|part| match part {
            Component::Normal(name) => Some(name.to_str()),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()
        .map(|parts| parts.join("/"))
}
