use std::io;
use std::path::Path;

use serde_json::{Value, json};

use crate::tool::ToolResultEnvelope;

use super::{Cache, NAME, RESOURCE_ROOT, format::valid_path};

pub(crate) const TOOL_NAME: &str = "skill_resources";
const MAX_PAGE_ITEMS: usize = 20;
const MAX_READ_LINES: usize = 200;
const MAX_READ_BYTES: usize = 15_000;
const MAX_EXCERPT_BYTES: usize = 180;

pub(crate) fn definition() -> crate::tool::ToolDefinition {
    crate::tool::ToolDefinition {
        name: TOOL_NAME.into(),
        description: "List, search or read the bundled references for the kuku-docs skill after loading it in this Turn. Paths are relative to the skill, starting with references/docs. Returns the source version with each result.".into(),
        input_schema: json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "skill_name": {"type": "string", "enum": [NAME]},
                "action": {"type": "string", "enum": ["list", "search", "read"]},
                "path": {"type": "string", "description": "A path such as references/docs/en/index.md. Required for read. For list or search, omit for all docs or give a directory to narrow the scope."},
                "query": {"type": "string", "description": "Literal case-insensitive text to find on a line. Required for search."},
                "offset": {"type": "integer", "minimum": 0, "description": "Number of items to skip for list or search (default 0). Starting line for read (1-based, defaults to 1)."},
                "limit": {"type": "integer", "minimum": 1, "description": "Page size. List and search allow up to 20 items. Read allows up to 200 lines and may return fewer to bound the result."}
            },
            "required": ["skill_name", "action"]
        }),
        read_only: true,
        max_result_chars: 24_000,
        risk: "read".into(),
    }
}

enum Action {
    List,
    Search(String),
    Read,
}

pub(crate) struct Request {
    action: Action,
    relative: String,
    offset: usize,
    limit: usize,
}

