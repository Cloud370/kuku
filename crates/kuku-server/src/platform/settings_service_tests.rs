use super::SettingsService;

#[test]
fn recovery_rejects_journal_bytes_that_do_not_match_intended_digest() {
    let home = tempfile::tempdir().unwrap();
    let settings_path = home.path().join("settings.json");
    std::fs::write(&settings_path, b"old-settings").unwrap();
    let intended = br#"{"format_version":1,"max_concurrent_runs":4}"#;
    let tampered = br#"{"format_version":1,"max_concurrent_runs":9}"#;
    let journal = serde_json::json!({
        "format_version": 1,
        "config_toml": null,
        "config_digest": null,
        "workspaces_json": null,
        "workspaces_digest": null,
        "settings_json": std::str::from_utf8(tampered).unwrap(),
        "settings_digest": digest_hex(intended),
    });
    std::fs::write(
        home.path().join("settings.journal.json"),
        serde_json::to_vec(&journal).unwrap(),
    )
    .unwrap();

    let error = SettingsService::recover(home.path()).unwrap_err();
    assert_eq!(crate::api::ApiErrorCode::Internal, error.code());
    assert_eq!(
        b"old-settings",
        std::fs::read(settings_path).unwrap().as_slice()
    );
    assert!(home.path().join("settings.journal.json").exists());
}

fn digest_hex(bytes: &[u8]) -> String {
    super::accepted_digest(bytes)
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
