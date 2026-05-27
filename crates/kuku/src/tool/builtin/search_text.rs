use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::Value;

use crate::tool::ToolResultEnvelope;

use super::common::{
    glob_match, is_blocked_relative_path, is_default_excluded_dir, join_bounded_strings,
    relative_path, resolve_path,
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
    let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
    let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(100) as usize;
    let context = args.get("context").and_then(Value::as_u64).unwrap_or(0) as usize;
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
    if view == "lines" {
        files.sort_by(|left, right| {
            let left_mtime = fs::metadata(&left.absolute).and_then(|m| m.modified()).ok();
            let right_mtime = fs::metadata(&right.absolute)
                .and_then(|m| m.modified())
                .ok();
            right_mtime.cmp(&left_mtime)
        });
    } else {
        files.sort_by(|left, right| left.relative.cmp(&right.relative));
    }

    let mut matches = Vec::new();
    let mut file_lines: HashMap<String, Vec<String>> = HashMap::new();
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
        if context > 0 && view == "lines" {
            let lines: Vec<String> = content.lines().map(String::from).collect();
            for (index, line) in lines.iter().enumerate() {
                if regex.is_match(line) {
                    matches.push(SearchMatch {
                        path: file.relative.clone(),
                        line_number: index + 1,
                        line: trim_search_line(line),
                    });
                }
            }
            file_lines.insert(file.relative.clone(), lines);
        } else {
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
    }

    let total_match_count = matches.len();
    let sliced: Vec<_> = matches.into_iter().skip(offset).take(limit).collect();
    let has_more = offset + sliced.len() < total_match_count;

    let model_lines = if view == "lines" && context > 0 {
        render_lines_with_context(&sliced, &file_lines, context)
    } else {
        render_search_lines(view, &sliced)
    };
    let truncation_message = if has_more {
        format!(
            "Showing {} of {} matches. Use offset={} to see more.",
            sliced.len(),
            total_match_count,
            offset + limit,
        )
    } else {
        "(Results are truncated. Use a narrower path/include pattern or view=files/count.)"
            .to_string()
    };
    let (model_content, truncated) =
        join_bounded_strings(&model_lines, SEARCH_TEXT_MAX_CHARS, &truncation_message);
    let file_count = unique_match_file_count(&sliced);
    let summary = if has_more {
        format!(
            "Showing {} of {} matches in {} files, view={}",
            sliced.len(),
            total_match_count,
            file_count,
            view
        )
    } else if truncated {
        format!(
            "{} matches in {} files, view={}, results truncated",
            sliced.len(),
            file_count,
            view
        )
    } else {
        format!(
            "{} matches in {} files, view={}",
            sliced.len(),
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
        "match_count": sliced.len(),
        "total_match_count": total_match_count,
        "file_count": file_count,
        "searched_file_count": searched_file_count,
        "skipped_file_count": skipped_file_count,
        "blocked_file_count": blocked_file_count,
        "offset": offset,
        "limit": limit,
        "has_more": has_more,
    });

    if truncated || has_more {
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
            let file_name = entry.file_name();
            if is_default_excluded_dir(&file_name.to_string_lossy()) {
                continue;
            }
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

fn render_lines_with_context(
    matches: &[SearchMatch],
    file_lines: &HashMap<String, Vec<String>>,
    context: usize,
) -> Vec<String> {
    let mut output = Vec::new();
    let mut prev_end: Option<(String, usize)> = None;

    for m in matches {
        let lines = match file_lines.get(&m.path) {
            Some(l) => l,
            None => {
                output.push(format!("{}:{}: {}", m.path, m.line_number, m.line));
                continue;
            }
        };

        let start = m.line_number.saturating_sub(context).max(1);
        let end = (m.line_number + context).min(lines.len());

        if let Some((ref prev_path, prev_end_line)) = prev_end {
            if prev_path != &m.path || start > prev_end_line + 1 {
                output.push("--".to_string());
            }
        }

        for line_num in start..=end {
            let idx = line_num - 1;
            let content = lines.get(idx).map(|s| s.as_str()).unwrap_or("");
            if line_num == m.line_number {
                output.push(format!("{}:{}: {}", m.path, line_num, content));
            } else {
                output.push(format!("{}:{}- {}", m.path, line_num, content));
            }
        }

        prev_end = Some((m.path.clone(), end));
    }

    output
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

    #[test]
    fn search_text_excludes_build_dirs() {
        let dir = workspace();
        std::fs::create_dir_all(dir.path().join("target/debug")).unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("target/debug/junk.rs"), "TODO in build").unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "TODO in src").unwrap();

        let result = search_text(&serde_json::json!({"pattern": "TODO"}), dir.path());
        assert!(result.model_content.contains("src/main.rs"));
        assert!(!result.model_content.contains("target"));
    }

    #[test]
    fn search_text_pagination_offset_and_limit() {
        let dir = workspace();
        std::fs::write(
            dir.path().join("a.txt"),
            "line1\nTODO a\nline3\nTODO b\nline5\nTODO c\n",
        )
        .unwrap();

        let page1 = search_text(
            &serde_json::json!({"pattern": "TODO", "view": "lines", "limit": 2}),
            dir.path(),
        );
        assert_eq!(page1.status, "ok");
        assert_eq!(page1.structured.as_ref().unwrap()["match_count"], 2);
        assert_eq!(page1.structured.as_ref().unwrap()["total_match_count"], 3);
        assert_eq!(page1.structured.as_ref().unwrap()["has_more"], true);

        let page2 = search_text(
            &serde_json::json!({"pattern": "TODO", "view": "lines", "offset": 2, "limit": 2}),
            dir.path(),
        );
        assert_eq!(page2.status, "ok");
        assert_eq!(page2.structured.as_ref().unwrap()["match_count"], 1);
        assert_eq!(page2.structured.as_ref().unwrap()["has_more"], false);
    }

    #[test]
    fn search_text_context_lines() {
        let dir = workspace();
        std::fs::write(
            dir.path().join("main.rs"),
            "use std::env;\n\nfn main() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();

        let result = search_text(
            &serde_json::json!({"pattern": "fn main", "view": "lines", "context": 1}),
            dir.path(),
        );
        assert_eq!(result.status, "ok");
        let lines: Vec<&str> = result.model_content.lines().collect();
        assert!(lines.iter().any(|l| l.contains("fn main")));
        assert!(lines.iter().any(|l| l.ends_with("- ")));
    }

    #[test]
    fn search_text_context_ignored_for_non_lines_view() {
        let dir = workspace();
        std::fs::write(dir.path().join("a.txt"), "TODO match\n").unwrap();

        let result = search_text(
            &serde_json::json!({"pattern": "TODO", "view": "files", "context": 5}),
            dir.path(),
        );
        assert_eq!(result.status, "ok");
        assert_eq!(result.model_content, "a.txt");
    }

    #[test]
    fn search_text_files_view_preserves_alphabetical_order() {
        let dir = workspace();
        std::fs::write(dir.path().join("z.txt"), "TODO z\n").unwrap();
        std::fs::write(dir.path().join("a.txt"), "TODO a\n").unwrap();
        std::fs::write(dir.path().join("m.txt"), "TODO m\n").unwrap();

        let result = search_text(
            &serde_json::json!({"pattern": "TODO", "view": "files"}),
            dir.path(),
        );
        assert_eq!(result.status, "ok");
        let files: Vec<&str> = result.model_content.lines().collect();
        assert_eq!(files, vec!["a.txt", "m.txt", "z.txt"]);
    }
}
