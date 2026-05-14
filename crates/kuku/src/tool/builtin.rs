use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Instant;

use regex::Regex;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::task::JoinHandle;
use tokio::time::{sleep, Duration};

use crate::event::{EventPayload, StoredEvent};
use crate::tool::ToolResultEnvelope;

const FIND_FILES_MAX_CHARS: usize = 20_000;
const READ_FILE_MAX_CHARS: usize = 80_000;
const SEARCH_TEXT_MAX_CHARS: usize = 80_000;
const RUN_COMMAND_MAX_CHARS: usize = 80_000;
const RUN_COMMAND_TIMEOUT_CAP_SECONDS: u64 = 600;
const MAX_SEARCH_LINE_CHARS: usize = 500;

struct ResolvedPath {
    workspace: PathBuf,
    path: PathBuf,
    relative: String,
}

struct CollectedFile {
    absolute: PathBuf,
    relative: String,
}

struct ReadRequest {
    path: String,
    offset: usize,
    limit: Option<usize>,
}

struct ReadSnapshot {
    event_id: u64,
    start_line: usize,
    line_count: usize,
    is_full_file_snapshot: bool,
}

struct EditRequest {
    path: String,
    old_text: String,
    new_text: String,
    replace_all: bool,
}

struct WriteRequest {
    path: String,
    content: String,
}

struct CommandRequest {
    command: String,
    timeout_seconds: u64,
}

struct WriteSnapshot {
    event_id: u64,
    content_hash: String,
}

struct SearchMatch {
    path: String,
    line_number: usize,
    line: String,
}

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

pub(crate) fn read_file(
    args: &Value,
    workspace: &Path,
    prior_events: &[StoredEvent],
    read_event_id: u64,
) -> ToolResultEnvelope {
    let request = match read_request(args) {
        Ok(request) => request,
        Err(result) => return result,
    };
    let resolved = match resolve_path(workspace, &request.path) {
        Ok(resolved) => resolved,
        Err(result) => return result,
    };
    if !resolved.path.is_file() {
        return ToolResultEnvelope::error(
            format!("failed: not a file: {}", request.path),
            format!("path is not a file: {}", request.path),
        );
    }

    let bytes = match fs::read(&resolved.path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return ToolResultEnvelope::error(
                format!("failed: {error}"),
                format!("error reading file: {}", request.path),
            )
        }
    };
    let content = match String::from_utf8(bytes.clone()) {
        Ok(content) => content,
        Err(_) => {
            return ToolResultEnvelope::error(
                format!("failed: file is not valid UTF-8: {}", resolved.relative),
                format!("file is not valid UTF-8: {}", resolved.relative),
            )
        }
    };

    let hash = content_hash(&bytes);
    let lines = content.lines().collect::<Vec<_>>();
    let total_lines = lines.len();
    let requested_line_count = requested_line_count(request.offset, request.limit, total_lines);

    if requested_line_count > 0 {
        if let Some(prior) = find_covering_read(
            prior_events,
            &resolved.path,
            &hash,
            request.offset,
            requested_line_count,
        ) {
            let summary = format!(
                "already read {}; unchanged since event {}",
                resolved.relative, prior.event_id
            );
            let structured = serde_json::json!({
                "kind": "file_content",
                "path": resolved.relative,
                "canonical_path": resolved.path.to_string_lossy(),
                "content_hash": hash,
                "size_bytes": bytes.len(),
                "read_event_id": read_event_id,
                "prior_read_event_id": prior.event_id,
                "start_line": request.offset,
                "line_count": requested_line_count,
                "total_lines": total_lines,
                "line_numbered": true,
                "is_full_file_snapshot": request.offset == 1 && requested_line_count == total_lines,
                "cached": true,
            });
            return ToolResultEnvelope::ok(summary.clone(), summary, structured);
        }
    }

    let start_index = request.offset.saturating_sub(1).min(total_lines);
    let end_index = request.limit.map_or(total_lines, |limit| {
        start_index.saturating_add(limit).min(total_lines)
    });
    let mut rendered = Vec::new();
    for (index, line) in lines[start_index..end_index].iter().enumerate() {
        let line_number = start_index + index + 1;
        rendered.push(format!("{line_number}\t{line}"));
    }
    let (model_content, truncated) = join_bounded_strings(
        &rendered,
        READ_FILE_MAX_CHARS,
        "(Results are truncated. Use offset and limit to read a smaller range.)",
    );
    let line_count = if truncated {
        model_content
            .lines()
            .filter(|line| !line.starts_with("(Results are truncated."))
            .count()
    } else {
        rendered.len()
    };
    let summary = read_summary(
        &resolved.relative,
        request.offset,
        line_count,
        total_lines,
        truncated,
    );
    let structured = serde_json::json!({
        "kind": "file_content",
        "path": resolved.relative,
        "canonical_path": resolved.path.to_string_lossy(),
        "content_hash": hash,
        "size_bytes": bytes.len(),
        "read_event_id": read_event_id,
        "start_line": request.offset,
        "line_count": line_count,
        "total_lines": total_lines,
        "line_numbered": true,
        "is_full_file_snapshot": request.offset == 1 && line_count == total_lines && !truncated,
        "cached": false,
    });

    if truncated {
        ToolResultEnvelope::ok_truncated(summary, model_content, structured)
    } else {
        ToolResultEnvelope::ok(summary, model_content, structured)
    }
}

