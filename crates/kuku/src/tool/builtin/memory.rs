use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::session::{global_memory_path, project_memory_path};
use crate::tool::ToolResultEnvelope;

use super::common::{content_hash, write_atomically};

struct MemoryRememberRequest {
    scope: MemoryScope,
    section: MemorySection,
    text: String,
}

struct MemoryForgetRequest {
    scope: MemoryScope,
    text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemoryScope {
    Global,
    Project,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemorySection {
    HowToWork,
    WhatIsTrue,
    WhereToLook,
}

struct MemoryFile {
    prelude: String,
    how_to_work: Vec<String>,
    what_is_true: Vec<String>,
    where_to_look: Vec<String>,
}

pub(crate) fn memory_remember_with_home(
    args: &Value,
    workspace: &Path,
    kuku_home: &Path,
) -> ToolResultEnvelope {
    let request = match memory_remember_request(args) {
        Ok(request) => request,
        Err(result) => return result,
    };
    let memory_path = match resolve_memory_path(kuku_home, request.scope, workspace) {
        Ok(path) => path,
        Err(result) => return result,
    };
    let mut memory = match load_memory_file(&memory_path) {
        Ok(memory) => memory,
        Err(result) => return result,
    };
    memory
        .section_mut(request.section)
        .push(request.text.clone());

    let raw_text_after = memory.render();
    if let Err(error) = write_memory_file(&memory_path, &raw_text_after) {
        return ToolResultEnvelope::error(
            format!("failed: {error}"),
            format!("error writing memory file: {}", memory_path.display()),
        );
    }

    memory_success_result(
        "memory_write",
        request.scope,
        Some(request.section),
        &memory_path,
        raw_text_after,
    )
}

pub(crate) fn memory_forget_with_home(
    args: &Value,
    workspace: &Path,
    kuku_home: &Path,
) -> ToolResultEnvelope {
    let request = match memory_forget_request(args) {
        Ok(request) => request,
        Err(result) => return result,
    };
    let memory_path = match resolve_memory_path(kuku_home, request.scope, workspace) {
        Ok(path) => path,
        Err(result) => return result,
    };
    let mut memory = match load_memory_file(&memory_path) {
        Ok(memory) => memory,
        Err(result) => return result,
    };

    let matches = memory.matching_sections(&request.text);
    if matches.is_empty() {
        return ToolResultEnvelope::error(
            "failed: no matching bullet".to_string(),
            "no matching bullet found in memory".to_string(),
        );
    }
    if matches.len() > 1 {
        return ToolResultEnvelope::error(
            "failed: text matched multiple bullets".to_string(),
            "text matched multiple bullets; forget requires exactly one match".to_string(),
        );
    }

    let section = matches[0];
    memory.remove_one(section, &request.text);
    let raw_text_after = memory.render();
    if let Err(error) = write_memory_file(&memory_path, &raw_text_after) {
        return ToolResultEnvelope::error(
            format!("failed: {error}"),
            format!("error writing memory file: {}", memory_path.display()),
        );
    }

    memory_success_result(
        "memory_forget",
        request.scope,
        Some(section),
        &memory_path,
        raw_text_after,
    )
}

fn memory_remember_request(args: &Value) -> Result<MemoryRememberRequest, ToolResultEnvelope> {
    Ok(MemoryRememberRequest {
        scope: memory_scope(args.get("scope"))?,
        section: memory_section(args.get("kind"))?,
        text: required_memory_text(args.get("text"))?,
    })
}

fn memory_forget_request(args: &Value) -> Result<MemoryForgetRequest, ToolResultEnvelope> {
    Ok(MemoryForgetRequest {
        scope: memory_scope(args.get("scope"))?,
        text: required_memory_text(args.get("text"))?,
    })
}

fn memory_scope(value: Option<&Value>) -> Result<MemoryScope, ToolResultEnvelope> {
    match value.and_then(Value::as_str).map(str::trim) {
        Some("global") => Ok(MemoryScope::Global),
        Some("project") => Ok(MemoryScope::Project),
        Some(other) => Err(ToolResultEnvelope::error(
            format!("failed: invalid scope: {other}"),
            "scope must be one of: global, project".to_string(),
        )),
        None => Err(ToolResultEnvelope::error(
            "failed: missing scope",
            "memory tool requires scope",
        )),
    }
}

fn memory_section(value: Option<&Value>) -> Result<MemorySection, ToolResultEnvelope> {
    match value.and_then(Value::as_str).map(str::trim) {
        Some("how_to_work") => Ok(MemorySection::HowToWork),
        Some("what_is_true") => Ok(MemorySection::WhatIsTrue),
        Some("where_to_look") => Ok(MemorySection::WhereToLook),
        Some(other) => Err(ToolResultEnvelope::error(
            format!("failed: invalid kind: {other}"),
            "kind must be one of: how_to_work, what_is_true, where_to_look".to_string(),
        )),
        None => Err(ToolResultEnvelope::error(
            "failed: missing kind",
            "memory.remember requires kind",
        )),
    }
}

fn required_memory_text(value: Option<&Value>) -> Result<String, ToolResultEnvelope> {
    let Some(text) = value.and_then(Value::as_str) else {
        return Err(ToolResultEnvelope::error(
            "failed: missing text",
            "memory tool requires text",
        ));
    };
    let text = text.trim();
    if text.is_empty() {
        return Err(ToolResultEnvelope::error(
            "failed: text is empty",
            "text must not be empty",
        ));
    }
    if text.contains('\n') || text.contains('\r') {
        return Err(ToolResultEnvelope::error(
            "failed: text must not contain line breaks",
            "text must not contain line breaks",
        ));
    }
    if text.starts_with('-') {
        return Err(ToolResultEnvelope::error(
            "failed: text must not start with '-'",
            "text should be natural language without a bullet prefix",
        ));
    }
    Ok(text.to_string())
}

fn resolve_memory_path(
    kuku_home: &Path,
    scope: MemoryScope,
    workspace: &Path,
) -> Result<PathBuf, ToolResultEnvelope> {
    let workspace = workspace.canonicalize().map_err(|_| {
        ToolResultEnvelope::error(
            "failed: workspace not found",
            "workspace path does not exist",
        )
    })?;
    let path = match scope {
        MemoryScope::Global => global_memory_path(kuku_home),
        MemoryScope::Project => project_memory_path(kuku_home, &workspace).map_err(|error| {
            ToolResultEnvelope::error(format!("failed: {error}"), error.to_string())
        })?,
    };
    Ok(path)
}

fn load_memory_file(path: &Path) -> Result<MemoryFile, ToolResultEnvelope> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(parse_memory_file(&content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(MemoryFile::default()),
        Err(error) => Err(ToolResultEnvelope::error(
            format!("failed: {error}"),
            format!("error reading memory file: {}", path.display()),
        )),
    }
}

fn write_memory_file(path: &Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    write_atomically(path, content.as_bytes())
}

fn parse_memory_file(content: &str) -> MemoryFile {
    const HOW_TO_WORK: &str = "## how_to_work";
    const WHAT_IS_TRUE: &str = "## what_is_true";
    const WHERE_TO_LOOK: &str = "## where_to_look";

    let mut memory = MemoryFile::default();
    let first_header = [HOW_TO_WORK, WHAT_IS_TRUE, WHERE_TO_LOOK]
        .iter()
        .filter_map(|header| content.find(header))
        .min();

    if let Some(index) = first_header {
        memory.prelude = content[..index].trim_end().to_string();
    } else {
        memory.prelude = content.trim().to_string();
        return memory;
    }

    let mut section = None;
    for line in content[first_header.unwrap()..].lines() {
        match line.trim() {
            HOW_TO_WORK => {
                section = Some(MemorySection::HowToWork);
                continue;
            }
            WHAT_IS_TRUE => {
                section = Some(MemorySection::WhatIsTrue);
                continue;
            }
            WHERE_TO_LOOK => {
                section = Some(MemorySection::WhereToLook);
                continue;
            }
            _ => {}
        }
        if let Some(text) = line.trim().strip_prefix("- ") {
            if let Some(section) = section {
                memory.section_mut(section).push(text.to_string());
            }
        }
    }

    memory
}

fn memory_success_result(
    kind: &str,
    scope: MemoryScope,
    section: Option<MemorySection>,
    path: &Path,
    raw_text_after: String,
) -> ToolResultEnvelope {
    let content_hash_after = content_hash(raw_text_after.as_bytes());
    let canonical_path = fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned();
    let mut structured = serde_json::json!({
        "kind": kind,
        "scope": scope.as_str(),
        "canonical_path": canonical_path,
        "content_hash_after": content_hash_after,
        "raw_text_after": raw_text_after,
    });
    if let Some(section) = section {
        structured["section"] = Value::String(section.as_str().to_string());
    }
    let summary = format!("updated {} memory", scope.as_str());
    ToolResultEnvelope::ok(summary.clone(), summary, structured)
}

impl Default for MemoryFile {
    fn default() -> Self {
        Self {
            prelude: "# memory".to_string(),
            how_to_work: Vec::new(),
            what_is_true: Vec::new(),
            where_to_look: Vec::new(),
        }
    }
}

impl MemoryFile {
    fn section_mut(&mut self, section: MemorySection) -> &mut Vec<String> {
        match section {
            MemorySection::HowToWork => &mut self.how_to_work,
            MemorySection::WhatIsTrue => &mut self.what_is_true,
            MemorySection::WhereToLook => &mut self.where_to_look,
        }
    }

    fn matching_sections(&self, text: &str) -> Vec<MemorySection> {
        let mut matches = Vec::new();
        for section in [
            MemorySection::HowToWork,
            MemorySection::WhatIsTrue,
            MemorySection::WhereToLook,
        ] {
            for entry in self.section(section) {
                if entry == text {
                    matches.push(section);
                }
            }
        }
        matches
    }

    fn section(&self, section: MemorySection) -> &[String] {
        match section {
            MemorySection::HowToWork => &self.how_to_work,
            MemorySection::WhatIsTrue => &self.what_is_true,
            MemorySection::WhereToLook => &self.where_to_look,
        }
    }

    fn remove_one(&mut self, section: MemorySection, text: &str) {
        if let Some(index) = self.section(section).iter().position(|entry| entry == text) {
            self.section_mut(section).remove(index);
        }
    }

    fn render(&self) -> String {
        let mut out = String::new();
        if !self.prelude.trim().is_empty() {
            out.push_str(self.prelude.trim_end());
            out.push_str("\n\n");
        }
        render_memory_section(&mut out, MemorySection::HowToWork, &self.how_to_work);
        out.push_str("\n\n");
        render_memory_section(&mut out, MemorySection::WhatIsTrue, &self.what_is_true);
        out.push_str("\n\n");
        render_memory_section(&mut out, MemorySection::WhereToLook, &self.where_to_look);
        out.push('\n');
        out
    }
}

impl MemoryScope {
    fn as_str(self) -> &'static str {
        match self {
            MemoryScope::Global => "global",
            MemoryScope::Project => "project",
        }
    }
}

impl MemorySection {
    fn as_str(self) -> &'static str {
        match self {
            MemorySection::HowToWork => "how_to_work",
            MemorySection::WhatIsTrue => "what_is_true",
            MemorySection::WhereToLook => "where_to_look",
        }
    }

