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
    pub synthetic_user: PromptAsset,
    pub tool_guidance: PromptAsset,
}

impl PromptCatalog {
    /// Load prompt catalog from a directory, falling back to embedded for missing files.
    /// External files override embedded prompts when present.
    pub fn load_from_dir(dir: &Path) -> crate::error::Result<PromptCatalog> {
        let builtin = builtin_prompt_catalog();
        Ok(PromptCatalog {
            system: load_or_fallback(dir, "system.md", builtin.system)?,
            synthetic_user: load_or_fallback(dir, "synthetic-user.md", builtin.synthetic_user)?,
            tool_guidance: load_or_fallback(dir, "tool-guidance.md", builtin.tool_guidance)?,
        })
    }
}

pub fn builtin_prompt_catalog() -> PromptCatalog {
    PromptCatalog {
        system: asset(
            "crates/kuku/prompts/system.md",
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/prompts/system.md")),
        ),
        synthetic_user: asset(
            "crates/kuku/prompts/synthetic-user.md",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/prompts/synthetic-user.md"
            )),
        ),
        tool_guidance: asset(
            "crates/kuku/prompts/tool-guidance.md",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/prompts/tool-guidance.md"
            )),
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

fn load_or_fallback(dir: &Path, filename: &str, fallback: PromptAsset) -> crate::error::Result<PromptAsset> {
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

        assert!(catalog
            .system
            .path
            .ends_with("crates/kuku/prompts/system.md"));
        assert!(catalog
            .synthetic_user
            .path
            .ends_with("crates/kuku/prompts/synthetic-user.md"));
        assert!(catalog
            .tool_guidance
            .path
            .ends_with("crates/kuku/prompts/tool-guidance.md"));
        assert!(!catalog.system.text.trim().is_empty());
        assert!(!catalog.synthetic_user.text.trim().is_empty());
        assert!(!catalog.tool_guidance.text.trim().is_empty());
        assert!(catalog.system.hash.starts_with("sha256:"));
    }

    #[test]
    fn load_from_dir_uses_external_file_when_present() {
        let dir = tempfile::tempdir().unwrap();
        let custom_system = "<kuku_identity>Custom identity</kuku_identity>";
        std::fs::write(dir.path().join("system.md"), custom_system).unwrap();

        let catalog = PromptCatalog::load_from_dir(dir.path()).unwrap();
        assert_eq!(catalog.system.text, custom_system);
        assert!(catalog.system.path.contains("system.md"));
        assert!(catalog.synthetic_user.text.contains("<kuku_execution_context>"));
        assert!(catalog.tool_guidance.text.contains("<kuku_tool_guidance>"));
    }

    #[test]
    fn load_from_dir_uses_all_embedded_when_dir_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let builtin = builtin_prompt_catalog();
        let catalog = PromptCatalog::load_from_dir(dir.path()).unwrap();
        assert_eq!(catalog.system.text, builtin.system.text);
        assert_eq!(catalog.synthetic_user.text, builtin.synthetic_user.text);
        assert_eq!(catalog.tool_guidance.text, builtin.tool_guidance.text);
    }

    #[test]
    fn load_from_dir_preserves_hash_for_external_files() {
        let dir = tempfile::tempdir().unwrap();
        let custom = "custom system content";
        std::fs::write(dir.path().join("system.md"), custom).unwrap();

        let catalog = PromptCatalog::load_from_dir(dir.path()).unwrap();
        assert!(catalog.system.hash.starts_with("sha256:"));
        assert_ne!(catalog.system.hash, builtin_prompt_catalog().system.hash);
    }
}
