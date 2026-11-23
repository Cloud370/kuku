use std::cmp::Reverse;
use std::fs;
use std::io;
use std::path::Path;
use std::time::SystemTime;

use serde::Serialize;
use serde_json::Value;

use crate::tool::ToolResultEnvelope;
use crate::WorkspaceIoLimits;

use super::common::{
    charge_entry, ensure_active, file_read_error, glob_match, is_cancellation,
    is_default_excluded_dir, is_io_limit, optional_positive_usize, relative_path, resolve_path,
    verify_directory, workspace_path_blocked, FileCancellation, IoLimit, ScanDeadline,
    WorkspaceFiles,
};

mod preview;
mod scan;
mod workspace_scan;

use preview::ContextPreview;
use scan::FileSearch;
use workspace_scan::{ScanStats, WorkspaceScan};

const SEARCH_MATCH_BYTES: usize = 80_000;
const PAGE_NOTICE_BYTES: usize = 320;
const MAX_SEARCH_LINE_CHARS: usize = 500;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SearchView {
    Files,
    Lines,
    Count,
}

struct CollectedFile {
    relative: String,
    original: Option<Box<Path>>,
    modified: Option<SystemTime>,
}

impl CollectedFile {
    fn path(&self) -> &Path {
        self.original
            .as_deref()
            .unwrap_or(Path::new(&self.relative))
    }
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
    view: SearchView,
    body_budget: usize,
    progress: SearchProgress,
    matches: Vec<SearchMatch>,
}

#[derive(Clone, Copy, Default)]
struct SearchProgress {
    total: usize,
    body_bytes: usize,
    match_bytes: usize,
    full: bool,
}

impl SearchPage {
    fn new(offset: usize, limit: usize, view: SearchView, body_budget: usize) -> Self {
        Self {
            offset,
            limit,
            view,
            body_budget,
            progress: SearchProgress::default(),
            matches: Vec::new(),
        }
    }

    fn matched(&mut self, path: &str, line_number: usize, source: &str) -> bool {
        self.progress.total += 1;
        if !self.progress.full
            && self.progress.total > self.offset
            && self.matches.len() < self.limit
        {
            self.add_match(path, line_number, source);
        }
        true
    }

    fn add_match(&mut self, path: &str, line_number: usize, source: &str) {
        let prefix_bytes = self.row_prefix_bytes(path, line_number);
        let line = (self.view == SearchView::Lines)
            .then(|| trim_search_line(source, self.body_budget.saturating_sub(prefix_bytes)));
        let line_truncated = line.as_deref().is_some_and(|line| line != source);
        self.push_match(
            SearchMatch {
                path: path.to_owned(),
                line_number,
                line,
                line_truncated,
            },
            prefix_bytes,
        );
    }

    fn row_prefix_bytes(&self, path: &str, line_number: usize) -> usize {
        let new_file = self.matches.last().is_none_or(|hit| hit.path != path);
        match self.view {
            SearchView::Lines => line_row_bytes(path, line_number),
            SearchView::Files if new_file => path.len() + 1,
            SearchView::Count if new_file => path.len() + usize::MAX.ilog10() as usize + 4,
            SearchView::Files | SearchView::Count => 0,
        }
    }

    fn push_match(&mut self, hit: SearchMatch, prefix_bytes: usize) {
        let line_bytes = hit.line.as_ref().map_or(0, String::len);
        let body_bytes = prefix_bytes + line_bytes;
        let match_bytes = hit.path.len() + line_bytes + 64;
        if body_bytes > self.body_budget - self.progress.body_bytes
            || match_bytes > SEARCH_MATCH_BYTES - self.progress.match_bytes
        {
            self.progress.full = true;
            return;
        }
        self.matches.push(hit);
        self.progress.body_bytes += body_bytes;
        self.progress.match_bytes += match_bytes;
    }

    fn append_page(&mut self, page: Self) {
        self.progress.total += page.progress.total;
        for hit in page.matches {
            if self.progress.full || self.matches.len() == self.limit {
                break;
            }
            let prefix = self.row_prefix_bytes(&hit.path, hit.line_number);
            self.push_match(hit, prefix);
        }
        self.progress.full |= page.progress.full;
    }
}

pub(crate) fn search_text_for_executor(
    args: &Value,
    workspace: &Path,
    limits: WorkspaceIoLimits,
    cancelled: &FileCancellation<'_>,
) -> ToolResultEnvelope {
    search_text(args, workspace, SEARCH_MATCH_BYTES, limits, cancelled)
}

