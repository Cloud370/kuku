use std::process::Command;

#[test]
fn config_validate_resolves_all_tiers_without_credentials() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("config.toml");
    let validate = || {
        Command::new(env!("CARGO_BIN_EXE_kuku-cli"))
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("OPENAI_API_KEY")
            .arg("--config")
            .arg(&path)
            .args(["config", "validate"])
            .output()
            .unwrap()
    };

    let generated = kuku::config::generate_default();
    let compatible = generated
        .replace("https://api.anthropic.com", "https://gateway.example")
        .replace("default_model = \"balanced\"", "default_model = \"light\"");
    for content in [generated, &compatible] {
        std::fs::write(&path, content).unwrap();
        let output = validate();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            "Config is valid.",
            String::from_utf8_lossy(&output.stdout).trim()
        );
    }

    let invalid = compatible.replace(
        "think = \"high\"",
        "think = \"high\"\nreasoning_effort = \"none\"",
    );
    std::fs::write(&path, invalid).unwrap();
    let output = validate();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("tier 'strong'"), "{error}");
    assert!(error.contains("connection 'anthropic'"), "{error}");
    assert!(
        error.contains("unsupported reasoning_effort 'none'"),
        "{error}"
    );
}
