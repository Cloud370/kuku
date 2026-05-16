use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::Value;

use crate::tool::ToolResultEnvelope;

use super::common::{
    glob_match, is_blocked_relative_path, join_bounded_strings, relative_path, resolve_path,
};

const SEARCH_TEXT_MAX_CHARS: usize = 80_000;
const MAX_SEARCH_LINE_CHARS: usize = 500;

struct CollectedFile {
    absolute: PathBuf,
    relative: String,
}

struct SearchMatch {
    path: String,
    line_number: usize,
    line: String,
}

pub(crate) fn search_text(args: &Value, workspace: &Path) -> ToolResultEnvelope {
    let Some(pattern) = args.get("pattern").and_then(Value::as_str) else {
        return ToolResultEnvelope::error(
            "failed: missing pattern",
            "search_text requires pattern",
        );
    };
    let path = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let include = args.get("include").and_then(Value::as_str);
    let view = args.get("view").and_then(Value::as_str).unwrap_or("files");
    if !matches!(view, "files" | "lines" | "count") {
        return ToolResultEnvelope::error(
            format!("failed: invalid view: {view}"),
            "view must be one of: files, lines, count",
        );
    }
    let regex = match Regex::new(pattern) {
        Ok(regex) => regex,
        Err(error) => {
            return ToolResultEnvelope::error(
                "failed: invalid regex",
                format!("invalid regex: {error}"),
            )
        }
    };
    let resolved = match resolve_path(workspace, path) {
        Ok(resolved) => resolved,
        Err(result) => return result,
    };

    let mut files = Vec::new();
    let mut blocked_file_count = 0_usize;
    if let Err(error) = collect_search_files(
        &resolved.path,
        &resolved.workspace,
        include,
        &mut files,
        &mut blocked_file_count,
    ) {
        return ToolResultEnvelope::error(
            format!("failed: {error}"),
            format!("error reading search path: {path}"),
        );
    }
    files.sort_by(|left, right| left.relative.cmp(&right.relative));

    let mut matches = Vec::new();
    let mut skipped_file_count = 0_usize;
    let mut searched_file_count = 0_usize;
    for file in &files {
        let bytes = match fs::read(&file.absolute) {
            Ok(bytes) => bytes,
            Err(_) => {
                skipped_file_count += 1;
                continue;
            }
        };
        let Ok(content) = String::from_utf8(bytes) else {
            skipped_file_count += 1;
            continue;
        };
        searched_file_count += 1;
        for (index, line) in content.lines().enumerate() {
            if regex.is_match(line) {
                matches.push(SearchMatch {
                    path: file.relative.clone(),
                    line_number: index + 1,
                    line: trim_search_line(line),
                });
            }
        }
    }

    let model_lines = render_search_lines(view, &matches);
    let (model_content, truncated) = join_bounded_strings(
        &model_lines,
        SEARCH_TEXT_MAX_CHARS,
        "(Results are truncated. Use a narrower path/include pattern or view=files/count.)",
    );
    let file_count = unique_match_file_count(&matches);
    let summary = if truncated {
        format!(
            "{} matches in {} files, view={}, results truncated",
            matches.len(),
            file_count,
            view
        )
    } else {
        format!(
            "{} matches in {} files, view={}",
            matches.len(),
            file_count,
            view
        )
    };
    let structured = serde_json::json!({
        "kind": "search_results",
        "pattern": pattern,
        "path": path,
        "include": include,
        "view": view,
        "match_count": matches.len(),
        "file_count": file_count,
        "searched_file_count": searched_file_count,
        "skipped_file_count": skipped_file_count,
        "blocked_file_count": blocked_file_count,
    });

    if truncated {
        ToolResultEnvelope::ok_truncated(summary, model_content, structured)
    } else {
        ToolResultEnvelope::ok(summary, model_content, structured)
    }
}

fn collect_search_files(
    root: &Path,
    workspace: &Path,
    include: Option<&str>,
    files: &mut Vec<CollectedFile>,
    blocked_file_count: &mut usize,
) -> std::io::Result<()> {
    let relative = relative_path(root, workspace);
    if is_blocked_relative_path(&relative) {
        *blocked_file_count += 1;
        return Ok(());
    }
    if root.is_file() {
        push_search_file(root, workspace, include, files, blocked_file_count);
        return Ok(());
    }

    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let path = entry.path();
        let relative = relative_path(&path, workspace);

        if is_blocked_relative_path(&relative) {
            *blocked_file_count += 1;
            continue;
        }
        if file_type.is_dir() {
            collect_search_files(&path, workspace, include, files, blocked_file_count)?;
        } else if file_type.is_file() {
            push_search_file(&path, workspace, include, files, blocked_file_count);
        }
    }

    Ok(())
}