pub(crate) fn search_text(
    args: &Value,
    workspace: &Path,
    preview_bytes: usize,
    limits: WorkspaceIoLimits,
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
    let limit = match optional_positive_usize(args, "limit") {
        Ok(limit) => limit.unwrap_or(100),
        Err(result) => return result,
    };
    let context = args.get("context").and_then(Value::as_u64).unwrap_or(0) as usize;
    let view_kind = match view {
        "files" => SearchView::Files,
        "lines" => SearchView::Lines,
        "count" => SearchView::Count,
        _ => {
            return ToolResultEnvelope::error(
                format!("failed: invalid view: {view}"),
                "view must be one of: files, lines, count",
            );
        }
    };
    let deadline = ScanDeadline::new(limits.scan_timeout_ms);
    let search = match FileSearch::new(pattern, limits.search_line_bytes, deadline) {
        Ok(search) => search,
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

    let mut candidates = SearchFiles {
        workspace: &resolved.workspace,
        include,
        view: view_kind,
        files: Vec::new(),
        blocked: 0,
        unreadable_entries: 0,
        entries_left: limits.scan_entries,
        path_bytes_left: limits.search_path_bytes,
        deadline,
        cancelled,
    };
    let collection_limit = match candidates.collect(&resolved.path) {
        Ok(()) => None,
        Err(error) if is_io_limit(&error) => Some(error.to_string()),
        Err(error) => return file_read_error(Path::new(path), error),
    };
    let SearchFiles {
        mut files,
        blocked: blocked_file_count,
        unreadable_entries,
        ..
    } = candidates;
    if view_kind == SearchView::Lines {
        files.sort_unstable_by(|left, right| {
            (Reverse(left.modified), &left.relative)
                .cmp(&(Reverse(right.modified), &right.relative))
        });
    } else {
        files.sort_unstable_by(|left, right| left.relative.cmp(&right.relative));
    }

    let body_budget = preview_bytes - PAGE_NOTICE_BYTES;
    let mut page = SearchPage::new(offset, limit, view_kind, body_budget);
    let mut preview = ContextPreview::new(body_budget);
    let source = match WorkspaceFiles::new(&resolved.workspace) {
        Ok(source) => source,
        Err(error) => return file_read_error(Path::new(path), error),
    };
    let scan = WorkspaceScan {
        source: &source,
        deadline,
        cancelled,
    };
    let ScanStats {
        searched: searched_file_count,
        skipped: skipped_file_count,
        limit: scan_limit,
    } = match scan.run(search, &files, &mut page, &mut preview, context, limits) {
        Ok(stats) => stats,
        Err(error) => return file_read_error(Path::new(path), error),
    };
    let budget_reason = collection_limit.or(scan_limit);

    if let Err(error) = source.verify_root() {
        return file_read_error(Path::new(path), error);
    }
    let total_match_count = page.progress.total;
    let sliced = page.matches;
    let next_offset = offset.saturating_add(sliced.len());
    let scan_complete = budget_reason.is_none() && unreadable_entries == 0;
    let page_has_more = next_offset < total_match_count;
    let has_more = scan_complete && page_has_more;
    if page_has_more && sliced.is_empty() {
        return ToolResultEnvelope::error(
            "search match does not fit within the preview limit",
            "The next search match exceeds the preview limit. Try the search in run_js to access a larger local result.",
        );
    }

    let primary_lines = render_search_lines(view_kind, &sliced);
    let (mut model_content, context_truncated) =
        preview.render(primary_lines, body_budget - page.progress.body_bytes);
    if has_more {
        use std::fmt::Write;
        let _ = write!(
            model_content,
            "\nShowing {} of {total_match_count} matches. Use offset={next_offset} to continue.",
            sliced.len()
        );
    }
    if context_truncated {
        model_content.push_str(
            "\nSome context lines were omitted. Lower limit or use read_file around a hit.",
        );
    }
    if unreadable_entries > 0 {
        model_content.push_str("\nSome directory entries could not be read.");
    }
    if let Some(reason) = &budget_reason {
        use std::fmt::Write;
        let _ = write!(
            model_content,
            "\nSearch budget reached ({reason}). Narrow path or include."
        );
    }
    let file_count = sliced
        .chunk_by(|left, right| left.path == right.path)
        .count();
    let summary = if has_more {
        format!(
            "Showing {} of {} matches in {} files, view={}",
            sliced.len(),
            total_match_count,
            file_count,
            view
        )
    } else if context_truncated {
        format!(
            "{} matches in {} files, view={}, context truncated",
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
        "unreadable_entry_count": unreadable_entries,
        "offset": offset,
        "limit": limit,
        "has_more": has_more,
        "next_offset": has_more.then_some(next_offset),
        "context_truncated": context_truncated,
        "scan_complete": scan_complete,
        "scan_limit": budget_reason,
        "matches": sliced,
    });

    if context_truncated || has_more || !scan_complete {
        ToolResultEnvelope::ok_truncated(summary, model_content, structured)
    } else {
        ToolResultEnvelope::ok(summary, model_content, structured)
    }
}

struct SearchFiles<'a> {
    workspace: &'a Path,
    include: Option<&'a str>,
    view: SearchView,
    files: Vec<CollectedFile>,
    blocked: usize,
    unreadable_entries: usize,
    entries_left: usize,
    path_bytes_left: usize,
    deadline: ScanDeadline,
    cancelled: &'a FileCancellation<'a>,
}

