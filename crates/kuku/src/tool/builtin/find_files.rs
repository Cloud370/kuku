use std::collections::BinaryHeap;
use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::tool::ToolResultEnvelope;

use super::common::{
    bounded_lines, ensure_active, file_read_error, glob_match, is_default_excluded_dir,
    join_bounded_strings, relative_path, resolve_path, verify_directory, workspace_path_blocked,
    FileCancellation,
};

const FIND_FILES_MAX_CHARS: usize = 8_000;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum EntryKind {
    Directory,
    File,
}

#[derive(Default)]
struct Listing {
    count: usize,
    first: BinaryHeap<(EntryKind, usize, String)>,
}

impl Listing {
    fn push(&mut self, path: String, kind: EntryKind) {
        let depth = path.bytes().filter(|byte| *byte == b'/').count();
        self.count += 1;
        self.first.push((kind, depth, path));
        if self.first.len() > FIND_FILES_MAX_CHARS {
            self.first.pop();
        }
    }
}

pub(crate) fn find_files(
    args: &Value,
    workspace: &Path,
    cancelled: &FileCancellation<'_>,
) -> ToolResultEnvelope {
    let path = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let pattern = args.get("pattern").and_then(Value::as_str);
    let max_depth = args
        .get("max_depth")
        .and_then(Value::as_u64)
        .map(|v| u32::try_from(v).unwrap_or(u32::MAX))
        .unwrap_or(u32::MAX);
    let root = match resolve_path(workspace, path) {
        Ok(root) => root,
        Err(result) => return result,
    };

    let mut listing = Listing::default();
    if let Err(error) = collect_entries(
        &root.path,
        &root.workspace,
        pattern,
        max_depth,
        0,
        &mut listing,
        cancelled,
    ) {
        return file_read_error(Path::new(path), error);
    }
    let entry_count = listing.count;
    let entries: Vec<_> = listing
        .first
        .into_sorted_vec()
        .into_iter()
        .map(|(_, _, path)| path)
        .collect();
    let (model_content, truncated) = join_bounded_strings(
        &entries,
        FIND_FILES_MAX_CHARS,
        "(Showing shallowest entries first. Use pattern or max_depth to focus.)",
    );
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
        "entries": bounded_lines(&entries, FIND_FILES_MAX_CHARS),
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
        ensure_active(cancelled)?;
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
            if recurse {
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
        super::find_files(args, workspace, &|| false)
    }

    #[test]
    fn directory_listing_can_stop_while_traversing_entries() {
        let dir = workspace();
        let checks = AtomicUsize::new(0);
        let stop = || checks.fetch_add(1, Ordering::Relaxed) >= 2;
        let result = super::find_files(&serde_json::json!({}), dir.path(), &stop);

        assert_eq!("cancelled", result.status);
        assert_eq!(3, checks.load(Ordering::Relaxed));
    }

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
        let structured = result.structured.unwrap();
        assert!(structured["entry_count"].as_u64().unwrap() >= 3);
        assert!(structured["entries"]
            .as_array()
            .unwrap()
            .contains(&"docs/tools.md".into()));
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
