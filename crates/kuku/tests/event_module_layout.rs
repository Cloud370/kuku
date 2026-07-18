#[test]
fn event_types_are_split_and_each_file_stays_below_limit() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    assert!(!root.join("src/event/types.rs").exists());
    for relative in [
        "src/event/types/mod.rs",
        "src/event/types/stored.rs",
        "src/event/types/payload.rs",
        "src/event/types/codec.rs",
    ] {
        let text = std::fs::read_to_string(root.join(relative)).unwrap();
        assert!(text.lines().count() < 1000, "{relative} is too large");
    }
}