impl SearchFiles<'_> {
    fn collect(&mut self, root: &Path) -> io::Result<()> {
        self.deadline.check()?;
        ensure_active(self.cancelled)?;
        let relative = relative_path(root, self.workspace);
        if workspace_path_blocked(&relative) {
            self.blocked += 1;
            return Ok(());
        }
        if root.is_file() {
            return self.push(root, relative);
        }
        self.walk(root)
    }

    fn walk(&mut self, root: &Path) -> io::Result<()> {
        verify_directory(root)?;
        for entry in fs::read_dir(root)? {
            self.deadline.check()?;
            charge_entry(&mut self.entries_left, self.cancelled)?;
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    self.unreadable_entries += 1;
                    continue;
                }
            };
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => {
                    self.unreadable_entries += 1;
                    continue;
                }
            };
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if workspace_path_blocked(&name) {
                self.blocked += 1;
                continue;
            }
            if file_type.is_dir() && !is_default_excluded_dir(&name) {
                if let Err(error) = self.walk(&entry.path()) {
                    if is_cancellation(&error) || is_io_limit(&error) {
                        return Err(error);
                    }
                    self.unreadable_entries += 1;
                }
            } else if file_type.is_file() {
                let path = entry.path();
                self.push(&path, relative_path(&path, self.workspace))?;
            }
        }
        Ok(())
    }

    fn push(&mut self, path: &Path, relative: String) -> io::Result<()> {
        if self
            .include
            .is_some_and(|pattern| !glob_match(pattern, &relative))
        {
            return Ok(());
        }
        let original = path.strip_prefix(self.workspace).unwrap_or(path);
        let original = (original != Path::new(&relative)).then(|| Box::<Path>::from(original));
        let bytes = std::mem::size_of::<CollectedFile>()
            + relative.capacity()
            + original.as_ref().map_or(0, |raw| raw.as_os_str().len());
        self.path_bytes_left = self
            .path_bytes_left
            .checked_sub(bytes)
            .ok_or_else(|| io::Error::other(IoLimit("search path memory budget exhausted")))?;
        self.files
            .try_reserve(1)
            .map_err(|error| io::Error::new(io::ErrorKind::OutOfMemory, error))?;
        let modified = (self.view == SearchView::Lines)
            .then(|| {
                fs::metadata(path)
                    .and_then(|metadata| metadata.modified())
                    .ok()
            })
            .flatten();
        self.files.push(CollectedFile {
            relative,
            original,
            modified,
        });
        Ok(())
    }
}

fn render_search_lines(view: SearchView, matches: &[SearchMatch]) -> Vec<String> {
    if view == SearchView::Lines {
        return matches
            .iter()
            .map(|hit| {
                format!(
                    "{}:{}: {}",
                    hit.path,
                    hit.line_number,
                    hit.line.as_deref().unwrap_or_default()
                )
            })
            .collect();
    }
    matches
        .chunk_by(|left, right| left.path == right.path)
        .map(|group| {
            let path = &group[0].path;
            if view == SearchView::Count {
                format!("{path}: {}", group.len())
            } else {
                path.clone()
            }
        })
        .collect()
}

fn line_row_bytes(path: &str, line_number: usize) -> usize {
    path.len() + line_number.ilog10() as usize + 5
}