fn push_search_file(
    path: &Path,
    workspace: &Path,
    include: Option<&str>,
    files: &mut Vec<CollectedFile>,
    blocked_file_count: &mut usize,
) {
    let relative = relative_path(path, workspace);
    if is_blocked_relative_path(&relative) {
        *blocked_file_count += 1;
        return;
    }
    if include.is_some_and(|pattern| !glob_match(pattern, &relative)) {
        return;
    }
    files.push(CollectedFile {
        absolute: path.to_path_buf(),
        relative,
    });
}

fn render_search_lines(view: &str, matches: &[SearchMatch]) -> Vec<String> {
    match view {
        "lines" => matches
            .iter()
            .map(|match_| format!("{}:{}: {}", match_.path, match_.line_number, match_.line))
            .collect(),
        "count" => {
            let mut counts = Vec::<(String, usize)>::new();
            for match_ in matches {
                if let Some((_, count)) = counts.iter_mut().find(|(path, _)| path == &match_.path) {
                    *count += 1;
                } else {
                    counts.push((match_.path.clone(), 1));
                }
            }
            counts
                .into_iter()
                .map(|(path, count)| format!("{path}: {count}"))
                .collect()
        }
        _ => {
            let mut paths = Vec::<String>::new();
            for match_ in matches {
                if !paths.contains(&match_.path) {
                    paths.push(match_.path.clone());
                }
            }
            paths
        }
    }
}

fn trim_search_line(line: &str) -> String {
    let mut trimmed = String::new();
    for ch in line.chars().take(MAX_SEARCH_LINE_CHARS) {
        trimmed.push(ch);
    }
    if trimmed.len() < line.len() {
        trimmed.push_str("...");
    }
    trimmed
}

fn unique_match_file_count(matches: &[SearchMatch]) -> usize {
    let mut paths = Vec::<&str>::new();
    for match_ in matches {
        if !paths.contains(&match_.path.as_str()) {
            paths.push(&match_.path);
        }
    }
    paths.len()
}

#[cfg(test)]
mod tests {
    use super::super::test_helpers::workspace;
    use super::*;

    #[test]
    fn search_text_supports_files_lines_and_count_views() {
        let dir = workspace();
        std::fs::write(dir.path().join("README.md"), "TODO root\nDONE\n").unwrap();
        std::fs::write(
            dir.path().join("docs/tools.md"),
            "alpha\nTODO docs\nTODO again\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();

        let files = search_text(&serde_json::json!({"pattern": "TODO"}), dir.path());
        assert_eq!(files.status, "ok");
        assert_eq!(files.model_content, "README.md\ndocs/tools.md");
        assert_eq!(files.structured.as_ref().unwrap()["match_count"], 3);
        assert_eq!(files.structured.as_ref().unwrap()["file_count"], 2);

        let lines = search_text(
            &serde_json::json!({"pattern": "TODO", "path": "docs", "view": "lines"}),
            dir.path(),
        );
        assert_eq!(lines.status, "ok");
        assert_eq!(
            lines.model_content.lines().collect::<Vec<_>>(),
            vec!["docs/tools.md:2: TODO docs", "docs/tools.md:3: TODO again"]
        );

        let count = search_text(
            &serde_json::json!({"pattern": "TODO", "view": "count"}),
            dir.path(),
        );
        assert_eq!(count.status, "ok");
        assert_eq!(
            count.model_content.lines().collect::<Vec<_>>(),
            vec!["README.md: 1", "docs/tools.md: 2"]
        );
    }

    #[test]
    fn search_text_filters_by_include_glob() {
        let dir = workspace();
        std::fs::write(dir.path().join("README.md"), "TODO root\n").unwrap();
        std::fs::write(dir.path().join("docs/tools.md"), "TODO docs\n").unwrap();

        let filtered = search_text(
            &serde_json::json!({"pattern": "TODO", "include": "docs/**/*.md"}),
            dir.path(),
        );
        assert_eq!(filtered.status, "ok");
        assert_eq!(filtered.model_content, "docs/tools.md");
    }

    #[test]
    fn search_text_blocks_sensitive_paths_and_rejects_invalid_regex() {
        let dir = workspace();
        std::fs::write(dir.path().join("README.md"), "TODO root\n").unwrap();
        std::fs::write(dir.path().join(".env"), "TODO secret\n").unwrap();
        std::fs::write(dir.path().join("secret.pem"), "TODO secret\n").unwrap();

        let blocked = search_text(
            &serde_json::json!({"pattern": "TODO", "path": ".env"}),
            dir.path(),
        );
        assert_eq!(blocked.status, "blocked");

        let unfiltered = search_text(&serde_json::json!({"pattern": "TODO"}), dir.path());
        assert_eq!(
            unfiltered.structured.as_ref().unwrap()["blocked_file_count"],
            3
        );
        assert!(!unfiltered.model_content.contains("secret"));

        let invalid_regex = search_text(&serde_json::json!({"pattern": "["}), dir.path());
        assert_eq!(invalid_regex.status, "error");
        assert!(invalid_regex.model_content.contains("invalid regex"));
    }
}