impl Request {
    pub(crate) fn from_args(args: &Value) -> Result<Self, &'static str> {
        if args["skill_name"] != NAME {
            return Err("bundled resources are available for skill_name kuku-docs");
        }
        let action = match args["action"].as_str() {
            Some("list") => Action::List,
            Some("search") => Action::Search(
                args["query"]
                    .as_str()
                    .map(str::trim)
                    .filter(|query| !query.is_empty() && query.len() <= 256)
                    .ok_or("search requires a nonempty query of up to 256 bytes")?
                    .to_lowercase(),
            ),
            Some("read") => Action::Read,
            _ => return Err("skill resource action must be list, search, or read"),
        };
        let read = matches!(action, Action::Read);
        let path = match args.get("path") {
            Some(path) => path.as_str().ok_or("path must be a string")?,
            None if read => return Err("read requires a file path under references/docs/"),
            None => RESOURCE_ROOT,
        };
        let path = if read {
            path
        } else {
            path.strip_suffix('/').unwrap_or(path)
        };
        let relative = match path.strip_prefix(RESOURCE_ROOT) {
            Some("") if !read => "",
            Some(suffix) => suffix
                .strip_prefix('/')
                .filter(|path| valid_path(path))
                .ok_or(
                    "path must stay under references/docs/ and have no dot or parent components",
                )?,
            None => return Err("path must stay under references/docs/"),
        };
        let offset = integer(args, "offset", usize::from(read))?;
        if read && offset == 0 {
            return Err("read offset must be a 1-based line number");
        }
        let limit = integer(args, "limit", if read { 120 } else { 10 })?;
        if limit == 0 {
            return Err("limit must be positive");
        }
        Ok(Self {
            action,
            relative: relative.to_owned(),
            offset,
            limit: limit.min(if read { MAX_READ_LINES } else { MAX_PAGE_ITEMS }),
        })
    }

    pub(crate) fn execute(&self, home: &Path, version: &str) -> io::Result<ToolResultEnvelope> {
        super::with_cache(home, version, |cache| match &self.action {
            Action::List => Ok(self.list(cache, version)),
            Action::Search(query) => self.search(cache, version, query),
            Action::Read => self.read(cache, version),
        })
    }

    fn files<'a>(
        &'a self,
        cache: &'a Cache,
    ) -> impl Iterator<Item = (&'a String, &'a super::FileEntry)> {
        cache.manifest.files.iter().filter(|(path, _)| {
            self.relative.is_empty()
                || *path == &self.relative
                || path
                    .strip_prefix(&self.relative)
                    .is_some_and(|suffix| suffix.starts_with('/'))
        })
    }

    fn list(&self, cache: &Cache, version: &str) -> ToolResultEnvelope {
        let total = self.files(cache).count();
        if total == 0 {
            return unknown_path(&self.relative);
        }
        let files: Vec<_> = self
            .files(cache)
            .skip(self.offset)
            .take(self.limit)
            .map(|(path, entry)| json!({"path": resource_path(path), "bytes": entry.bytes}))
            .collect();
        let summary = format!("{} Skill resource files returned", files.len());
        let mut payload = details(cache, version, "list");
        payload["files"] = json!(files);
        payload["total"] = json!(total);
        payload["next_offset"] = json!(self.next_offset(files.len(), total));
        ToolResultEnvelope::ok(summary, payload.to_string(), payload)
    }

    fn search(&self, cache: &Cache, version: &str, query: &str) -> io::Result<ToolResultEnvelope> {
        let mut matches = Vec::new();
        let mut total = 0_usize;
        let mut found_path = false;
        for (path, entry) in self.files(cache) {
            found_path = true;
            let bytes = cache.file(path, entry)?;
            let Ok(text) = std::str::from_utf8(&bytes) else {
                continue;
            };
            for (index, line) in text.lines().enumerate() {
                if line.to_lowercase().contains(query) {
                    if total >= self.offset && matches.len() < self.limit {
                        let excerpt = line.trim();
                        matches.push(json!({
                            "path": resource_path(path),
                            "line": index + 1,
                            "excerpt": &excerpt[..excerpt.floor_char_boundary(MAX_EXCERPT_BYTES.min(excerpt.len()))],
                        }));
                    }
                    total += 1;
                }
            }
        }
        if !found_path {
            return Ok(unknown_path(&self.relative));
        }
        let summary = format!("{} Skill resource matches returned", matches.len());
        let mut payload = details(cache, version, "search");
        payload["matches"] = json!(matches);
        payload["total"] = json!(total);
        payload["next_offset"] = json!(self.next_offset(matches.len(), total));
        Ok(ToolResultEnvelope::ok(
            summary,
            payload.to_string(),
            payload,
        ))
    }

    fn read(&self, cache: &Cache, version: &str) -> io::Result<ToolResultEnvelope> {
        let Some(entry) = cache.manifest.files.get(&self.relative) else {
            return Ok(unknown_path(&self.relative));
        };
        let bytes = cache.file(&self.relative, entry)?;
        let Ok(text) = std::str::from_utf8(&bytes) else {
            return Ok(ToolResultEnvelope::error(
                "resource is not text",
                "read requires a UTF-8 text resource",
            ));
        };
        let total = text.lines().count();
        let mut view = String::new();
        let mut shown = 0;
        let mut clipped_line = false;
        for line in text.lines().skip(self.offset - 1).take(self.limit) {
            if view.len() + line.len() + 1 > MAX_READ_BYTES {
                if view.is_empty() {
                    view.push_str(&line[..line.floor_char_boundary(MAX_READ_BYTES - 1)]);
                    view.push('\n');
                    shown = 1;
                    clipped_line = true;
                }
                break;
            }
            view.push_str(line);
            view.push('\n');
            shown += 1;
        }
        let path = resource_path(&self.relative);
        let summary = format!("read Skill resource {path}");
        let end_line = self.offset.saturating_add(shown).saturating_sub(1);
        let next = self.next_offset(shown, total.saturating_add(1));
        let mut payload = details(cache, version, "read");
        payload["path"] = json!(path);
        payload["start_line"] = json!(self.offset);
        payload["line_count"] = json!(shown);
        payload["total_lines"] = json!(total);
        payload["next_offset"] = json!(next);
        let mut model_content = format!("{path} · bundle {version}\n");
        if !cache.manifest.source_revision.is_empty() {
            let url = format!(
                "{}/blob/{}/docs/{}",
                env!("CARGO_PKG_REPOSITORY"),
                cache.manifest.source_revision,
                self.relative
            );
            model_content.push_str(&format!("Source: {url}\n"));
            payload["source_url"] = json!(url);
        }
        if shown == 0 {
            model_content.push_str(&format!(
                "No lines at offset {} (total {total}).",
                self.offset
            ));
        } else {
            model_content.push_str(&format!("Lines {}–{end_line} of {total}.\n\n", self.offset));
            model_content.push_str(&view);
        }
        if clipped_line {
            model_content.push_str("\nThe line exceeds the result size and was clipped.\n");
        }
        if let Some(next) = next {
            model_content.push_str(&format!("\nContinue from offset {next}."));
        }
        Ok(ToolResultEnvelope::ok(summary, model_content, payload))
    }

    fn next_offset(&self, count: usize, end: usize) -> Option<usize> {
        let next = self.offset.saturating_add(count);
        (count > 0 && next < end).then_some(next)
    }
}

fn integer(args: &Value, key: &str, default: usize) -> Result<usize, &'static str> {
    match args.get(key) {
        None => Ok(default),
        Some(number) => number
            .as_u64()
            .and_then(|number| usize::try_from(number).ok())
            .ok_or("offset and limit must be nonnegative integers"),
    }
}

fn details(cache: &Cache, version: &str, action: &str) -> Value {
    json!({"kind": "skill_resources", "skill_name": NAME, "action": action,
        "version": version, "source_revision": cache.manifest.source_revision})
}

fn resource_path(relative: &str) -> String {
    format!("{RESOURCE_ROOT}/{relative}")
}

fn unknown_path(relative: &str) -> ToolResultEnvelope {
    ToolResultEnvelope::error(
        "Skill resource path not found",
        format!("No bundled resource at {}", resource_path(relative)),
    )
}
