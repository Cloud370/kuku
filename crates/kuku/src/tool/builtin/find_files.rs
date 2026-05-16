use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::tool::ToolResultEnvelope;

use super::common::{glob_match, join_bounded_strings};

const FIND_FILES_MAX_CHARS: usize = 20_000;

pub(crate) fn find_files(args: &Value, workspace: &Path) -> ToolResultEnvelope {
    let path = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let include = args.get("include").and_then(Value::as_str);
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
    if let Err(error) = collect_files(&root, &workspace, include, &mut files) {
        return ToolResultEnvelope::error(
            format!("failed: {error}"),
            format!("error reading directory: {path}"),
        );
    }
    files.sort();

    let file_count = files.len();
    let (mut model_content, truncated) = join_bounded_strings(
        &files,
        FIND_FILES_MAX_CHARS,
        "(Results are truncated. Use a narrower path or include pattern.)",
    );
    if files.is_empty() {
        model_content.clear();
    }

    let summary = if truncated {
        format!("found {file_count} files under {path}, results truncated")
    } else {
        format!("found {file_count} files under {path}")
    };
    let structured = serde_json::json!({
        "kind": "file_list",
        "path": path,
        "include": include,
        "file_count": file_count,
    });

    if truncated {
        ToolResultEnvelope::ok_truncated(summary, model_content, structured)
    } else {
        ToolResultEnvelope::ok(summary, model_content, structured)
    }
}

fn collect_files(
    root: &Path,
    workspace: &Path,
    include: Option<&str>,
    files: &mut Vec<String>,
) -> std::io::Result<()> {
    if root.is_file() {
        push_file(root, workspace, include, files);
        return Ok(());
    }

    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        let path = entry.path();

        if file_type.is_dir() {
            if file_name == ".git" {
                continue;
            }
            collect_files(&path, workspace, include, files)?;
        } else if file_type.is_file() {
            push_file(&path, workspace, include, files);
        }
    }

    Ok(())
}

fn push_file(path: &Path, workspace: &Path, include: Option<&str>, files: &mut Vec<String>) {
    let Ok(relative) = path.strip_prefix(workspace) else {
        return;
    };
    let relative = relative.to_string_lossy().replace('\\', "/");

    if include.is_some_and(|pattern| !glob_match(pattern, &relative)) {
        return;
    }

    files.push(relative);
}

#[cfg(test)]
mod tests {
    use super::super::test_helpers::workspace;
    use super::*;

    #[test]
    fn find_files_returns_sorted_workspace_relative_paths_without_git() {
        let dir = workspace();
        let result = find_files(&serde_json::json!({"path": "."}), dir.path());

        assert_eq!(result.status, "ok");
        assert_eq!(
            result.model_content.lines().collect::<Vec<_>>(),
            vec!["README.md", "docs/tools.md", "src/main.rs"]
        );
        assert!(!result.model_content.contains(".git"));
        assert_eq!(result.structured.unwrap()["file_count"], 3);
    }

    #[test]
    fn find_files_filters_include_globs_and_blocks_outside_workspace() {
        let dir = workspace();
        let md = find_files(
            &serde_json::json!({"path": ".", "include": "*.md"}),
            dir.path(),
        );
        assert_eq!(
            md.model_content.lines().collect::<Vec<_>>(),
            vec!["README.md", "docs/tools.md"]
        );

        let docs = find_files(
            &serde_json::json!({"path": ".", "include": "docs/*"}),
            dir.path(),
        );
        assert_eq!(docs.model_content, "docs/tools.md");

        let recursive_docs = find_files(
            &serde_json::json!({"path": ".", "include": "docs/**/*.md"}),
            dir.path(),
        );
        assert_eq!(recursive_docs.model_content, "docs/tools.md");

        let blocked = find_files(&serde_json::json!({"path": "/etc"}), dir.path());
        assert_eq!(blocked.status, "blocked");
    }
}
