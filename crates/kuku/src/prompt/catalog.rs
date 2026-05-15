use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PromptAsset {
    pub(crate) path: &'static str,
    pub(crate) text: &'static str,
    pub(crate) hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PromptCatalog {
    pub(crate) system: PromptAsset,
    pub(crate) synthetic_user: PromptAsset,
    pub(crate) tool_guidance: PromptAsset,
}

pub(crate) fn builtin_prompt_catalog() -> PromptCatalog {
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

fn asset(path: &'static str, text: &'static str) -> PromptAsset {
    let digest = Sha256::digest(text.as_bytes());
    PromptAsset {
        path,
        text,
        hash: format!("sha256:{digest:x}"),
    }
}

#[cfg(test)]
mod tests {
    use super::builtin_prompt_catalog;

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
}