pub(crate) fn edit_file(
    args: &Value,
    workspace: &Path,
    prior_events: &[StoredEvent],
) -> ToolResultEnvelope {
    let request = match edit_request(args) {
        Ok(request) => request,
        Err(result) => return result,
    };
    let resolved = match resolve_path(workspace, &request.path) {
        Ok(resolved) => resolved,
        Err(result) => return result,
    };
    if !resolved.path.is_file() {
        return ToolResultEnvelope::error(
            format!("failed: not a file: {}", request.path),
            format!("path is not a file: {}", request.path),
        );
    }

    let bytes = match fs::read(&resolved.path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return ToolResultEnvelope::error(
                format!("failed: {error}"),
                format!("error reading file: {}", request.path),
            )
        }
    };
    let content = match String::from_utf8(bytes.clone()) {
        Ok(content) => content,
        Err(_) => {
            return ToolResultEnvelope::error(
                format!("failed: file is not valid UTF-8: {}", resolved.relative),
                format!("file is not valid UTF-8: {}", resolved.relative),
            )
        }
    };
    let current_hash = content_hash(&bytes);
    let Some(snapshot) =
        find_write_snapshot(prior_events, &resolved.path, false, Some(&request.old_text))
    else {
        return ToolResultEnvelope::error(
            format!("failed: read {} before editing", resolved.relative),
            format!(
                "edit_file requires a prior successful read_file snapshot for {}",
                resolved.relative
            ),
        );
    };
    if snapshot.content_hash != current_hash {
        return ToolResultEnvelope::error(
            format!(
                "failed: {} changed since event {}",
                resolved.relative, snapshot.event_id
            ),
            format!(
                "file changed since it was read; read {} again before editing",
                resolved.relative
            ),
        );
    }

    let replacement_count = content.matches(&request.old_text).count();
    if replacement_count == 0 {
        return ToolResultEnvelope::error(
            format!("failed: old_text not found in {}", resolved.relative),
            "old_text was not found".to_string(),
        );
    }
    if replacement_count > 1 && !request.replace_all {
        return ToolResultEnvelope::error(
            format!(
                "failed: old_text matched {replacement_count} times in {}",
                resolved.relative
            ),
            "old_text is not unique; provide more context or set replace_all=true".to_string(),
        );
    }

    let edited = if request.replace_all {
        content.replace(&request.old_text, &request.new_text)
    } else {
        content.replacen(&request.old_text, &request.new_text, 1)
    };
    if let Err(error) = write_atomically(&resolved.path, edited.as_bytes()) {
        return ToolResultEnvelope::error(
            format!("failed: {error}"),
            format!("error writing file: {}", resolved.relative),
        );
    }

    let summary = format!(
        "edited {}, {replacement_count} replacement{}",
        resolved.relative,
        plural(replacement_count)
    );
    ToolResultEnvelope::ok(
        summary.clone(),
        summary,
        serde_json::json!({
            "kind": "file_edit",
            "path": resolved.relative,
            "replacement_count": replacement_count,
            "bytes_written": edited.len(),
            "content_hash": content_hash(edited.as_bytes()),
        }),
    )
}

pub(crate) fn write_file(
    args: &Value,
    workspace: &Path,
    prior_events: &[StoredEvent],
) -> ToolResultEnvelope {
    let request = match write_request(args) {
        Ok(request) => request,
        Err(result) => return result,
    };
    let resolved = match resolve_write_path(workspace, &request.path) {
        Ok(resolved) => resolved,
        Err(result) => return result,
    };

    let created = !resolved.path.exists();
    if !created {
        if !resolved.path.is_file() {
            return ToolResultEnvelope::error(
                format!("failed: not a file: {}", request.path),
                format!("path is not a file: {}", request.path),
            );
        }
        let bytes = match fs::read(&resolved.path) {
            Ok(bytes) => bytes,
            Err(error) => {
                return ToolResultEnvelope::error(
                    format!("failed: {error}"),
                    format!("error reading file: {}", request.path),
                )
            }
        };
        if String::from_utf8(bytes.clone()).is_err() {
            return ToolResultEnvelope::error(
                format!("failed: file is not valid UTF-8: {}", resolved.relative),
                format!("file is not valid UTF-8: {}", resolved.relative),
            );
        }
        let current_hash = content_hash(&bytes);
        let Some(snapshot) = find_write_snapshot(prior_events, &resolved.path, true, None) else {
            return ToolResultEnvelope::error(
                format!(
                    "failed: fully read {} before overwriting",
                    resolved.relative
                ),
                format!(
                    "write_file requires a prior full read_file snapshot before overwriting {}",
                    resolved.relative
                ),
            );
        };
        if snapshot.content_hash != current_hash {
            return ToolResultEnvelope::error(
                format!(
                    "failed: {} changed since event {}",
                    resolved.relative, snapshot.event_id
                ),
                format!(
                    "file changed since it was read; read {} again before overwriting",
                    resolved.relative
                ),
            );
        }
    }

    if let Err(error) = write_atomically(&resolved.path, request.content.as_bytes()) {
        return ToolResultEnvelope::error(
            format!("failed: {error}"),
            format!("error writing file: {}", resolved.relative),
        );
    }

    let line_count = request.content.lines().count();
    let summary = format!(
        "wrote {}, {line_count} line{}",
        resolved.relative,
        plural(line_count)
    );
    ToolResultEnvelope::ok(
        summary.clone(),
        summary,
        serde_json::json!({
            "kind": "file_write",
            "path": resolved.relative,
            "line_count": line_count,
            "bytes_written": request.content.len(),
            "content_hash": content_hash(request.content.as_bytes()),
            "created": created,
        }),
    )
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

pub(crate) async fn run_command(args: &Value, workspace: &Path) -> ToolResultEnvelope {
    let request = match command_request(args) {
        Ok(request) => request,
        Err(result) => return result,
    };
    if let Some(reason) = blocked_command_reason(&request.command) {
        return ToolResultEnvelope::blocked(
            format!("blocked by command hard guard: {reason}"),
            format!("blocked by command hard guard: {reason}"),
        );
    }
    let workspace = match workspace.canonicalize() {
        Ok(path) => path,
        Err(_) => {
            return ToolResultEnvelope::error(
                "failed: workspace not found",
                "workspace path does not exist",
            )
        }
    };

    let started = Instant::now();
    let mut command = shell_command(&request.command);
    command
        .current_dir(&workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return ToolResultEnvelope::error(
                format!("failed: command could not start: {error}"),
                format!("command could not start: {error}"),
            )
        }
    };
    let stdout_task = child.stdout.take().map(read_pipe_task);
    let stderr_task = child.stderr.take().map(read_pipe_task);

    let status = tokio::select! {
        status = child.wait() => match status {
            Ok(status) => status,
            Err(error) => {
                return ToolResultEnvelope::error(
                    format!("failed: command wait error: {error}"),
                    format!("command wait error: {error}"),
                )
            }
        },
        _ = sleep(Duration::from_secs(request.timeout_seconds)) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            let duration_ms = started.elapsed().as_millis() as u64;
            let stdout = collect_pipe(stdout_task).await.unwrap_or_default();
            let stderr = collect_pipe(stderr_task).await.unwrap_or_default();
            return command_timeout_result(
                &request.command,
                request.timeout_seconds,
                duration_ms,
                &stdout,
                &stderr,
            );
        }
    };

    let duration_ms = started.elapsed().as_millis() as u64;
    let stdout = match collect_pipe(stdout_task).await {
        Ok(bytes) => bytes,
        Err(error) => {
            return ToolResultEnvelope::error(
                format!("failed: stdout capture error: {error}"),
                format!("stdout capture error: {error}"),
            )
        }
    };
    let stderr = match collect_pipe(stderr_task).await {
        Ok(bytes) => bytes,
        Err(error) => {
            return ToolResultEnvelope::error(
                format!("failed: stderr capture error: {error}"),
                format!("stderr capture error: {error}"),
            )
        }
    };

    command_output_result(&request.command, status, stdout, stderr, duration_ms)
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

