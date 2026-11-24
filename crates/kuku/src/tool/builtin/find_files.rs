use std::collections::BinaryHeap;
use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::WorkspaceIoLimits;
use crate::tool::ToolResultEnvelope;

use super::common::{
    FileCancellation, ScanDeadline, bounded_lines, charge_entry, ensure_active, file_read_error,
    glob_match, is_default_excluded_dir, is_io_limit, relative_path, resolve_path,
    verify_directory, workspace_path_blocked,
};

const FIND_FILES_MAX_BYTES: usize = 8_000;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum EntryKind {
    Directory,
    File,
}

#[derive(Default)]
struct Listing {
    count: usize,
    entries_left: usize,
    deadline: ScanDeadline,
    first_bytes: usize,
    first: BinaryHeap<(EntryKind, usize, String)>,
}

impl Listing {
    fn push(&mut self, path: String, kind: EntryKind) {
        let depth = path.bytes().filter(|byte| *byte == b'/').count();
        self.count += 1;
        if self.first_bytes > FIND_FILES_MAX_BYTES
            && self
                .first
                .peek()
                .is_some_and(|(tail_kind, tail_depth, tail_path)| {
                    (kind, depth, path.as_str()) >= (*tail_kind, *tail_depth, tail_path.as_str())
                })
        {
            return;
        }
        self.first_bytes += path.len() + 1;
        self.first.push((kind, depth, path));
        while let Some((_, _, last)) = self.first.peek() {
            let shorter = self.first_bytes - last.len() - 1;
            if shorter <= FIND_FILES_MAX_BYTES {
                break;
            }
            self.first_bytes = shorter;
            self.first.pop();
        }
    }
}

