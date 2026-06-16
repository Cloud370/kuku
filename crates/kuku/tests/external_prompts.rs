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
        catalog.blocks["project-policy"].text,
        builtin_prompt_catalog().blocks["project-policy"].text
    );
    assert_eq!(
        catalog.blocks["tool-guidance"].text,
        builtin_prompt_catalog().blocks["tool-guidance"].text
    );
}

#[test]
fn load_from_dir_all_embedded_when_dir_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let builtin = builtin_prompt_catalog();
    let catalog = PromptCatalog::load_from_dir(dir.path()).unwrap();
    assert_eq!(catalog.system.text, builtin.system.text);
    assert_eq!(
        catalog.blocks["project-policy"].text,
        builtin.blocks["project-policy"].text
    );
    assert_eq!(
        catalog.blocks["tool-guidance"].text,
        builtin.blocks["tool-guidance"].text
    );
}

#[test]
fn load_from_dir_returns_err_for_unreadable_system_md() {
    // system.md exists but contains non-UTF-8 bytes; read_to_string must fail
    // and load_from_dir must surface the error rather than silently falling
    // back to the builtin catalog.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("system.md"), [0xFF, 0xFE, 0xFD]).unwrap();

    let result = PromptCatalog::load_from_dir(dir.path());
    assert!(
        result.is_err(),
        "expected Err for non-UTF-8 system.md, got Ok"
    );
}

#[test]
fn load_from_dir_returns_err_for_unreadable_block() {
    // A non-UTF-8 file under blocks/ must propagate as Err so that callers
    // (CLI --prompts-dir, runtime catalog reload) do not silently degrade.
    let dir = tempfile::tempdir().unwrap();
    let blocks = dir.path().join("blocks");
    std::fs::create_dir(&blocks).unwrap();
    std::fs::write(blocks.join("project-policy.md"), [0xFF, 0xFE, 0xFD]).unwrap();

    let result = PromptCatalog::load_from_dir(dir.path());
    assert!(
        result.is_err(),
        "expected Err for non-UTF-8 block file, got Ok"
    );
}