fn glob_match(pattern: &str, path: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(suffix) = pattern.strip_prefix("**/*") {
        return path.ends_with(suffix);
    }
    if let Some((prefix, suffix)) = pattern.split_once("/**/*") {
        return path.starts_with(&format!("{prefix}/")) && path.ends_with(suffix);
    }
    if let Some(prefix) = pattern.strip_suffix("/**") {
        return path == prefix || path.starts_with(&format!("{prefix}/"));
    }
    if let Some(prefix) = pattern.strip_suffix("/*") {
        return path
            .strip_prefix(&format!("{prefix}/"))
            .is_some_and(|rest| !rest.contains('/'));
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return path
            .rsplit('/')
            .next()
            .is_some_and(|name| name.ends_with(suffix));
    }
    path == pattern
}

fn read_request(args: &Value) -> Result<ReadRequest, ToolResultEnvelope> {
    let Some(path) = args.get("path").and_then(Value::as_str) else {
        return Err(ToolResultEnvelope::error(
            "failed: missing path",
            "read_file requires path",
        ));
    };
    Ok(ReadRequest {
        path: path.to_string(),
        offset: optional_positive_usize(args, "offset")?.unwrap_or(1),
        limit: optional_positive_usize(args, "limit")?,
    })
}

fn edit_request(args: &Value) -> Result<EditRequest, ToolResultEnvelope> {
    let Some(path) = args.get("path").and_then(Value::as_str) else {
        return Err(ToolResultEnvelope::error(
            "failed: missing path",
            "edit_file requires path",
        ));
    };
    let Some(old_text) = args.get("old_text").and_then(Value::as_str) else {
        return Err(ToolResultEnvelope::error(
            "failed: missing old_text",
            "edit_file requires old_text",
        ));
    };
    let Some(new_text) = args.get("new_text").and_then(Value::as_str) else {
        return Err(ToolResultEnvelope::error(
            "failed: missing new_text",
            "edit_file requires new_text",
        ));
    };
    if old_text.is_empty() {
        return Err(ToolResultEnvelope::error(
            "failed: old_text is empty",
            "old_text must not be empty",
        ));
    }
    Ok(EditRequest {
        path: path.to_string(),
        old_text: old_text.to_string(),
        new_text: new_text.to_string(),
        replace_all: args
            .get("replace_all")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn write_request(args: &Value) -> Result<WriteRequest, ToolResultEnvelope> {
    let Some(path) = args.get("path").and_then(Value::as_str) else {
        return Err(ToolResultEnvelope::error(
            "failed: missing path",
            "write_file requires path",
        ));
    };
    let Some(content) = args.get("content").and_then(Value::as_str) else {
        return Err(ToolResultEnvelope::error(
            "failed: missing content",
            "write_file requires content",
        ));
    };
    Ok(WriteRequest {
        path: path.to_string(),
        content: content.to_string(),
    })
}

fn command_request(args: &Value) -> Result<CommandRequest, ToolResultEnvelope> {
    let Some(command) = args.get("command").and_then(Value::as_str) else {
        return Err(ToolResultEnvelope::error(
            "failed: missing command",
            "run_command requires command",
        ));
    };
    if command.trim().is_empty() {
        return Err(ToolResultEnvelope::error(
            "failed: command is empty",
            "command must not be empty",
        ));
    }
    let Some(timeout_seconds) = args.get("timeout").and_then(Value::as_u64) else {
        return Err(ToolResultEnvelope::error(
            "failed: missing timeout",
            "run_command requires timeout in seconds",
        ));
    };
    if timeout_seconds == 0 {
        return Err(ToolResultEnvelope::error(
            "failed: timeout must be >= 1",
            "timeout must be >= 1",
        ));
    }
    if timeout_seconds > RUN_COMMAND_TIMEOUT_CAP_SECONDS {
        return Err(ToolResultEnvelope::error(
            format!("failed: timeout must be <= {RUN_COMMAND_TIMEOUT_CAP_SECONDS}"),
            format!("timeout must be <= {RUN_COMMAND_TIMEOUT_CAP_SECONDS}"),
        ));
    }

    Ok(CommandRequest {
        command: command.to_string(),
        timeout_seconds,
    })
}

fn blocked_command_reason(command: &str) -> Option<&'static str> {
    let normalized = command
        .to_ascii_lowercase()
        .replace("&&", "\n")
        .replace("||", "\n")
        .replace(';', "\n");
    for segment in normalized
        .lines()
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
    {
        let segment = segment.strip_prefix("sudo ").unwrap_or(segment).trim();
        for (prefix, reason) in [
            ("git push", "git push affects a remote repository"),
            ("git reset --hard", "git reset --hard discards local work"),
            ("git clean -f", "git clean deletes untracked files"),
            ("rm -rf", "recursive force delete is destructive"),
            ("rm -fr", "recursive force delete is destructive"),
            (
                "cargo publish",
                "publish affects external package registries",
            ),
            ("npm publish", "publish affects external package registries"),
            (
                "pnpm publish",
                "publish affects external package registries",
            ),
            (
                "yarn publish",
                "publish affects external package registries",
            ),
            ("bun publish", "publish affects external package registries"),
            ("npm run deploy", "deploy affects external systems"),
            ("pnpm deploy", "deploy affects external systems"),
            ("yarn deploy", "deploy affects external systems"),
            ("make deploy", "deploy affects external systems"),
            ("cargo release", "release affects external systems"),
        ] {
            if segment.starts_with(prefix) {
                return Some(reason);
            }
        }
    }
    None
}

#[cfg(windows)]
fn shell_command(command: &str) -> Command {
    let mut child = Command::new("cmd");
    child.arg("/C").arg(command);
    child
}

#[cfg(not(windows))]
fn shell_command(command: &str) -> Command {
    let mut child = Command::new("sh");
    child.arg("-c").arg(command);
    child
}

fn optional_positive_usize(args: &Value, field: &str) -> Result<Option<usize>, ToolResultEnvelope> {
    let Some(value) = args.get(field) else {
        return Ok(None);
    };
    let Some(value) = value.as_u64() else {
        return Err(ToolResultEnvelope::error(
            format!("failed: {field} must be a positive integer"),
            format!("{field} must be a positive integer"),
        ));
    };
    if value == 0 {
        return Err(ToolResultEnvelope::error(
            format!("failed: {field} must be >= 1"),
            format!("{field} must be >= 1"),
        ));
    }
    usize::try_from(value).map(Some).map_err(|_| {
        ToolResultEnvelope::error(
            format!("failed: {field} is too large"),
            format!("{field} is too large"),
        )
    })
}

fn resolve_path(workspace: &Path, path: &str) -> Result<ResolvedPath, ToolResultEnvelope> {
    let workspace = workspace.canonicalize().map_err(|_| {
        ToolResultEnvelope::error(
            "failed: workspace not found",
            "workspace path does not exist",
        )
    })?;
    let candidate = Path::new(path);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        workspace.join(candidate)
    };
    let resolved = joined.canonicalize().map_err(|_| {
        ToolResultEnvelope::error(
            format!("failed: path not found: {path}"),
            format!("path does not exist: {path}"),
        )
    })?;

    if !resolved.starts_with(&workspace) {
        return Err(ToolResultEnvelope::blocked(
            format!("blocked: path outside workspace: {path}"),
            format!("path is outside the workspace: {path}"),
        ));
    }

    let relative = relative_path(&resolved, &workspace);
    if is_blocked_relative_path(&relative) {
        return Err(ToolResultEnvelope::blocked(
            format!("blocked: path is not readable: {relative}"),
            format!("path is blocked by read guard: {relative}"),
        ));
    }

    Ok(ResolvedPath {
        workspace,
        path: resolved,
        relative,
    })
}

fn resolve_write_path(workspace: &Path, path: &str) -> Result<ResolvedPath, ToolResultEnvelope> {
    match resolve_path(workspace, path) {
        Ok(existing) => return Ok(existing),
        Err(result) if result.status == "blocked" => return Err(result),
        Err(_) => {}
    }

    let workspace = workspace.canonicalize().map_err(|_| {
        ToolResultEnvelope::error(
            "failed: workspace not found",
            "workspace path does not exist",
        )
    })?;
    let candidate = Path::new(path);
    let joined = normalize_existing_components(if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        workspace.join(candidate)
    });
    let Some(parent) = joined.parent() else {
        return Err(ToolResultEnvelope::error(
            format!("failed: missing parent for {path}"),
            format!("path has no parent directory: {path}"),
        ));
    };
    let parent = parent.canonicalize().map_err(|_| {
        ToolResultEnvelope::error(
            format!("failed: parent path not found: {path}"),
            format!("parent directory does not exist: {path}"),
        )
    })?;
    if !parent.starts_with(&workspace) {
        return Err(ToolResultEnvelope::blocked(
            format!("blocked: path outside workspace: {path}"),
            format!("path is outside the workspace: {path}"),
        ));
    }
    let file_name = joined.file_name().ok_or_else(|| {
        ToolResultEnvelope::error(
            format!("failed: missing file name: {path}"),
            format!("path has no file name: {path}"),
        )
    })?;
    let resolved = parent.join(file_name);
    let relative = relative_path(&resolved, &workspace);
    if is_blocked_relative_path(&relative) {
        return Err(ToolResultEnvelope::blocked(
            format!("blocked: path is not writable: {relative}"),
            format!("path is blocked by write guard: {relative}"),
        ));
    }
    Ok(ResolvedPath {
        workspace,
        path: resolved,
        relative,
    })
}

