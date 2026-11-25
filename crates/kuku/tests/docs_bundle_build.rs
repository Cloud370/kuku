use std::fs;
use std::io::Read;

use flate2::read::GzDecoder;

#[path = "../build/docs.rs"]
mod build_docs;
#[path = "../src/skill/bundle/format.rs"]
mod bundle_format;

#[test]
fn source_links_in_the_bundle_preserve_titles_and_decoded_query_strings() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("docs");
    fs::create_dir_all(root.join("en")).unwrap();
    let source = "../../README.md";
    let query = "?first=1&amp;second=2";
    let title = format!("Example ]({source}) and ]: {source}{query}");
    let page = format!(
        "[link]({source} \"{title}\")\n\n\
         [ref]: {source}{query} \"{title}\"\n\n\
         [![image]({source}{query})]({source}{query})\n"
    );
    fs::write(root.join("en/fixture.md"), page).unwrap();
    let skill = temp.path().join("SKILL.md");
    let instructions =
        "---\nname: kuku-docs\ndescription: Fixture skill\n---\nFollow the bundled references.\n";
    fs::write(&skill, instructions.replace('\n', "\r\n")).unwrap();
    let revision = "a".repeat(40);
    let version = build_docs::build(&root, &revision, temp.path(), &skill).unwrap();

    let bundle = fs::read(temp.path().join("kuku-docs.bundle")).unwrap();
    let payload = bundle.strip_prefix(bundle_format::HEADER).unwrap();
    let manifest_size = u32::from_be_bytes(payload[..4].try_into().unwrap()) as usize;
    let index = &payload[4..4 + manifest_size];
    let manifest: bundle_format::Manifest = serde_json::from_slice(index).unwrap();
    assert_eq!(bundle_format::fingerprint(index), version);
    assert_eq!(manifest.skill_md.as_deref(), Some(instructions));
    let mut markdown = String::new();
    GzDecoder::new(&payload[4 + manifest_size..])
        .read_to_string(&mut markdown)
        .unwrap();
    let url = format!("{}/blob/{revision}/README.md", env!("CARGO_PKG_REPOSITORY"));
    assert_eq!(
        markdown,
        format!(
            "[link](<{url}> \"{title}\")\n\n\
             [ref]: <{url}{query}> \"{title}\"\n\n\
             [![image](<{url}{query}>)](<{url}{query}>)\n"
        )
    );
}