    fn heading(self) -> &'static str {
        match self {
            MemorySection::HowToWork => "## how_to_work",
            MemorySection::WhatIsTrue => "## what_is_true",
            MemorySection::WhereToLook => "## where_to_look",
        }
    }
}

fn render_memory_section(out: &mut String, section: MemorySection, entries: &[String]) {
    out.push_str(section.heading());
    for entry in entries {
        out.push_str("\n- ");
        out.push_str(entry);
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_helpers::workspace;
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn with_kuku_home<T>(home: &Path, f: impl FnOnce() -> T) -> T {
        let _guard = env_lock().lock().unwrap();
        let previous = std::env::var_os("KUKU_HOME");
        std::env::set_var("KUKU_HOME", home);
        let result = f();
        match previous {
            Some(value) => std::env::set_var("KUKU_HOME", value),
            None => std::env::remove_var("KUKU_HOME"),
        }
        result
    }

    #[test]
    fn memory_remember_creates_expected_memory_file_and_appends_bullet() {
        let dir = workspace();
        let home = tempfile::tempdir().unwrap();

        let result = with_kuku_home(home.path(), || {
            memory_remember_with_home(
                &serde_json::json!({
                    "scope": "project",
                    "kind": "how_to_work",
                    "text": "Keep answers concise"
                }),
                dir.path(),
                home.path(),
            )
        });

        assert_eq!(result.status, "ok");
        assert_eq!(result.structured.as_ref().unwrap()["kind"], "memory_write");
        assert_eq!(result.structured.as_ref().unwrap()["scope"], "project");
        assert_eq!(
            result.structured.as_ref().unwrap()["section"],
            "how_to_work"
        );
        assert!(result.structured.as_ref().unwrap()["canonical_path"]
            .as_str()
            .unwrap()
            .ends_with("memory.md"));
        assert!(result.structured.as_ref().unwrap()["content_hash_after"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
        let raw = result.structured.as_ref().unwrap()["raw_text_after"]
            .as_str()
            .unwrap();
        assert!(raw.contains("# memory"));
        assert!(raw.contains("## how_to_work"));
        assert!(raw.contains("- Keep answers concise"));
        assert!(raw.contains("## what_is_true"));
        assert!(raw.contains("## where_to_look"));

        let project_memory = crate::session::project_memory_path(
            home.path(),
            &std::fs::canonicalize(dir.path()).unwrap(),
        )
        .unwrap();
        assert_eq!(std::fs::read_to_string(project_memory).unwrap(), raw);
    }

    #[test]
    fn memory_remember_and_forget_only_touch_selected_scope() {
        let dir = workspace();
        let home = tempfile::tempdir().unwrap();
        let workspace_path = std::fs::canonicalize(dir.path()).unwrap();
        let global_path = crate::session::global_memory_path(home.path());
        let project_path =
            crate::session::project_memory_path(home.path(), &workspace_path).unwrap();
        std::fs::create_dir_all(project_path.parent().unwrap()).unwrap();
        std::fs::write(
            &global_path,
            "# memory\n\n## how_to_work\n- Global only\n\n## what_is_true\n\n## where_to_look\n",
        )
        .unwrap();
        std::fs::write(
            &project_path,
            "# memory\n\n## how_to_work\n- Project only\n\n## what_is_true\n\n## where_to_look\n",
        )
        .unwrap();

        let remember = with_kuku_home(home.path(), || {
            memory_remember_with_home(
                &serde_json::json!({
                    "scope": "global",
                    "kind": "what_is_true",
                    "text": "The user prefers concise answers"
                }),
                dir.path(),
                home.path(),
            )
        });
        assert_eq!(remember.status, "ok");
        assert!(std::fs::read_to_string(&global_path)
            .unwrap()
            .contains("- The user prefers concise answers"));
        assert_eq!(
            std::fs::read_to_string(&project_path).unwrap(),
            "# memory\n\n## how_to_work\n- Project only\n\n## what_is_true\n\n## where_to_look\n"
        );

        let forget = with_kuku_home(home.path(), || {
            memory_forget_with_home(
                &serde_json::json!({
                    "scope": "project",
                    "text": "Project only"
                }),
                dir.path(),
                home.path(),
            )
        });
        assert_eq!(forget.status, "ok");
        assert_eq!(forget.structured.as_ref().unwrap()["kind"], "memory_forget");
        assert_eq!(forget.structured.as_ref().unwrap()["scope"], "project");
        assert_eq!(
            forget.structured.as_ref().unwrap()["section"],
            "how_to_work"
        );
        assert!(!std::fs::read_to_string(&project_path)
            .unwrap()
            .contains("Project only"));
        assert!(std::fs::read_to_string(&global_path)
            .unwrap()
            .contains("Global only"));
    }

    #[test]
    fn memory_forget_requires_exactly_one_matching_bullet() {
        let dir = workspace();
        let home = tempfile::tempdir().unwrap();
        let workspace_path = std::fs::canonicalize(dir.path()).unwrap();
        let project_path =
            crate::session::project_memory_path(home.path(), &workspace_path).unwrap();
        std::fs::create_dir_all(project_path.parent().unwrap()).unwrap();

        // Cross-section duplicates
        std::fs::write(
            &project_path,
            "# memory\n\n## how_to_work\n- Duplicate\n\n## what_is_true\n- Duplicate\n\n## where_to_look\n- Somewhere else\n",
        )
        .unwrap();
        let cross_section = with_kuku_home(home.path(), || {
            memory_forget_with_home(
                &serde_json::json!({"scope": "project", "text": "Duplicate"}),
                dir.path(),
                home.path(),
            )
        });
        assert_eq!(cross_section.status, "error");
        assert!(cross_section.model_content.contains("matched multiple"));

        // Same-section duplicates
        let same_section_content = "# memory\n\n## how_to_work\n- Duplicate\n- Duplicate\n\n## what_is_true\n\n## where_to_look\n";
        std::fs::write(&project_path, same_section_content).unwrap();
        let same_section = with_kuku_home(home.path(), || {
            memory_forget_with_home(
                &serde_json::json!({"scope": "project", "text": "Duplicate"}),
                dir.path(),
                home.path(),
            )
        });
        assert_eq!(same_section.status, "error");
        assert!(same_section
            .model_content
            .contains("forget requires exactly one match"));
        assert_eq!(
            std::fs::read_to_string(&project_path).unwrap(),
            same_section_content
        );

        // Zero matches
        let zero = with_kuku_home(home.path(), || {
            memory_forget_with_home(
                &serde_json::json!({"scope": "project", "text": "Missing"}),
                dir.path(),
                home.path(),
            )
        });
        assert_eq!(zero.status, "error");
        assert!(zero.model_content.contains("no matching bullet"));
    }

    #[test]
    fn memory_tools_reject_invalid_scope_kind_and_text_without_panicking() {
        let dir = workspace();
        let home = tempfile::tempdir().unwrap();

        let invalid_scope = with_kuku_home(home.path(), || {
            memory_remember_with_home(
                &serde_json::json!({
                    "scope": "session",
                    "kind": "how_to_work",
                    "text": "Keep answers concise"
                }),
                dir.path(),
                home.path(),
            )
        });
        assert_eq!(invalid_scope.status, "error");
        assert_eq!(invalid_scope.structured.as_ref().unwrap()["kind"], "error");

        let invalid_kind = with_kuku_home(home.path(), || {
            memory_remember_with_home(
                &serde_json::json!({
                    "scope": "project",
                    "kind": "preferences",
                    "text": "Keep answers concise"
                }),
                dir.path(),
                home.path(),
            )
        });
        assert_eq!(invalid_kind.status, "error");
        assert_eq!(invalid_kind.structured.as_ref().unwrap()["kind"], "error");

        let invalid_text = with_kuku_home(home.path(), || {
            memory_forget_with_home(
                &serde_json::json!({"scope": "project", "text": "   "}),
                dir.path(),
                home.path(),
            )
        });
        assert_eq!(invalid_text.status, "error");
        assert_eq!(invalid_text.structured.as_ref().unwrap()["kind"], "error");
    }

    #[test]
    fn memory_remember_rejects_text_with_embedded_line_breaks() {
        let dir = workspace();
        let home = tempfile::tempdir().unwrap();

        let result = with_kuku_home(home.path(), || {
            memory_remember_with_home(
                &serde_json::json!({
                    "scope": "project",
                    "kind": "how_to_work",
                    "text": "Keep answers\nconcise"
                }),
                dir.path(),
                home.path(),
            )
        });

        assert_eq!(result.status, "error");
        assert!(result.model_content.contains("line breaks"));
        let workspace_path = std::fs::canonicalize(dir.path()).unwrap();
        let project_memory =
            crate::session::project_memory_path(home.path(), &workspace_path).unwrap();
        assert!(!project_memory.exists());
    }
}