fn normalize_existing_components(path: PathBuf) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn relative_path(path: &Path, workspace: &Path) -> String {
    path.strip_prefix(workspace)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn is_blocked_relative_path(path: &str) -> bool {
    path.split('/').any(|part| part == ".git" || part == ".ssh")
        || path.rsplit('/').next().is_some_and(is_sensitive_file_name)
}

fn is_sensitive_file_name(name: &str) -> bool {
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

fn requested_line_count(offset: usize, limit: Option<usize>, total_lines: usize) -> usize {
    if offset > total_lines {
        return 0;
    }
    let available = total_lines - offset + 1;
    limit.map_or(available, |limit| limit.min(available))
}

fn read_summary(
    path: &str,
    offset: usize,
    line_count: usize,
    total_lines: usize,
    truncated: bool,
) -> String {
    if total_lines == 0 {
        return format!("read {path}, 0 lines of 0");
    }
    if line_count == 0 {
        return format!("read {path}, no lines at offset {offset} of {total_lines}");
    }
    let end_line = offset + line_count - 1;
    if truncated {
        format!("read {path}, lines {offset}-{end_line} of {total_lines}, results truncated")
    } else {
        format!("read {path}, lines {offset}-{end_line} of {total_lines}")
    }
}

fn content_hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("sha256:{digest:x}")
}

fn find_covering_read(
    events: &[StoredEvent],
    canonical_path: &Path,
    content_hash: &str,
    start_line: usize,
    line_count: usize,
) -> Option<ReadSnapshot> {
    let canonical_path = canonical_path.to_string_lossy();
    events.iter().rev().find_map(|event| {
        let EventPayload::ToolResult {
            status,
            structured: Some(structured),
            ..
        } = &event.payload
        else {
            return None;
        };
        if status != "ok" || structured["kind"] != "file_content" || structured["cached"] == true {
            return None;
        }
        if structured["canonical_path"].as_str()? != canonical_path
            || structured["content_hash"].as_str()? != content_hash
        {
            return None;
        }
        let snapshot = ReadSnapshot {
            event_id: structured["read_event_id"].as_u64().unwrap_or(event.id),
            start_line: structured["start_line"].as_u64()? as usize,
            line_count: structured["line_count"].as_u64()? as usize,
            is_full_file_snapshot: structured["is_full_file_snapshot"]
                .as_bool()
                .unwrap_or(false),
        };
        if snapshot.covers(start_line, line_count) {
            Some(snapshot)
        } else {
            None
        }
    })
}

impl ReadSnapshot {
    fn covers(&self, start_line: usize, line_count: usize) -> bool {
        self.is_full_file_snapshot
            || (self.start_line == start_line && self.line_count == line_count)
    }
}

fn find_write_snapshot(
    events: &[StoredEvent],
    canonical_path: &Path,
    require_full_file: bool,
    required_text: Option<&str>,
) -> Option<WriteSnapshot> {
    let canonical_path = canonical_path.to_string_lossy();
    events.iter().rev().find_map(|event| {
        let EventPayload::ToolResult {
            status,
            model_content,
            structured: Some(structured),
            ..
        } = &event.payload
        else {
            return None;
        };
        if status != "ok" || structured["kind"] != "file_content" || structured["cached"] == true {
            return None;
        }
        if structured["canonical_path"].as_str()? != canonical_path {
            return None;
        }
        let is_full_file_snapshot = structured["is_full_file_snapshot"]
            .as_bool()
            .unwrap_or(false);
        if require_full_file && !is_full_file_snapshot {
            return None;
        }
        if !is_full_file_snapshot && required_text.is_some_and(|text| !model_content.contains(text))
        {
            return None;
        }
        Some(WriteSnapshot {
            event_id: structured["read_event_id"].as_u64().unwrap_or(event.id),
            content_hash: structured["content_hash"].as_str()?.to_string(),
        })
    })
}

fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temp_path = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("kuku")
    ));
    fs::write(&temp_path, bytes)?;
    fs::rename(temp_path, path)
}

