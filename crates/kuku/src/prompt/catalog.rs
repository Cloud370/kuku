use std::path::Path;

use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptAsset {
    pub path: String,
    pub text: String,
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptCatalog {
    pub system: PromptAsset,
    pub project_context: PromptAsset,
    pub tool_guidance: PromptAsset,
    pub runtime_context: PromptAsset,
    pub global_memory: PromptAsset,
    pub project_memory: PromptAsset,
    pub fetch_web: PromptAsset,
}

impl PromptCatalog {
    /// Load prompt catalog from a directory, falling back to embedded for missing files.
    pub fn load_from_dir(dir: &Path) -> crate::error::Result<PromptCatalog> {
        let builtin = builtin_prompt_catalog();
        Ok(PromptCatalog {
            system: load_or_fallback(dir, "system.md", builtin.system)?,
            project_context: load_or_fallback(dir, "project-context.md", builtin.project_context)?,
            tool_guidance: load_or_fallback(dir, "tool-guidance.md", builtin.tool_guidance)?,
            runtime_context: load_or_fallback(dir, "runtime-context.md", builtin.runtime_context)?,
            global_memory: load_or_fallback(dir, "global-memory.md", builtin.global_memory)?,
            project_memory: load_or_fallback(dir, "project-memory.md", builtin.project_memory)?,
            fetch_web: load_or_fallback(dir, "fetch-web.md", builtin.fetch_web)?,
        })
    }
}

pub fn load_prompt_template(dir: &Path, name: &str) -> crate::error::Result<String> {
    let path = dir.join(format!("{name}.md"));
    std::fs::read_to_string(&path).map_err(|e| {
        crate::error::Error::PromptRender(format!(
            "failed to load prompt template {}: {e}",
            path.display()
        ))
    })
}

pub fn builtin_handoff_instruction() -> &'static str {
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/prompts/handoff_instruction.md"
    ))
}

pub fn builtin_session_query_guidance() -> &'static str {
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/prompts/session_query_guidance.md"
    ))
}

pub fn builtin_prompt_catalog() -> PromptCatalog {
    PromptCatalog {
        system: asset(
            "crates/kuku/prompts/system.md",
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/prompts/system.md")),
        ),
        project_context: asset(
            "crates/kuku/prompts/project-context.md",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/prompts/project-context.md"
            )),
        ),
        tool_guidance: asset(
            "crates/kuku/prompts/tool-guidance.md",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/prompts/tool-guidance.md"
            )),
        ),
        runtime_context: asset(
            "crates/kuku/prompts/runtime-context.md",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/prompts/runtime-context.md"
            )),
        ),
        global_memory: asset(
            "crates/kuku/prompts/global-memory.md",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/prompts/global-memory.md"
            )),
        ),
        project_memory: asset(
            "crates/kuku/prompts/project-memory.md",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/prompts/project-memory.md"
            )),
        ),
        fetch_web: asset(
            "crates/kuku/prompts/fetch-web.md",
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/prompts/fetch-web.md")),
        ),
    }
}

fn asset(path: &str, text: &str) -> PromptAsset {
    let digest = Sha256::digest(text.as_bytes());
    PromptAsset {
        path: path.to_string(),
        text: text.to_string(),
        hash: format!("sha256:{digest:x}"),
    }
}

fn load_or_fallback(
    dir: &Path,
    filename: &str,
    fallback: PromptAsset,
) -> crate::error::Result<PromptAsset> {
    let path = dir.join(filename);
    if path.exists() {
        let text = std::fs::read_to_string(&path)?;
        let digest = Sha256::digest(text.as_bytes());
        Ok(PromptAsset {
            path: path.to_string_lossy().into_owned(),
            text,
            hash: format!("sha256:{digest:x}"),
        })
    } else {
        Ok(fallback)
    }
}

#[cfg(test)]
mod tests {
    use super::{builtin_prompt_catalog, PromptCatalog};

    #[test]
    fn loads_required_sdk_core_prompt_assets() {
        let catalog = builtin_prompt_catalog();

        assert!(catalog.system.path.ends_with("system.md"));
        assert!(catalog.project_context.path.ends_with("project-context.md"));
        assert!(catalog.tool_guidance.path.ends_with("tool-guidance.md"));
        assert!(catalog.runtime_context.path.ends_with("runtime-context.md"));
        assert!(catalog.global_memory.path.ends_with("global-memory.md"));
        assert!(catalog.project_memory.path.ends_with("project-memory.md"));
        assert!(!catalog.system.text.trim().is_empty());
        assert!(!catalog.project_context.text.trim().is_empty());
        assert!(!catalog.tool_guidance.text.trim().is_empty());
        assert!(!catalog.global_memory.text.trim().is_empty());
        assert!(!catalog.project_memory.text.trim().is_empty());
        assert!(catalog.fetch_web.path.ends_with("fetch-web.md"));
        assert!(!catalog.fetch_web.text.trim().is_empty());
        assert!(catalog.system.hash.starts_with("sha256:"));
        assert!(catalog.global_memory.hash.starts_with("sha256:"));
        assert!(catalog.project_memory.hash.starts_with("sha256:"));
    }

    #[test]
    fn load_from_dir_uses_external_file_when_present() {
        let dir = tempfile::tempdir().unwrap();
        let custom_system = "<kuku_identity>Custom identity</kuku_identity>";
        std::fs::write(dir.path().join("system.md"), custom_system).unwrap();

        let catalog = PromptCatalog::load_from_dir(dir.path()).unwrap();
        assert_eq!(catalog.system.text, custom_system);
        assert!(catalog
            .project_context
            .text
            .contains("<kuku_project_context>"));
    }
}
