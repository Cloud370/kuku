use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::tool::ToolResultEnvelope;

use super::common::{glob_match, is_default_excluded_dir, join_bounded_strings, relative_path};

const FIND_FILES_MAX_CHARS: usize = 8_000;

pub(crate) fn find_files(args: &Value, workspace: &Path) -> ToolResultEnvelope {
    let path = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let pattern = args.get("pattern").and_then(Value::as_str);
    let max_depth = args
        .get("max_depth")
        .and_then(Value::as_u64)
        .map(|v| v as u32)
        .unwrap_or(u32::MAX);
    let workspace = match workspace.canonicalize() {
        Ok(path) => path,
        Err(_) => {
            return ToolResultEnvelope::error(
                "failed: workspace not found",
                "workspace path does not exist",
            )
        }
    };
    let root = match workspace.join(path).canonicalize() {
        Ok(path) => path,
        Err(_) => {
            return ToolResultEnvelope::error(
                format!("failed: path not found: {path}"),
                format!("path does not exist: {path}"),
            )
        }
    };

    if !root.starts_with(&workspace) {
        return ToolResultEnvelope::blocked(
            format!("blocked: path outside workspace: {path}"),
            format!("path is outside the workspace: {path}"),
        );
    }

    let mut files = Vec::new();
    let mut dirs = Vec::new();
    if let Err(error) = collect_entries(
        &root, &workspace, pattern, max_depth, 0, &mut files, &mut dirs,
    ) {
        return ToolResultEnvelope::error(
            format!("failed: {error}"),
            format!("error reading directory: {path}"),
        );
    }
    files.sort();
    dirs.sort();
    dirs.sort_by_key(|d| d.matches('/').count());
    files.sort_by_key(|f| f.matches('/').count());

    let mut entries: Vec<String> = Vec::with_capacity(dirs.len() + files.len());
    entries.append(&mut dirs);
    entries.append(&mut files);

    let entry_count = entries.len();
    let (mut model_content, truncated) = join_bounded_strings(
        &entries,
        FIND_FILES_MAX_CHARS,
        "(Showing shallowest entries first. Use pattern or max_depth to focus.)",
    );
    if entries.is_empty() {
        model_content.clear();
    }

    let summary = if truncated {
        format!("found {entry_count} entries under {path}, results truncated")
    } else {
        format!("found {entry_count} entries under {path}")
    };
    let structured = serde_json::json!({
        "kind": "file_list",
        "path": path,
        "pattern": pattern,
        "entry_count": entry_count,
    });

    if truncated {
        ToolResultEnvelope::ok_truncated(summary, model_content, structured)
    } else {
        ToolResultEnvelope::ok(summary, model_content, structured)
    }
}

fn collect_entries(
    root: &Path,
    workspace: &Path,
    pattern: Option<&str>,
    max_depth: u32,
    depth: u32,
    files: &mut Vec<String>,
    dirs: &mut Vec<String>,
) -> std::io::Result<()> {
    if root.is_file() {
        push_entry(root, workspace, pattern, files);
        return Ok(());
    }
    if depth > max_depth {
        return Ok(());
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let file_name = entry.file_name();
        let file_name_str = file_name.to_string_lossy();
        let path = entry.path();

        if file_type.is_dir() {
            if is_default_excluded_dir(&file_name_str) {
                continue;
            }
            let (add_dir, recurse) = match pattern {
                None => (true, true),
                Some(p) if !p.contains('/') => (false, true),
                Some(p) => {
                    let rel = relative_path(&path, workspace);
                    let dir = format!("{rel}/");
                    let wildcard_prefix = p.split('*').next().unwrap_or("");
                    let dominated = if wildcard_prefix.is_empty() {
                        !p.contains("**")
                    } else {
                        !wildcard_prefix.starts_with(&dir) || wildcard_prefix.len() <= dir.len()
                    };
                    (!dominated, dir_should_traverse(p, &dir))
                }
            };
            if add_dir {
                if let Ok(relative) = path.strip_prefix(workspace) {
                    let rel = relative.to_string_lossy().replace('\\', "/");
                    dirs.push(format!("{rel}/"));
                }
            }
            if recurse {
                collect_entries(&path, workspace, pattern, max_depth, depth + 1, files, dirs)?;
            }
        } else if file_type.is_file() {
            push_entry(&path, workspace, pattern, files);
        }
    }
    Ok(())
}

