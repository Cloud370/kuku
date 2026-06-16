use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptAsset {
    pub path: String,
    pub text: String,
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptCatalog {
    pub system: PromptAsset,
    /// block templates: "project-policy", "tool-guidance", "memory",
    /// "agent-catalog", "system-notice", "hook-context",
    /// "notice-agent-directory", "notice-open-conversations",
    /// "notice-inbox", "notice-loaded-skills", "notice-context-drift"
    pub blocks: BTreeMap<String, PromptAsset>,
    /// agent identity files: "main", "review", "explore", + user-defined
    pub agents: BTreeMap<String, PromptAsset>,
    /// memory data wrappers: "global", "project"
    pub memory: BTreeMap<String, PromptAsset>,
    /// runtime templates: "context", "handoff-context", "handoff-instruction", "notice-context-drift"
    pub runtime: BTreeMap<String, PromptAsset>,
    /// tool templates: "fetch-web"
    pub tools: BTreeMap<String, PromptAsset>,
}

impl PromptCatalog {
    /// Load prompt catalog from a directory, falling back to embedded for missing files.
    pub fn load_from_dir(dir: &Path) -> crate::error::Result<PromptCatalog> {
        let builtin = builtin_prompt_catalog();
        Ok(PromptCatalog {
            system: load_or_fallback(dir, "system.md", builtin.system.clone())?,
            blocks: load_subdir_map(dir, "blocks", &builtin.blocks)?,
            agents: load_subdir_map(dir, "agents", &builtin.agents)?,
            memory: load_subdir_map(dir, "memory", &builtin.memory)?,
            runtime: load_subdir_map(dir, "runtime", &builtin.runtime)?,
            tools: load_subdir_map(dir, "tools", &builtin.tools)?,
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
        "/prompts/runtime/handoff-instruction.md"
    ))
}

pub fn builtin_prompt_catalog() -> PromptCatalog {
    macro_rules! p {
        ($subdir:literal, $name:literal) => {
            asset(
                concat!("prompts/", $subdir, "/", $name, ".md"),
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/prompts/",
                    $subdir,
                    "/",
                    $name,
                    ".md"
                )),
            )
        };
    }
    PromptCatalog {
        system: asset(
            "prompts/system.md",
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/prompts/system.md")),
        ),
        blocks: {
            let mut m = BTreeMap::new();
            m.insert("project-policy".into(), p!("blocks", "project-policy"));
            m.insert("tool-guidance".into(), p!("blocks", "tool-guidance"));
            m.insert("memory".into(), p!("blocks", "memory"));
            m.insert("agent-catalog".into(), p!("blocks", "agent-catalog"));
            m.insert("system-notice".into(), p!("blocks", "system-notice"));
            m.insert("hook-context".into(), p!("blocks", "hook-context"));
            m.insert(
                "notice-agent-directory".into(),
                p!("blocks", "notice-agent-directory"),
            );
            m.insert(
                "notice-open-conversations".into(),
                p!("blocks", "notice-open-conversations"),
            );
            m.insert("notice-inbox".into(), p!("blocks", "notice-inbox"));
            m.insert(
                "notice-loaded-skills".into(),
                p!("blocks", "notice-loaded-skills"),
            );
            m.insert(
                "notice-context-drift".into(),
                p!("blocks", "notice-context-drift"),
            );
            m.insert("runtime-notices".into(), p!("blocks", "runtime-notices"));
            m.insert(
                "conversation-inbox".into(),
                p!("blocks", "conversation-inbox"),
            );
            m
        },
        agents: {
            let mut m = BTreeMap::new();
            m.insert("main".into(), p!("agents", "main"));
            m.insert("review".into(), p!("agents", "review"));
            m.insert("explore".into(), p!("agents", "explore"));
            m
        },
        memory: {
            let mut m = BTreeMap::new();
            m.insert("global".into(), p!("memory", "global"));
            m.insert("project".into(), p!("memory", "project"));
            m
        },
        runtime: {
            let mut m = BTreeMap::new();
            m.insert("context".into(), p!("runtime", "context"));
            m.insert("handoff-context".into(), p!("runtime", "handoff-context"));
            m.insert(
                "handoff-instruction".into(),
                p!("runtime", "handoff-instruction"),
            );
            m.insert(
                "notice-context-drift".into(),
                p!("runtime", "notice-context-drift"),
            );
            m
        },
        tools: {
            let mut m = BTreeMap::new();
            m.insert("fetch-web".into(), p!("tools", "fetch-web"));
            m
        },
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

fn load_subdir_map(
    base: &Path,
    subdir: &str,
    builtins: &BTreeMap<String, PromptAsset>,
) -> crate::error::Result<BTreeMap<String, PromptAsset>> {
    let dir = base.join(subdir);
    let mut map = builtins.clone();
    if dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("md") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        let text = std::fs::read_to_string(&path).map_err(|e| {
                            crate::error::Error::PromptRender(format!(
                                "failed to read {}: {e}",
                                path.display()
                            ))
                        })?;
                        let hash = format!("sha256:{:x}", Sha256::digest(text.as_bytes()));
                        map.insert(
                            stem.to_string(),
                            PromptAsset {
                                path: path.to_string_lossy().to_string(),
                                text,
                                hash,
                            },
                        );
                    }
                }
            }
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::{builtin_prompt_catalog, PromptCatalog};

    #[test]
    fn loads_required_sdk_core_prompt_assets() {
        let catalog = builtin_prompt_catalog();

        assert!(catalog.system.path.ends_with("system.md"));
        assert!(catalog.blocks["project-policy"]
            .path
            .ends_with("project-policy.md"));
        assert!(catalog.blocks["tool-guidance"]
            .path
            .ends_with("tool-guidance.md"));
        assert!(catalog.runtime["context"].path.ends_with("context.md"));
        assert!(catalog.memory["global"].path.ends_with("global.md"));
        assert!(catalog.memory["project"].path.ends_with("project.md"));
        assert!(!catalog.system.text.trim().is_empty());
        assert!(!catalog.blocks["project-policy"].text.trim().is_empty());
        assert!(!catalog.blocks["tool-guidance"].text.trim().is_empty());
        assert!(!catalog.memory["global"].text.trim().is_empty());
        assert!(!catalog.memory["project"].text.trim().is_empty());
        assert!(catalog.tools["fetch-web"].path.ends_with("fetch-web.md"));
        assert!(!catalog.tools["fetch-web"].text.trim().is_empty());
        assert!(catalog.runtime["handoff-context"]
            .path
            .ends_with("handoff-context.md"));
        assert!(!catalog.runtime["handoff-context"].text.trim().is_empty());
        assert!(catalog.runtime["handoff-context"]
            .text
            .contains("{{handoff_summary}}"));
        assert!(catalog.system.hash.starts_with("sha256:"));
        assert!(catalog.memory["global"].hash.starts_with("sha256:"));
        assert!(catalog.memory["project"].hash.starts_with("sha256:"));
    }

    #[test]
    fn load_from_dir_uses_external_file_when_present() {
        let dir = tempfile::tempdir().unwrap();
        let custom_system = "<kuku_identity>Custom identity</kuku_identity>";
        std::fs::write(dir.path().join("system.md"), custom_system).unwrap();

        let catalog = PromptCatalog::load_from_dir(dir.path()).unwrap();
        assert_eq!(catalog.system.text, custom_system);
        assert!(catalog.blocks["project-policy"]
            .text
            .contains("<kuku_project_context>"));
    }
}