pub(crate) fn find_files(
    args: &Value,
    workspace: &Path,
    limits: WorkspaceIoLimits,
    cancelled: &FileCancellation<'_>,
) -> ToolResultEnvelope {
    let path = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let pattern = args.get("pattern").and_then(Value::as_str);
    let max_depth = args
        .get("max_depth")
        .and_then(Value::as_u64)
        .map(|v| u32::try_from(v).unwrap_or(u32::MAX))
        .unwrap_or_else(|| pattern.map_or(0, |_| u32::MAX));
    let root = match resolve_path(workspace, path) {
        Ok(root) => root,
        Err(result) => return result,
    };

    let mut listing = Listing {
        entries_left: limits.scan_entries,
        deadline: ScanDeadline::new(limits.scan_timeout_ms),
        ..Listing::default()
    };
    let budget_reason = match collect_entries(
        &root.path,
        &root.workspace,
        pattern,
        max_depth,
        0,
        &mut listing,
        cancelled,
    ) {
        Ok(()) => None,
        Err(error) if is_io_limit(&error) => Some(error.to_string()),
        Err(error) => return file_read_error(Path::new(path), error),
    };
    let scan_complete = budget_reason.is_none();
    let entry_count = listing.count;
    let entries: Vec<_> = listing
        .first
        .into_sorted_vec()
        .into_iter()
        .map(|(_, _, path)| path)
        .collect();
    let mut visible = bounded_lines(&entries, FIND_FILES_MAX_BYTES);
    let truncated = visible.len() < entry_count || !scan_complete;
    let model_content = if truncated {
        let scope = if max_depth == 0 {
            "direct entries"
        } else {
            "entries in the search"
        };
        let hint = match &budget_reason {
            None => format!(
                "Found {entry_count} {scope}. Listing shortened. Narrow the path or pattern."
            ),
            Some(reason) => format!(
                "Directory scan incomplete ({reason}). Found {entry_count} {scope} so far. List a smaller folder with path."
            ),
        };
        visible = bounded_lines(&entries, FIND_FILES_MAX_BYTES - hint.len() - 1);
        if visible.is_empty() {
            hint
        } else if !scan_complete {
            format!("{hint}\n{}", visible.join("\n"))
        } else {
            format!("{}\n{hint}", visible.join("\n"))
        }
    } else {
        visible.join("\n")
    };
    let summary = if !scan_complete {
        format!("Incomplete directory scan with {entry_count} entries found so far")
    } else if truncated {
        format!("found {entry_count} entries under {path}, results truncated")
    } else {
        format!("found {entry_count} entries under {path}")
    };
    let structured = serde_json::json!({
        "kind": "file_list",
        "path": path,
        "pattern": pattern,
        "entry_count": entry_count,
        "scan_complete": scan_complete,
        "scan_limit": budget_reason,
        "entries": visible,
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
    listing: &mut Listing,
    cancelled: &FileCancellation<'_>,
) -> std::io::Result<()> {
    ensure_active(cancelled)?;
    if root.is_file() {
        push_entry(root, workspace, pattern, listing);
        return Ok(());
    }
    if depth > max_depth {
        return Ok(());
    }
    verify_directory(root)?;
    for entry in fs::read_dir(root)? {
        listing.deadline.check()?;
        charge_entry(&mut listing.entries_left, cancelled)?;
        let entry = entry?;
        let file_type = entry.file_type()?;
        let file_name = entry.file_name();
        let file_name_str = file_name.to_string_lossy();
        let path = entry.path();
        let relative = relative_path(&path, workspace);
        if workspace_path_blocked(&relative) {
            continue;
        }

        if file_type.is_dir() {
            if is_default_excluded_dir(&file_name_str) {
                continue;
            }
            let (add_dir, recurse) = match pattern {
                None => (true, true),
                Some(p) if !p.contains('/') => (false, true),
                Some(p) => {
                    let dir = format!("{relative}/");
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
                listing.push(format!("{relative}/"), EntryKind::Directory);
            }
            if recurse && depth < max_depth {
                collect_entries(
                    &path,
                    workspace,
                    pattern,
                    max_depth,
                    depth + 1,
                    listing,
                    cancelled,
                )?;
            }
        } else if file_type.is_file() {
            push_entry(&path, workspace, pattern, listing);
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

fn push_entry(path: &Path, workspace: &Path, pattern: Option<&str>, listing: &mut Listing) {
    let Ok(relative) = path.strip_prefix(workspace) else {
        return;
    };
    let relative = relative.to_string_lossy().replace('\\', "/");

    if pattern.is_some_and(|p| !glob_match(p, &relative)) {
        return;
    }

    listing.push(relative, EntryKind::File);
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::super::test_helpers::workspace;
    use super::*;

    fn find_files(args: &Value, workspace: &Path) -> ToolResultEnvelope {
        super::find_files(args, workspace, WorkspaceIoLimits::default(), &|| false)
    }

    #[test]
    fn directory_listing_can_stop_while_traversing_entries() {
        let dir = workspace();
        let checks = AtomicUsize::new(0);
        let stop = || checks.fetch_add(1, Ordering::Relaxed) >= 2;
        let result = super::find_files(
            &serde_json::json!({}),
            dir.path(),
            WorkspaceIoLimits::default(),
            &stop,
        );

        assert_eq!("cancelled", result.status);
        assert_eq!(3, checks.load(Ordering::Relaxed));
    }

    #[test]
    fn directory_listing_reports_incomplete_coverage_before_entries() {
        let dir = tempfile::tempdir().unwrap();
        for file in ["first.txt", "second.txt"] {
            fs::write(dir.path().join(file), "").unwrap();
        }
        let result = super::find_files(
            &serde_json::json!({}),
            dir.path(),
            WorkspaceIoLimits {
                scan_entries: 1,
                ..Default::default()
            },
            &|| false,
        );
        assert!(
            result
                .model_content
                .starts_with("Directory scan incomplete")
        );
        assert!(
            result
                .model_content
                .contains("List a smaller folder with path")
        );
        assert_eq!(result.structured.unwrap()["scan_complete"], false);
    }

    #[test]
    fn find_files_defaults_to_immediate_entries_with_dirs_first() {
        let dir = workspace();
        let result = find_files(&serde_json::json!({"path": "."}), dir.path());

        assert_eq!(result.status, "ok");
        let lines: Vec<_> = result.model_content.lines().collect();
        assert!(lines.contains(&"docs/"), "dirs with trailing /: {lines:?}");
        assert!(lines.contains(&"README.md"));
        assert!(
            lines
                .iter()
                .all(|entry| !entry.trim_end_matches('/').contains('/'))
        );
        assert!(!result.model_content.contains(".git"));
        let structured = result.structured.unwrap();
        assert_eq!(structured["entry_count"], lines.len());
        let nested = find_files(&serde_json::json!({"path": "docs"}), dir.path());
        assert_eq!(nested.model_content, "docs/tools.md");
    }

    #[test]
    fn find_files_filters_globs_and_distinguishes_absolute_paths_from_relative_escapes() {
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

        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("notes.txt"), "outside").unwrap();
        let absolute = find_files(&serde_json::json!({"path": outside.path()}), dir.path());
        assert_eq!(absolute.status, "ok");
        assert!(absolute.model_content.contains("notes.txt"));
        let protected = outside.path().join(".ssh").join("nested");
        std::fs::create_dir_all(&protected).unwrap();
        let protected = find_files(&serde_json::json!({"path": protected}), dir.path());
        assert_eq!(protected.status, "blocked");
        let blocked = find_files(&serde_json::json!({"path": ".."}), dir.path());
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

        let result = find_files(
            &serde_json::json!({"path": ".", "max_depth": 2}),
            dir.path(),
        );
        assert!(!result.model_content.contains("target"));
        assert!(!result.model_content.contains("node_modules"));
        assert!(result.model_content.contains("src/main.rs"));
    }

    #[test]
    fn find_files_penetrates_excluded_dirs_when_explicit_path() {
        let dir = workspace();
        std::fs::create_dir_all(dir.path().join("target/debug")).unwrap();
        std::fs::write(dir.path().join("target/debug/build.txt"), "build").unwrap();

        let result = find_files(
            &serde_json::json!({"path": "target", "max_depth": 1}),
            dir.path(),
        );
        assert!(result.model_content.contains("debug/"));
        assert!(result.model_content.contains("build.txt"));
    }

    #[test]
    fn oversized_listing_keeps_its_count_and_narrowing_hint_inside_the_result_budget() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..180 {
            fs::write(
                dir.path().join(format!("{i:03}-{}.txt", "a".repeat(64))),
                "",
            )
            .unwrap();
        }
        let result = find_files(&serde_json::json!({}), dir.path());
        assert!(result.truncated);
        assert!(result.model_content.len() <= FIND_FILES_MAX_BYTES);
        assert!(
            result.model_content.ends_with(
                "Found 180 direct entries. Listing shortened. Narrow the path or pattern."
            )
        );
        let data = result.structured.unwrap();
        assert_eq!(data["entry_count"], 180);
        assert_eq!(
            data["entries"].as_array().unwrap().len() + 1,
            result.model_content.lines().count()
        );
    }
}