fn plural(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
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

fn join_bounded_strings(
    lines: &[String],
    max_chars: usize,
    truncation_message: &str,
) -> (String, bool) {
    let mut model_content = String::new();
    let mut truncated = false;
    for line in lines {
        let next_len = model_content.len() + line.len() + usize::from(!model_content.is_empty());
        if next_len > max_chars {
            truncated = true;
            break;
        }
        if !model_content.is_empty() {
            model_content.push('\n');
        }
        model_content.push_str(line);
    }
    if truncated {
        if !model_content.is_empty() {
            model_content.push('\n');
        }
        model_content.push_str(truncation_message);
    }
    (model_content, truncated)
}

fn read_pipe_task<P>(pipe: P) -> JoinHandle<io::Result<Vec<u8>>>
where
    P: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut pipe = pipe;
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes).await?;
        Ok(bytes)
    })
}

async fn collect_pipe(task: Option<JoinHandle<io::Result<Vec<u8>>>>) -> io::Result<Vec<u8>> {
    match task {
        Some(task) => task.await.map_err(io::Error::other)?,
        None => Ok(Vec::new()),
    }
}

fn command_output_result(
    command: &str,
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    duration_ms: u64,
) -> ToolResultEnvelope {
    let stdout = String::from_utf8_lossy(&stdout).into_owned();
    let stderr = String::from_utf8_lossy(&stderr).into_owned();
    let stdout_lines = line_count(&stdout);
    let stderr_lines = line_count(&stderr);
    let exit_code = status.code();
    let rendered = render_command_output(&stdout, &stderr);
    let (model_content, truncated) = truncate_text(
        rendered,
        RUN_COMMAND_MAX_CHARS,
        "(Command output truncated. Run a narrower command or inspect files with dedicated tools.)",
    );
    let mut summary = format!(
        "command exited {} in {}ms, stdout {} line{}, stderr {} line{}",
        exit_code.map_or_else(|| "unknown".to_string(), |code| code.to_string()),
        duration_ms,
        stdout_lines,
        plural(stdout_lines),
        stderr_lines,
        plural(stderr_lines),
    );
    if truncated {
        summary.push_str(", output truncated");
    }
    let structured = serde_json::json!({
        "kind": "command_result",
        "command": command,
        "exit_code": exit_code,
        "duration_ms": duration_ms,
        "stdout_lines": stdout_lines,
        "stderr_lines": stderr_lines,
        "timed_out": false,
        "line_numbered": false,
    });

    if truncated {
        ToolResultEnvelope::ok_truncated(summary, model_content, structured)
    } else {
        ToolResultEnvelope::ok(summary, model_content, structured)
    }
}

fn command_timeout_result(
    command: &str,
    timeout_seconds: u64,
    duration_ms: u64,
    stdout: &[u8],
    stderr: &[u8],
) -> ToolResultEnvelope {
    let stdout = String::from_utf8_lossy(stdout).into_owned();
    let stderr = String::from_utf8_lossy(stderr).into_owned();
    let stdout_lines = line_count(&stdout);
    let stderr_lines = line_count(&stderr);
    let rendered = render_command_output(&stdout, &stderr);
    let model_content = if rendered == "(command produced no output)" {
        format!("command timed out after {timeout_seconds}s")
    } else {
        format!("command timed out after {timeout_seconds}s\n\n{rendered}")
    };
    let (model_content, truncated) = truncate_text(
        model_content,
        RUN_COMMAND_MAX_CHARS,
        "(Command output truncated. Run a narrower command or inspect files with dedicated tools.)",
    );
    let mut summary = format!(
        "command timed out after {timeout_seconds}s, stdout {stdout_lines} line{}, stderr {stderr_lines} line{}",
        plural(stdout_lines),
        plural(stderr_lines),
    );
    if truncated {
        summary.push_str(", output truncated");
    }
    ToolResultEnvelope {
        status: "error".to_string(),
        summary,
        model_content,
        truncated,
        structured: Some(serde_json::json!({
            "kind": "command_result",
            "command": command,
            "exit_code": null,
            "duration_ms": duration_ms,
            "stdout_lines": stdout_lines,
            "stderr_lines": stderr_lines,
            "timed_out": true,
            "line_numbered": false,
        })),
    }
}

fn render_command_output(stdout: &str, stderr: &str) -> String {
    let mut sections = Vec::new();
    if !stdout.is_empty() {
        sections.push(format!(
            "stdout:\n{}",
            stdout.trim_end_matches(['\r', '\n'])
        ));
    }
    if !stderr.is_empty() {
        sections.push(format!(
            "stderr:\n{}",
            stderr.trim_end_matches(['\r', '\n'])
        ));
    }
    if sections.is_empty() {
        "(command produced no output)".to_string()
    } else {
        sections.join("\n\n")
    }
}

fn line_count(text: &str) -> usize {
    text.lines().count()
}

