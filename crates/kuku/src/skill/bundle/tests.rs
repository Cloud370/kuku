use std::collections::BTreeMap;
use std::sync::Barrier;

use super::format::FileEntry;
use super::*;

#[test]
fn concurrent_publishers_share_one_complete_real_directory() {
    let home = tempfile::tempdir().unwrap();
    let barrier = Barrier::new(6);
    let root = std::thread::scope(|scope| {
        let handles = (0..6)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    materialize_cancellable(home.path(), CURRENT, INSTRUCTIONS, &|| false).unwrap()
                })
            })
            .collect::<Vec<_>>();
        let roots = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert!(roots.iter().all(|path| *path == roots[0]));
        roots[0].clone()
    });
    assert!(root.is_absolute());
    let (index, _) = archive_parts(CONTENT, CURRENT).unwrap();
    let manifest = parse_manifest(index, CURRENT).unwrap();
    assert_eq!(
        fs::read_to_string(root.join("SKILL.md")).unwrap(),
        manifest.skill_md.unwrap()
    );
    assert_eq!(fs::read(root.join(MANIFEST)).unwrap(), index);
    for (path, entry) in &manifest.files {
        assert_eq!(
            fingerprint(&fs::read(root.join(RESOURCE_ROOT).join(path)).unwrap()),
            entry.sha256
        );
    }
    assert!(root.with_extension("bundle").is_file());
    assert!(!fs::read_dir(root.parent().unwrap()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".extract-")
    }));
}

#[test]
fn interrupted_publication_restores_the_retained_version_not_the_current_build() {
    let home = tempfile::tempdir().unwrap();
    let note = b"# Older release documentation\n\nPrevious behavior.\n";
    let previous_skill =
        "---\nname: kuku-docs\ndescription: Original instructions\n---\nRead the old docs.\n";
    let index = serde_json::to_vec(&Manifest {
        skill_md: None, // Existing archives captured the documents separately.
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
    assert!(materialize_cancellable(home.path(), &version, previous_skill, &|| false).is_err());
    assert!(!path.exists());
    fs::write(&archive_path, archive).unwrap();
    let old = materialize_cancellable(home.path(), &version, previous_skill, &|| false).unwrap();
    assert!(!orphan.exists());
    let current = materialize_cancellable(home.path(), CURRENT, INSTRUCTIONS, &|| false).unwrap();
    assert_ne!(old, current);
    assert_eq!(
        fs::read(old.join(RESOURCE_ROOT).join("en/index.md")).unwrap(),
        note
    );
    assert_eq!(
        fs::read_to_string(old.join("SKILL.md")).unwrap(),
        previous_skill
    );
    assert!(
        !fs::read_to_string(current.join("SKILL.md"))
            .unwrap()
            .contains("Original instructions")
    );
}

#[cfg(unix)]
#[test]
fn a_symlink_cannot_replace_the_published_skill_entry() {
    let home = tempfile::tempdir().unwrap();
    let outside = tempfile::NamedTempFile::new().unwrap();
    fs::write(outside.path(), "outside docs scope").unwrap();
    let root = materialize_cancellable(home.path(), CURRENT, INSTRUCTIONS, &|| false).unwrap();
    let target = root.join("SKILL.md");
    fs::remove_file(&target).unwrap();
    std::os::unix::fs::symlink(outside.path(), &target).unwrap();
    materialize_cancellable(home.path(), CURRENT, INSTRUCTIONS, &|| false).unwrap();
    assert!(!fs::symlink_metadata(&target).unwrap().is_symlink());
    assert!(
        !fs::read_to_string(target)
            .unwrap()
            .contains("outside docs scope")
    );
    assert_eq!(
        fs::read_to_string(outside.path()).unwrap(),
        "outside docs scope"
    );
}