fn dir_should_traverse(pattern: &str, dir: &str) -> bool {
    let pattern = pattern.replace('\\', "/");
    let dir = dir.replace('\\', "/");
    if pattern.starts_with("**/") {
        return true;
    }
    if let Some(after) = pattern.strip_prefix("**") {
        return after.is_empty() || after.starts_with('/');
    }
    let wildcard_prefix = pattern.split('*').next().unwrap_or("");
    if wildcard_prefix.is_empty() {
        return pattern.contains("**");
    }
    wildcard_prefix.starts_with(&dir)
}

fn push_entry(path: &Path, workspace: &Path, pattern: Option<&str>, files: &mut Vec<String>) {
    let Ok(relative) = path.strip_prefix(workspace) else {
        return;
    };
    let relative = relative.to_string_lossy().replace('\\', "/");

    if pattern.is_some_and(|p| !glob_match(p, &relative)) {
        return;
    }

    files.push(relative);
}

#[cfg(test)]
mod tests {
    use super::super::test_helpers::workspace;
    use super::*;

    #[test]
    fn find_files_returns_depth_sorted_entries_with_dirs_first() {
        let dir = workspace();
        let result = find_files(&serde_json::json!({"path": "."}), dir.path());

        assert_eq!(result.status, "ok");
        let lines: Vec<_> = result.model_content.lines().collect();
        assert!(lines.contains(&"docs/"), "dirs with trailing /: {lines:?}");
        assert!(lines.contains(&"README.md"));
        assert!(lines.contains(&"docs/tools.md"));
        assert!(lines.contains(&"src/main.rs"));
        assert!(!result.model_content.contains(".git"));
        assert!(result.structured.unwrap()["entry_count"].as_u64().unwrap() >= 3);
    }

    #[test]
    fn find_files_filters_pattern_globs_and_blocks_outside_workspace() {
        let dir = workspace();
        let md = find_files(
            &serde_json::json!({"path": ".", "pattern": "*.md"}),
            dir.path(),
        );
        assert_eq!(
            md.model_content.lines().collect::<Vec<_>>(),
            vec!["README.md", "docs/tools.md"]
        );

        let docs = find_files(
            &serde_json::json!({"path": ".", "pattern": "docs/*"}),
            dir.path(),
        );
        assert_eq!(docs.model_content, "docs/tools.md");

        let recursive_docs = find_files(
            &serde_json::json!({"path": ".", "pattern": "docs/**/*.md"}),
            dir.path(),
        );
        assert_eq!(recursive_docs.model_content, "docs/tools.md");

        let outside_path = if cfg!(target_os = "windows") {
            "C:\\Windows"
        } else {
            "/etc"
        };
        let blocked = find_files(&serde_json::json!({"path": outside_path}), dir.path());
        assert_eq!(blocked.status, "blocked");
    }

    #[test]
    fn find_files_respects_max_depth() {
        let dir = workspace();
        std::fs::create_dir_all(dir.path().join("a/b/c")).unwrap();
        std::fs::write(dir.path().join("a/b/c/deep.txt"), "deep").unwrap();

        let shallow = find_files(
            &serde_json::json!({"path": ".", "max_depth": 1}),
            dir.path(),
        );
        let lines: Vec<_> = shallow.model_content.lines().collect();
        assert!(lines.contains(&"a/"), "a/ at depth 0: {lines:?}");
        assert!(lines.contains(&"a/b/"), "a/b/ at depth 1: {lines:?}");
        assert!(
            !lines.iter().any(|l| l.starts_with("a/b/c")),
            "a/b/c at depth 2 should be excluded: {lines:?}"
        );
    }

    #[test]
    fn find_files_excludes_build_dirs() {
        let dir = workspace();
        std::fs::create_dir_all(dir.path().join("target/debug")).unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules/pkg")).unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("target/debug/build.txt"), "build").unwrap();
        std::fs::write(dir.path().join("node_modules/pkg/index.js"), "js").unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();

        let result = find_files(&serde_json::json!({"path": "."}), dir.path());
        assert!(!result.model_content.contains("target"));
        assert!(!result.model_content.contains("node_modules"));
        assert!(result.model_content.contains("src/main.rs"));
    }

    #[test]
    fn find_files_penetrates_excluded_dirs_when_explicit_path() {
        let dir = workspace();
        std::fs::create_dir_all(dir.path().join("target/debug")).unwrap();
        std::fs::write(dir.path().join("target/debug/build.txt"), "build").unwrap();

        let result = find_files(&serde_json::json!({"path": "target"}), dir.path());
        assert!(result.model_content.contains("debug/"));
        assert!(result.model_content.contains("build.txt"));
    }
}