fn trim_search_line(line: &str, max_bytes: usize) -> String {
    let char_end = line
        .char_indices()
        .nth(MAX_SEARCH_LINE_CHARS)
        .map_or(line.len(), |(index, _)| index);
    if char_end == line.len() && line.len() <= max_bytes {
        return line.to_owned();
    }
    let suffix = if max_bytes >= 3 { "..." } else { "" };
    let end = line.floor_char_boundary(char_end.min(max_bytes - suffix.len()));
    format!("{}{suffix}", &line[..end])
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::super::test_helpers::workspace;
    use super::*;

    fn search_text(args: &Value, workspace: &Path) -> ToolResultEnvelope {
        super::search_text(
            args,
            workspace,
            crate::RuntimeLimits::default().max_preview_bytes,
            WorkspaceIoLimits::default(),
            &|| false,
        )
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
            crate::RuntimeLimits::default().max_preview_bytes,
            WorkspaceIoLimits::default(),
            &stop,
        );

        assert_eq!("cancelled", result.status);
        assert_eq!(5, checks.load(Ordering::Relaxed));
    }

    #[test]
    fn search_discards_matches_from_a_file_with_invalid_utf8_after_the_first_chunk() {
        let dir = workspace();
        let mut input = b"needle\n".to_vec();
        input.extend(b"ordinary line\n".repeat(8_000));
        input.push(0xff);
        std::fs::write(dir.path().join("bad.txt"), input).unwrap();
        std::fs::write(dir.path().join("good.txt"), "needle\n").unwrap();

        let result = search_text(&serde_json::json!({"pattern":"needle"}), dir.path());
        let facts = result.structured.unwrap();
        assert_eq!(facts["total_match_count"], 1);
        assert_eq!(facts["skipped_file_count"], 1);
        assert_eq!(result.model_content, "good.txt");
    }

    #[test]
    fn search_reports_incomplete_when_a_long_line_cannot_be_matched() {
        let dir = workspace();
        std::fs::write(dir.path().join("large.txt"), "a".repeat(1024)).unwrap();
        std::fs::write(dir.path().join("good.txt"), "needle\n").unwrap();
        let limits = WorkspaceIoLimits {
            search_line_bytes: 128,
            ..WorkspaceIoLimits::default()
        };
        let result = super::search_text(
            &serde_json::json!({"pattern":"needle"}),
            dir.path(),
            1024,
            limits,
            &|| false,
        );
        let facts = result.structured.unwrap();
        assert!(!facts["scan_complete"].as_bool().unwrap());
        assert_eq!(facts["total_match_count"], 1);
        assert_eq!(facts["skipped_file_count"], 1);
        assert!(result.truncated);
        assert!(result.model_content.contains("Search budget reached"));
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
    fn search_text_pages_keep_every_visible_match_in_order_with_or_without_context() {
        let dir = workspace();
        let row = format!(
            "{}\nTODO {}\n{}\n",
            "前".repeat(50),
            "界".repeat(40),
            "后".repeat(50)
        );
        std::fs::write(dir.path().join("fixture.txt"), row.repeat(20)).unwrap();
        let expected: Vec<_> = (0..20).map(|index| 3 * index + 2).collect();
        let cap = 1024;

        for context in [0, 5] {
            let mut offset = 0;
            let mut observed = Vec::new();
            let mut omitted_context = false;
            loop {
                let result = super::search_text(
                    &serde_json::json!({"path":"fixture.txt","pattern":"TODO","view":"lines","context":context,"offset":offset,"limit":1000}),
                    dir.path(),
                    cap,
                    WorkspaceIoLimits::default(),
                    &|| false,
                );
                assert_eq!("ok", result.status);
                assert!(result.model_content.len() <= cap);
                let facts = result.structured.unwrap();
                assert_eq!(20, facts["total_match_count"]);
                let visible: Vec<_> = result
                    .model_content
                    .lines()
                    .filter_map(|row| {
                        let (line, _) = row.strip_prefix("fixture.txt:")?.split_once(": TODO ")?;
                        line.parse::<u64>().ok()
                    })
                    .collect();
                let matches: Vec<_> = facts["matches"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|row| row["line_number"].as_u64().unwrap())
                    .collect();
                assert_eq!(matches, visible);
                assert!(!visible.is_empty());
                observed.extend(visible);
                omitted_context |= facts["context_truncated"] == true;
                let Some(next) = facts["next_offset"].as_u64() else {
                    assert_eq!(false, facts["has_more"]);
                    break;
                };
                assert!(next > offset);
                assert_eq!(observed.len() as u64, next);
                assert!(result
                    .model_content
                    .contains(&format!("Use offset={next} to continue.")));
                offset = next;
            }
            assert_eq!(expected, observed);
            assert_eq!(context > 0, omitted_context);
        }
    }

    #[test]
    fn search_text_executor_keeps_bulk_local_matches_available() {
        let dir = workspace();
        let row = format!("TODO {}\n", "🙂".repeat(650));
        std::fs::write(dir.path().join("fixture.txt"), row.repeat(20)).unwrap();
        let args = serde_json::json!({"path":"fixture.txt","pattern":"TODO","view":"lines"});
        let limits = WorkspaceIoLimits::default();
        let direct = super::search_text(&args, dir.path(), 1024, limits, &|| false);
        let local = super::search_text_for_executor(&args, dir.path(), limits, &|| false);

        assert!(direct.model_content.len() <= 1024);
        let facts = direct.structured.unwrap();
        assert_eq!(true, facts["matches"][0]["line_truncated"]);
        assert_eq!(1, facts["next_offset"]);
        let facts = local.structured.unwrap();
        assert_eq!(20, facts["match_count"]);
        assert_eq!(false, facts["has_more"]);
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
