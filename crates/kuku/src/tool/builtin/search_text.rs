use std::cmp::Reverse;
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::Serialize;
use serde_json::Value;

use crate::tool::ToolResultEnvelope;

use super::common::{
    ensure_active, file_read_error, glob_match, is_default_excluded_dir, join_bounded_strings,
    read_file_as_utf8, relative_path, resolve_path, verify_directory, workspace_path_blocked,
    FileCancellation,
};

const SEARCH_TEXT_MAX_CHARS: usize = 80_000;
const MAX_SEARCH_LINE_CHARS: usize = 500;

struct CollectedFile {
    absolute: PathBuf,
    relative: String,
}

#[derive(Serialize)]
struct SearchMatch {
    path: String,
    line_number: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<String>,
    line_truncated: bool,
}

struct SearchPage {
    offset: usize,
    limit: usize,
    total: usize,
    bytes: usize,
    matches: Vec<SearchMatch>,
}

impl SearchPage {
    fn new(offset: usize, limit: usize) -> Self {
        Self {
            offset,
            limit,
            total: 0,
            bytes: 0,
            matches: Vec::new(),
        }
    }

    fn scan(
        &mut self,
        path: &str,
        content: &str,
        regex: &Regex,
        include_line: bool,
        cancelled: &FileCancellation<'_>,
    ) -> io::Result<()> {
        let mut full = self.bytes >= SEARCH_TEXT_MAX_CHARS;
        for (index, source) in content.lines().enumerate() {
            ensure_active(cancelled)?;
            if !regex.is_match(source) {
                continue;
            }
            self.total += 1;
            if full || self.total <= self.offset || self.matches.len() >= self.limit {
                continue;
            }
            let line = include_line.then(|| trim_search_line(source));
            let row_bytes = path.len() + line.as_ref().map_or(0, String::len) + 64;
            if row_bytes > SEARCH_TEXT_MAX_CHARS - self.bytes {
                self.bytes = SEARCH_TEXT_MAX_CHARS;
                full = true;
                continue;
            }
            let line_truncated = line.as_ref().is_some_and(|line| line.as_str() != source);
            self.matches.push(SearchMatch {
                path: path.to_owned(),
                line_number: index + 1,
                line,
                line_truncated,
            });
            self.bytes += row_bytes;
        }
        Ok(())
    }
}

