use std::collections::BTreeMap;
use std::sync::Barrier;

use serde_json::json;

use super::*;

fn read_index() -> resources::Request {
    resources::Request::from_args(&json!({
        "skill_name": NAME,
        "action": "read",
        "path": "references/docs/en/index.md",
    }))
    .unwrap()
}

#[test]
fn concurrent_first_reads_share_a_full_cache_and_repair_corruption() {
    let home = tempfile::tempdir().unwrap();
    let parent = home.path().join("skill-resources").join(NAME);
    let cache_path = parent.join(CURRENT);
    assert!(!parent.exists());
    retain_cancellable(home.path(), CURRENT, &|| false).unwrap();
    assert!(cache_path.with_extension("bundle").is_file());
    assert!(!cache_path.exists());

    let barrier = Barrier::new(6);
    std::thread::scope(|scope| {
        let handles = (0..6)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    let result = read_index()
                        .execute(home.path(), CURRENT, &|| false)
                        .unwrap();
                    assert_eq!(result.status, "ok");
                    assert_eq!(result.structured.unwrap()["version"], CURRENT);
                    let (index, _) = archive_parts(CONTENT, CURRENT).unwrap();
                    assert_eq!(fs::read(cache_path.join(MANIFEST)).unwrap(), index);
                    let docs = cache_path.join(RESOURCE_ROOT);
                    for (path, entry) in &parse_manifest(index, CURRENT).unwrap().files {
                        assert_eq!(
                            fingerprint(&fs::read(docs.join(path)).unwrap()),
                            entry.sha256
                        );
                    }
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            handle.join().unwrap();
        }
    });
    assert!(!fs::read_dir(&parent).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".extract-")
    }));

    let index = cache_path.join(RESOURCE_ROOT).join("en/index.md");
    fs::write(&index, "tampered document").unwrap();
    let result = read_index()
        .execute(home.path(), CURRENT, &|| false)
        .unwrap();
    assert_eq!(result.status, "ok");
    assert!(!result.model_content.contains("tampered document"));
    let persisted = fs::read_to_string(index).unwrap();
    assert!(
        result
            .model_content
            .contains(persisted.lines().next().unwrap())
    );
}

#[test]
fn an_interrupted_cache_uses_the_retained_version_on_retry() {
    let home = tempfile::tempdir().unwrap();
    let note = b"# Older release documentation\n\nPrevious behavior.\n";
    let index = serde_json::to_vec(&Manifest {
        source_revision: "1".repeat(40),
        files: BTreeMap::from([(
            "en/index.md".into(),
            FileEntry {
                bytes: note.len(),
                sha256: fingerprint(note),
            },
        )]),
    })
    .unwrap();
    let version = fingerprint(&index);
    assert_ne!(version, CURRENT);
    let path = version_path(home.path(), &version).unwrap();
    let orphan = path
        .parent()
        .unwrap()
        .join(format!(".extract-{version}-orphan"));
    fs::create_dir(&orphan).unwrap();
    fs::write(orphan.join("partial"), b"unfinished").unwrap();
    let mut header = HEADER.to_vec();
    header.extend(u32::try_from(index.len()).unwrap().to_be_bytes());
    header.extend(&index);
    let mut contents = flate2::write::GzEncoder::new(header, flate2::Compression::fast());
    contents.write_all(note).unwrap();
    let archive = contents.finish().unwrap();

    let archive_path = path.with_extension("bundle");
    fs::write(&archive_path, &archive[..archive.len() - 3]).unwrap();
    assert!(
        read_index()
            .execute(home.path(), &version, &|| false)
            .is_err()
    );
    assert!(!path.exists());

    fs::write(&archive_path, archive).unwrap();
    let result = read_index()
        .execute(home.path(), &version, &|| false)
        .unwrap();
    assert!(result.model_content.contains("Previous behavior."));
    assert_eq!(result.structured.unwrap()["version"], version);
    assert!(!path.with_file_name(CURRENT).exists());
    assert!(!orphan.exists());
}

#[cfg(unix)]
#[test]
fn a_cache_symlink_cannot_supply_resource_bytes() {
    let home = tempfile::tempdir().unwrap();
    let outside = tempfile::NamedTempFile::new().unwrap();
    fs::write(outside.path(), "outside docs scope").unwrap();
    read_index()
        .execute(home.path(), CURRENT, &|| false)
        .unwrap();
    let target = version_path(home.path(), CURRENT)
        .unwrap()
        .join(RESOURCE_ROOT)
        .join("en/index.md");
    fs::remove_file(&target).unwrap();
    std::os::unix::fs::symlink(outside.path(), &target).unwrap();
    let result = read_index()
        .execute(home.path(), CURRENT, &|| false)
        .unwrap();
    assert_eq!(result.status, "ok");
    assert!(!result.model_content.contains("outside docs scope"));
    assert!(!fs::symlink_metadata(&target).unwrap().is_symlink());
}
