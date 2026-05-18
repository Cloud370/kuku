use kuku::prompt::{builtin_prompt_catalog, PromptCatalog};

#[test]
fn load_from_dir_uses_external_system_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let custom = "<kuku_identity>Custom identity</kuku_identity>";
    std::fs::write(dir.path().join("system.md"), custom).unwrap();

    let catalog = PromptCatalog::load_from_dir(dir.path()).unwrap();
    assert_eq!(catalog.system.text, custom);
    // Other prompts fall back to embedded
    assert_eq!(
        catalog.project_context.text,
        builtin_prompt_catalog().project_context.text
    );
    assert_eq!(
        catalog.tool_guidance.text,
        builtin_prompt_catalog().tool_guidance.text
    );
}

#[test]
fn load_from_dir_all_embedded_when_dir_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let builtin = builtin_prompt_catalog();
    let catalog = PromptCatalog::load_from_dir(dir.path()).unwrap();
    assert_eq!(catalog.system.text, builtin.system.text);
    assert_eq!(catalog.project_context.text, builtin.project_context.text);
    assert_eq!(catalog.tool_guidance.text, builtin.tool_guidance.text);
}