pub(crate) fn search_text(
    args: &Value,
    workspace: &Path,
    cancelled: &FileCancellation<'_>,
) -> ToolResultEnvelope {
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
        cancelled,
    ) {
        return file_read_error(Path::new(path), error);
    }
    if view == "lines" {
        files.sort_by_cached_key(|file| {
            let modified = if cancelled() {
                None
            } else {
                fs::metadata(&file.absolute).and_then(|m| m.modified()).ok()
            };
            (Reverse(modified), file.relative.clone())
        });
    } else {
        files.sort_by(|left, right| left.relative.cmp(&right.relative));
    }

    let mut page = SearchPage::new(offset, limit);
    let mut preview = ContextPreview::default();
    let mut skipped_file_count = 0_usize;
    let mut searched_file_count = 0_usize;
    for file in &files {
        let content = match read_file_as_utf8(&file.absolute, cancelled) {
            Ok(content) => content,
            Err(result) if result.status == "cancelled" => return result,
            Err(_) => {
                skipped_file_count += 1;
                continue;
            }
        };
        searched_file_count += 1;
        let previous_count = page.matches.len();
        if let Err(error) = page.scan(&file.relative, &content, &regex, view == "lines", cancelled)
        {
            return file_read_error(Path::new(path), error);
        }
        if context > 0 && view == "lines" {
            if let Err(error) = preview.extend(
                &file.relative,
                &content,
                &page.matches[previous_count..],
                context,
                cancelled,
            ) {
                return file_read_error(Path::new(path), error);
            }
        }
    }

    let total_match_count = page.total;
    let sliced = page.matches;
    let next_offset = offset.saturating_add(sliced.len());
    let has_more = next_offset < total_match_count;

    let model_lines = if view == "lines" && context > 0 {
        preview.rows
    } else {
        render_search_lines(view, &sliced)
    };
    let truncation_message = if has_more {
        format!(
            "Showing {} of {} matches. Use offset={} to see more.",
            sliced.len(),
            total_match_count,
            next_offset,
        )
    } else {
        "(Results are truncated. Use a narrower path/include pattern or view=files/count.)"
            .to_string()
    };
    let (mut model_content, truncated) =
        join_bounded_strings(&model_lines, SEARCH_TEXT_MAX_CHARS, &truncation_message);
    if !truncated && (preview.truncated || has_more) {
        if !model_content.is_empty() {
            model_content.push('\n');
        }
        model_content.push_str(&truncation_message);
    }
    let truncated = truncated || preview.truncated;
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
        "next_offset": has_more.then_some(next_offset),
        "matches": sliced,
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
    cancelled: &FileCancellation<'_>,
) -> io::Result<()> {
    ensure_active(cancelled)?;
    let relative = relative_path(root, workspace);
    if workspace_path_blocked(&relative) {
        *blocked_file_count += 1;
        return Ok(());
    }
    if root.is_file() {
        push_search_file(root, workspace, include, files, blocked_file_count);
        return Ok(());
    }

    verify_directory(root)?;
    for entry in fs::read_dir(root)? {
        ensure_active(cancelled)?;
        let entry = entry?;
        let file_type = entry.file_type()?;
        let path = entry.path();
        let relative = relative_path(&path, workspace);

        if workspace_path_blocked(&relative) {
            *blocked_file_count += 1;
            continue;
        }
        if file_type.is_dir() {
            let file_name = entry.file_name();
            if is_default_excluded_dir(&file_name.to_string_lossy()) {
                continue;
            }
            collect_search_files(
                &path,
                workspace,
                include,
                files,
                blocked_file_count,
                cancelled,
            )?;
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
    if workspace_path_blocked(&relative) {
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
            .map(|match_| {
                format!(
                    "{}:{}: {}",
                    match_.path,
                    match_.line_number,
                    match_.line.as_deref().unwrap_or_default()
                )
            })
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

#[derive(Default)]
struct ContextPreview {
    rows: Vec<String>,
    bytes: usize,
    truncated: bool,
}

impl ContextPreview {
    fn extend(
        &mut self,
        path: &str,
        content: &str,
        matches: &[SearchMatch],
        context: usize,
        cancelled: &FileCancellation<'_>,
    ) -> io::Result<()> {
        if matches.is_empty() || self.truncated {
            return Ok(());
        }
        let start = matches[0].line_number.saturating_sub(context).max(1);
        let end = matches[matches.len() - 1]
            .line_number
            .saturating_add(context);
        let mut upcoming = matches.iter().map(|hit| hit.line_number).peekable();
        let mut preceding = None;
        let mut last_rendered = 0;
        for (index, line) in content.lines().enumerate().take(end) {
            ensure_active(cancelled)?;
            let number = index + 1;
            if number < start {
                continue;
            }
            let matching = upcoming.peek() == Some(&number);
            if matching {
                preceding = upcoming.next();
            }
            let close = upcoming.peek().is_some_and(|next| next - number <= context)
                || preceding.is_some_and(|prior| number - prior <= context);
            if !close {
                continue;
            }
            if !self.rows.is_empty() && (last_rendered == 0 || last_rendered + 1 < number) {
                self.push("--".to_owned());
            }
            let marker = if matching { ':' } else { '-' };
            self.push(format!(
                "{path}:{number}{marker} {}",
                trim_search_line(line)
            ));
            if self.truncated {
                break;
            }
            last_rendered = number;
        }
        Ok(())
    }

    fn push(&mut self, row: String) {
        let next = self.bytes + row.len() + usize::from(!self.rows.is_empty());
        if next > SEARCH_TEXT_MAX_CHARS {
            self.truncated = true;
        } else if !self.truncated {
            self.rows.push(row);
            self.bytes = next;
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
    matches
        .iter()
        .map(|hit| hit.path.as_str())
        .collect::<HashSet<_>>()
        .len()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::super::test_helpers::workspace;
    use super::*;

    fn search_text(args: &Value, workspace: &Path) -> ToolResultEnvelope {
        super::search_text(args, workspace, &|| false)
    }

    #[test]
    fn search_stops_during_input_scan_without_returning_a_complete_page() {
        let dir = workspace();
        let path = dir.path().join("large.txt");
        std::fs::write(path, "a long fixture line with no matches\n".repeat(10_000)).unwrap();
        let checks = AtomicUsize::new(0);
        let stop = || checks.fetch_add(1, Ordering::Relaxed) >= 4;
        let result = super::search_text(
            &serde_json::json!({"path":"large.txt","pattern":"missing pattern"}),
            dir.path(),
            &stop,
        );

        assert_eq!("cancelled", result.status);
        assert_eq!(5, checks.load(Ordering::Relaxed));
    }

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
        let structured = lines.structured.unwrap();
        let first = &structured["matches"][0];
        assert_eq!("docs/tools.md", first["path"]);
        assert_eq!(2, first["line_number"]);
        assert_eq!("TODO docs", first["line"]);

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