fn truncate_text(mut text: String, max_chars: usize, truncation_message: &str) -> (String, bool) {
    if text.len() <= max_chars {
        return (text, false);
    }
    let keep = max_chars.saturating_sub(truncation_message.len() + 1);
    text.truncate(keep);
    while !text.is_char_boundary(text.len()) {
        text.pop();
    }
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(truncation_message);
    (text, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("docs")).unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join("README.md"), "# Project").unwrap();
        std::fs::write(dir.path().join("docs/tools.md"), "# Tools").unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.path().join(".git/config"), "[core]").unwrap();
        dir
    }

    #[cfg(unix)]
    fn stdout_command() -> &'static str {
        "printf 'hello\nworld\n'"
    }

    #[cfg(windows)]
    fn stdout_command() -> &'static str {
        "echo hello && echo world"
    }

    #[cfg(unix)]
    fn stderr_nonzero_command() -> &'static str {
        "printf 'bad\n' >&2; exit 7"
    }

    #[cfg(windows)]
    fn stderr_nonzero_command() -> &'static str {
        "echo bad 1>&2 && exit /B 7"
    }

    #[cfg(unix)]
    fn read_marker_command() -> &'static str {
        "cat cwd-marker.txt"
    }

    #[cfg(windows)]
    fn read_marker_command() -> &'static str {
        "type cwd-marker.txt"
    }

    #[cfg(unix)]
    fn sleep_command(seconds: u64) -> String {
        format!("sleep {seconds}")
    }

    #[cfg(windows)]
    fn sleep_command(seconds: u64) -> String {
        format!("ping 127.0.0.1 -n {} > NUL", seconds + 1)
    }

    #[cfg(unix)]
    fn noisy_timeout_command() -> String {
        format!(
            "printf '{}'; sleep 2",
            "x".repeat(RUN_COMMAND_MAX_CHARS + 100)
        )
    }

    #[cfg(windows)]
    fn noisy_timeout_command() -> String {
        format!(
            "echo {} && ping 127.0.0.1 -n 3 > NUL",
            "x".repeat(RUN_COMMAND_MAX_CHARS + 100)
        )
    }

    fn stored_read_event(id: u64, structured: Value) -> StoredEvent {
        StoredEvent {
            id,
            payload: EventPayload::ToolResult {
                turn: 1,
                ts: "2026-05-14T00:00:00Z".to_string(),
                tool_call_id: format!("tool_{id}"),
                status: "ok".to_string(),
                summary: "read".to_string(),
                model_content: "content".to_string(),
                truncated: false,
                structured: Some(structured),
            },
        }
    }

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

    #[test]
    fn read_file_returns_line_numbered_content_and_snapshot_metadata() {
        let dir = workspace();
        std::fs::write(dir.path().join("README.md"), "first\nsecond\nthird\n").unwrap();

        let result = read_file(
            &serde_json::json!({"path": "README.md", "offset": 2, "limit": 2}),
            dir.path(),
            &[],
            17,
        );

        assert_eq!(result.status, "ok");
        assert_eq!(result.summary, "read README.md, lines 2-3 of 3");
        assert_eq!(result.model_content, "2\tsecond\n3\tthird");
        let structured = result.structured.unwrap();
        assert_eq!(structured["kind"], "file_content");
        assert_eq!(structured["path"], "README.md");
        assert!(structured["canonical_path"]
            .as_str()
            .unwrap()
            .ends_with("README.md"));
        assert!(structured["content_hash"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
        assert_eq!(structured["read_event_id"], 17);
        assert_eq!(structured["start_line"], 2);
        assert_eq!(structured["line_count"], 2);
        assert_eq!(structured["total_lines"], 3);
        assert_eq!(structured["line_numbered"], true);
        assert_eq!(structured["is_full_file_snapshot"], false);
        assert_eq!(structured["cached"], false);
    }

    #[test]
    fn read_file_uses_event_derived_cache_for_unchanged_covered_reads() {
        let dir = workspace();
        let content = b"first\nsecond\nthird\n";
        std::fs::write(dir.path().join("README.md"), content).unwrap();
        let canonical = dir.path().join("README.md").canonicalize().unwrap();
        let prior = stored_read_event(
            17,
            serde_json::json!({
                "kind": "file_content",
                "path": "README.md",
                "canonical_path": canonical.to_string_lossy(),
                "content_hash": content_hash(content),
                "read_event_id": 17,
                "start_line": 1,
                "line_count": 3,
                "total_lines": 3,
                "is_full_file_snapshot": true,
                "cached": false,
            }),
        );

        let result = read_file(
            &serde_json::json!({"path": "README.md", "offset": 2, "limit": 1}),
            dir.path(),
            &[prior],
            21,
        );

        assert_eq!(result.status, "ok");
        assert_eq!(
            result.model_content,
            "already read README.md; unchanged since event 17"
        );
        let structured = result.structured.unwrap();
        assert_eq!(structured["cached"], true);
        assert_eq!(structured["prior_read_event_id"], 17);
        assert_eq!(structured["read_event_id"], 21);
    }

    #[test]
    fn read_file_blocks_sensitive_paths_and_rejects_invalid_pagination() {
        let dir = workspace();
        std::fs::write(dir.path().join(".env"), "TOKEN=secret").unwrap();
        std::fs::write(dir.path().join("credentials.json"), "{}").unwrap();

        let blocked_env = read_file(&serde_json::json!({"path": ".env"}), dir.path(), &[], 1);
        assert_eq!(blocked_env.status, "blocked");

        let blocked_credentials = read_file(
            &serde_json::json!({"path": "credentials.json"}),
            dir.path(),
            &[],
            1,
        );
        assert_eq!(blocked_credentials.status, "blocked");

        let invalid_offset = read_file(
            &serde_json::json!({"path": "README.md", "offset": 0}),
            dir.path(),
            &[],
            1,
        );
        assert_eq!(invalid_offset.status, "error");
        assert!(invalid_offset.model_content.contains("offset must be >= 1"));

        let invalid_limit = read_file(
            &serde_json::json!({"path": "README.md", "limit": 0}),
            dir.path(),
            &[],
            1,
        );
        assert_eq!(invalid_limit.status, "error");
        assert!(invalid_limit.model_content.contains("limit must be >= 1"));
    }

    #[test]
    fn read_file_reports_empty_and_eof_ranges_explicitly() {
        let dir = workspace();
        std::fs::write(dir.path().join("empty.txt"), "").unwrap();

        let empty = read_file(
            &serde_json::json!({"path": "empty.txt"}),
            dir.path(),
            &[],
            1,
        );
        assert_eq!(empty.summary, "read empty.txt, 0 lines of 0");
        assert_eq!(empty.model_content, "");

        let canonical = dir.path().join("README.md").canonicalize().unwrap();
        let prior = stored_read_event(
            17,
            serde_json::json!({
                "kind": "file_content",
                "path": "README.md",
                "canonical_path": canonical.to_string_lossy(),
                "content_hash": content_hash(b"# Project"),
                "read_event_id": 17,
                "start_line": 1,
                "line_count": 1,
                "total_lines": 1,
                "is_full_file_snapshot": true,
                "cached": false,
            }),
        );
        let eof = read_file(
            &serde_json::json!({"path": "README.md", "offset": 99}),
            dir.path(),
            &[prior],
            2,
        );
        assert_eq!(eof.summary, "read README.md, no lines at offset 99 of 1");
        assert_eq!(eof.model_content, "");
        assert_eq!(eof.structured.unwrap()["cached"], false);
    }

    fn read_snapshot_event(
        id: u64,
        dir: &Path,
        path: &str,
        content: &[u8],
        full: bool,
        model_content: &str,
    ) -> StoredEvent {
        let canonical = dir.join(path).canonicalize().unwrap();
        StoredEvent {
            id,
            payload: EventPayload::ToolResult {
                turn: 1,
                ts: "2026-05-14T00:00:00Z".to_string(),
                tool_call_id: format!("tool_{id}"),
                status: "ok".to_string(),
                summary: "read".to_string(),
                model_content: model_content.to_string(),
                truncated: false,
                structured: Some(serde_json::json!({
                    "kind": "file_content",
                    "path": path,
                    "canonical_path": canonical.to_string_lossy(),
                    "content_hash": content_hash(content),
                    "read_event_id": id,
                    "start_line": 1,
                    "line_count": if full { String::from_utf8_lossy(content).lines().count() } else { 1 },
                    "total_lines": String::from_utf8_lossy(content).lines().count(),
                    "is_full_file_snapshot": full,
                    "cached": false,
                })),
            },
        }
    }

    #[test]
    fn edit_file_requires_prior_read_and_rejects_stale_snapshot() {
        let dir = workspace();
        std::fs::write(dir.path().join("README.md"), "alpha\nbeta\n").unwrap();

        let missing = edit_file(
            &serde_json::json!({"path": "README.md", "old_text": "alpha", "new_text": "omega"}),
            dir.path(),
            &[],
        );
        assert_eq!(missing.status, "error");
        assert!(missing
            .model_content
            .contains("prior successful read_file snapshot"));

        let snapshot = read_snapshot_event(
            17,
            dir.path(),
            "README.md",
            b"alpha\nbeta\n",
            true,
            "1\talpha\n2\tbeta",
        );
        std::fs::write(dir.path().join("README.md"), "changed\nbeta\n").unwrap();
        let stale = edit_file(
            &serde_json::json!({"path": "README.md", "old_text": "beta", "new_text": "gamma"}),
            dir.path(),
            &[snapshot],
        );
        assert_eq!(stale.status, "error");
        assert!(stale.model_content.contains("read README.md again"));
    }

    #[test]
    fn edit_file_replaces_unique_text_or_all_matches() {
        let dir = workspace();
        let content = b"alpha\nbeta\nalpha\n";
        std::fs::write(dir.path().join("README.md"), content).unwrap();
        let snapshot = read_snapshot_event(
            17,
            dir.path(),
            "README.md",
            content,
            true,
            "1\talpha\n2\tbeta\n3\talpha",
        );

        let ambiguous = edit_file(
            &serde_json::json!({"path": "README.md", "old_text": "alpha", "new_text": "omega"}),
            dir.path(),
            std::slice::from_ref(&snapshot),
        );
        assert_eq!(ambiguous.status, "error");
        assert!(ambiguous.model_content.contains("not unique"));

        let unique = edit_file(
            &serde_json::json!({"path": "README.md", "old_text": "beta", "new_text": "gamma"}),
            dir.path(),
            std::slice::from_ref(&snapshot),
        );
        assert_eq!(unique.status, "ok");
        assert_eq!(unique.structured.as_ref().unwrap()["replacement_count"], 1);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("README.md")).unwrap(),
            "alpha\ngamma\nalpha\n"
        );

        let changed = b"alpha\ngamma\nalpha\n";
        let second_snapshot = read_snapshot_event(
            18,
            dir.path(),
            "README.md",
            changed,
            true,
            "1\talpha\n2\tgamma\n3\talpha",
        );
        let all = edit_file(
            &serde_json::json!({"path": "README.md", "old_text": "alpha", "new_text": "omega", "replace_all": true}),
            dir.path(),
            &[second_snapshot],
        );
        assert_eq!(all.status, "ok");
        assert_eq!(all.structured.as_ref().unwrap()["replacement_count"], 2);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("README.md")).unwrap(),
            "omega\ngamma\nomega\n"
        );
    }

    #[test]
    fn write_file_creates_new_file_without_prior_read() {
        let dir = workspace();
        let result = write_file(
            &serde_json::json!({"path": "docs/new.md", "content": "hello\nworld\n"}),
            dir.path(),
            &[],
        );

        assert_eq!(result.status, "ok");
        assert_eq!(result.structured.as_ref().unwrap()["created"], true);
        assert_eq!(result.structured.as_ref().unwrap()["line_count"], 2);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("docs/new.md")).unwrap(),
            "hello\nworld\n"
        );
    }

    #[test]
    fn write_file_normalizes_new_file_paths_before_parent_check() {
        let dir = workspace();
        let result = write_file(
            &serde_json::json!({"path": "missing/../docs/normalized.md", "content": "normalized\n"}),
            dir.path(),
            &[],
        );

        assert_eq!(result.status, "ok");
        assert_eq!(
            result.structured.as_ref().unwrap()["path"],
            "docs/normalized.md"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("docs/normalized.md")).unwrap(),
            "normalized\n"
        );
    }

    #[test]
    fn write_file_requires_full_snapshot_for_overwrite_and_rejects_stale_snapshot() {
        let dir = workspace();
        let original = b"alpha\nbeta\n";
        std::fs::write(dir.path().join("README.md"), original).unwrap();
        let partial = read_snapshot_event(17, dir.path(), "README.md", original, false, "1\talpha");

        let partial_result = write_file(
            &serde_json::json!({"path": "README.md", "content": "replacement\n"}),
            dir.path(),
            &[partial],
        );
        assert_eq!(partial_result.status, "error");
        assert!(partial_result
            .model_content
            .contains("prior full read_file snapshot"));

        let full = read_snapshot_event(
            18,
            dir.path(),
            "README.md",
            original,
            true,
            "1\talpha\n2\tbeta",
        );
        std::fs::write(dir.path().join("README.md"), "changed\n").unwrap();
        let stale = write_file(
            &serde_json::json!({"path": "README.md", "content": "replacement\n"}),
            dir.path(),
            std::slice::from_ref(&full),
        );
        assert_eq!(stale.status, "error");
        assert!(stale.model_content.contains("read README.md again"));

        std::fs::write(dir.path().join("README.md"), original).unwrap();
        let ok = write_file(
            &serde_json::json!({"path": "README.md", "content": "replacement\n"}),
            dir.path(),
            &[full],
        );
        assert_eq!(ok.status, "ok");
        assert_eq!(ok.structured.as_ref().unwrap()["created"], false);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("README.md")).unwrap(),
            "replacement\n"
        );
    }

    #[test]
    fn write_tools_block_guarded_paths() {
        let dir = workspace();
        std::fs::write(dir.path().join(".env"), "TOKEN=secret").unwrap();

        let edit = edit_file(
            &serde_json::json!({"path": ".env", "old_text": "TOKEN", "new_text": "KEY"}),
            dir.path(),
            &[],
        );
        assert_eq!(edit.status, "blocked");

        let write = write_file(
            &serde_json::json!({"path": ".env.local", "content": "TOKEN=secret"}),
            dir.path(),
            &[],
        );
        assert_eq!(write.status, "blocked");
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
    fn search_text_filters_include_blocks_sensitive_paths_and_reports_invalid_regex() {
        let dir = workspace();
        std::fs::write(dir.path().join("README.md"), "TODO root\n").unwrap();
        std::fs::write(dir.path().join("docs/tools.md"), "TODO docs\n").unwrap();
        std::fs::write(dir.path().join(".env"), "TODO secret\n").unwrap();
        std::fs::write(dir.path().join("secret.pem"), "TODO secret\n").unwrap();

        let filtered = search_text(
            &serde_json::json!({"pattern": "TODO", "include": "docs/**/*.md"}),
            dir.path(),
        );
        assert_eq!(filtered.status, "ok");
        assert_eq!(filtered.model_content, "docs/tools.md");

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

    #[tokio::test(flavor = "current_thread")]
    async fn run_command_captures_stdout_from_workspace() {
        let dir = workspace();
        std::fs::write(dir.path().join("cwd-marker.txt"), "workspace marker\n").unwrap();
        let result = run_command(
            &serde_json::json!({"command": read_marker_command(), "timeout": 5}),
            dir.path(),
        )
        .await;

        assert_eq!(result.status, "ok");
        assert!(result.summary.contains("command exited 0"));
        assert!(result.model_content.contains("stdout:"));
        assert!(result.model_content.contains("workspace marker"));
        let structured = result.structured.unwrap();
        assert_eq!(structured["kind"], "command_result");
        assert_eq!(structured["exit_code"], 0);
        assert_eq!(structured["stdout_lines"], 1);
        assert_eq!(structured["stderr_lines"], 0);
        assert_eq!(structured["timed_out"], false);
        assert_eq!(structured["line_numbered"], false);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn run_command_treats_nonzero_exit_as_command_evidence() {
        let dir = workspace();
        let result = run_command(
            &serde_json::json!({"command": stderr_nonzero_command(), "timeout": 5}),
            dir.path(),
        )
        .await;

        assert_eq!(result.status, "ok");
        assert!(result.summary.contains("command exited 7"));
        assert!(result.model_content.contains("stderr:"));
        assert!(result.model_content.contains("bad"));
        assert_eq!(result.structured.as_ref().unwrap()["exit_code"], 7);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn run_command_reports_timeout_as_tool_error() {
        let dir = workspace();
        let result = run_command(
            &serde_json::json!({"command": sleep_command(2), "timeout": 1}),
            dir.path(),
        )
        .await;

        assert_eq!(result.status, "error");
        assert!(result.summary.contains("command timed out after 1s"));
        assert_eq!(
            result.structured.as_ref().unwrap()["kind"],
            "command_result"
        );
        assert_eq!(result.structured.as_ref().unwrap()["timed_out"], true);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn run_command_truncates_timeout_output() {
        let dir = workspace();
        let result = run_command(
            &serde_json::json!({"command": noisy_timeout_command(), "timeout": 1}),
            dir.path(),
        )
        .await;

        assert_eq!(result.status, "error");
        assert!(result.truncated);
        assert!(result.model_content.len() <= RUN_COMMAND_MAX_CHARS);
        assert!(result.model_content.contains("Command output truncated"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn run_command_rejects_invalid_timeout_empty_command_and_dangerous_commands() {
        let dir = workspace();

        let missing = run_command(&serde_json::json!({"timeout": 1}), dir.path()).await;
        assert_eq!(missing.status, "error");
        assert!(missing
            .model_content
            .contains("run_command requires command"));

        let empty = run_command(
            &serde_json::json!({"command": "   ", "timeout": 1}),
            dir.path(),
        )
        .await;
        assert_eq!(empty.status, "error");
        assert!(empty.model_content.contains("command must not be empty"));

        let zero = run_command(
            &serde_json::json!({"command": stdout_command(), "timeout": 0}),
            dir.path(),
        )
        .await;
        assert_eq!(zero.status, "error");
        assert!(zero.model_content.contains("timeout must be >= 1"));

        let over_cap = run_command(
            &serde_json::json!({"command": stdout_command(), "timeout": 601}),
            dir.path(),
        )
        .await;
        assert_eq!(over_cap.status, "error");
        assert!(over_cap.model_content.contains("timeout must be <= 600"));

        let dangerous = run_command(
            &serde_json::json!({"command": "git reset --hard HEAD", "timeout": 1}),
            dir.path(),
        )
        .await;
        assert_eq!(dangerous.status, "blocked");
        assert!(dangerous
            .model_content
            .contains("blocked by command hard guard"));
    }
}
