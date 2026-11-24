use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use super::sync_directory;

pub(super) fn create_dir_all(path: &Path) -> io::Result<()> {
    create_with(path, &mut sync_directory).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("cannot publish directory {}: {error}", path.display()),
        )
    })
}

fn create_with(
    path: &Path,
    synchronize: &mut impl FnMut(&Path) -> io::Result<()>,
) -> io::Result<()> {
    let absolute = std::env::current_dir()?.join(path);
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::RootDir => resolved.push(component.as_os_str()),
            Component::Normal(name) => {
                let directory = resolved.join(name);
                publish_directory(&directory, synchronize)?;
                resolved = fs::canonicalize(directory)?;
            }
            Component::ParentDir => {
                resolved.pop();
            }
            Component::CurDir => {}
            Component::Prefix(_) => unreachable!(),
        }
    }
    Ok(())
}

fn publish_directory(
    directory: &Path,
    synchronize: &mut impl FnMut(&Path) -> io::Result<()>,
) -> io::Result<()> {
    let marker = intent_path(directory);
    let Some(intent) = acquire_intent(directory, &marker)? else {
        return Ok(());
    };
    let parent = directory.parent().unwrap();
    intent.sync_all()?;
    synchronize(parent)?;
    match fs::create_dir(directory) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists && directory.is_dir() => {}
        Err(error) => return Err(error),
    }
    synchronize(directory)?;
    synchronize(parent)?;
    fs::remove_file(&marker)?;
    synchronize(parent)
}

fn acquire_intent(directory: &Path, marker: &Path) -> io::Result<Option<File>> {
    acquire_with(directory, marker, &mut |_| Ok(()), &mut || Ok(()))
}

fn acquire_with(
    directory: &Path,
    marker: &Path,
    opened: &mut impl FnMut(&File) -> io::Result<()>,
    missing: &mut impl FnMut() -> io::Result<()>,
) -> io::Result<Option<File>> {
    loop {
        let intent = match OpenOptions::new().read(true).write(true).open(marker) {
            Ok(intent) => intent,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                missing()?;
                match fs::metadata(directory) {
                    Ok(metadata) if metadata.is_dir() => {
                        // NOTE: Another creator may have installed an intent before this directory appeared.
                        match OpenOptions::new().read(true).write(true).open(marker) {
                            Ok(_) => continue,
                            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                                return Ok(None);
                            }
                            Err(error) => return Err(error),
                        }
                    }
                    Ok(_) => return Err(not_directory(directory)),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
                match OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(marker)
                {
                    Ok(intent) => intent,
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error),
                }
            }
            Err(error) => return Err(error),
        };
        opened(&intent)?;
        intent.lock()?;
        let current = match fs::symlink_metadata(marker) {
            Ok(metadata) if metadata.is_file() => metadata,
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "directory creation intent is not a regular file",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let held = intent.metadata()?;
        if held.len() != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "directory creation intent is not empty",
            ));
        }
        // NOTE: A waiter may hold an unlinked intent; only the current inode authorizes publication.
        if current.dev() == held.dev() && current.ino() == held.ino() {
            return Ok(Some(intent));
        }
    }
}

fn intent_path(directory: &Path) -> PathBuf {
    let name = directory.file_name().unwrap();
    let digest = Sha256::digest(name.as_bytes());
    directory
        .parent()
        .unwrap()
        .join(format!(".kuku-mkdir-v1-{digest:x}.intent"))
}

fn not_directory(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotADirectory,
        format!("{} is not a directory", path.display()),
    )
}

#[cfg(test)]
mod tests {
    use std::fs::{self, OpenOptions};
    use std::io::{self, Write};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::path::PathBuf;
    use std::sync::{Arc, Barrier, mpsc};

    use super::{
        acquire_with, create_dir_all, create_with, intent_path, publish_directory, sync_directory,
    };

    fn root() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn existing_unlistable_ancestors_need_no_directory_sync() {
        let root = root();
        let ancestor = root.path().join("ancestor");
        let session = ancestor.join("session");
        fs::create_dir_all(&session).unwrap();
        fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o111)).unwrap();
        let ordinary = OpenOptions::new()
            .create(true)
            .append(true)
            .open(session.join("events"));
        let actual = create_dir_all(&session);
        let mut synchronized = Vec::new();
        let observed = create_with(&session, &mut |path| {
            synchronized.push(path.to_path_buf());
            sync_directory(path)
        });
        fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(ordinary.is_ok());
        actual.unwrap();
        observed.unwrap();
        assert!(synchronized.is_empty());
    }

    #[test]
    fn required_sync_failure_keeps_a_recoverable_intent() {
        let root = root();
        let target = fs::canonicalize(root.path()).unwrap().join("new-directory");
        let failed = create_with(&target, &mut |path| {
            if path == target {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected directory sync failure",
                ));
            }
            sync_directory(path)
        });
        assert_eq!(io::ErrorKind::PermissionDenied, failed.unwrap_err().kind());
        assert!(target.is_dir());
        assert!(intent_path(&target).is_file());
        create_dir_all(&target).unwrap();
        assert!(!intent_path(&target).exists());
    }

    #[test]
    fn malformed_intents_are_rejected_without_deleting_their_contents() {
        let root = root();
        let target = root.path().join("directory");
        let marker = intent_path(&target);
        fs::write(&marker, b"unexpected contents").unwrap();
        assert_eq!(
            io::ErrorKind::InvalidData,
            create_dir_all(&target).unwrap_err().kind()
        );
        assert!(!target.exists());
        assert_eq!(
            b"unexpected contents",
            fs::read(&marker).unwrap().as_slice()
        );
    }

    #[test]
    fn recovers_nested_bootstrap_after_process_exit() {
        if let Some(target) = std::env::var_os("KUKU_MKDIR_CRASH_TARGET") {
            let target = PathBuf::from(target);
            if let Ok(stage) = std::env::var("KUKU_MKDIR_CRASH_STAGE") {
                let stage: usize = stage.parse().unwrap();
                let mut calls = 0;
                let result = create_with(&target, &mut |path| {
                    calls += 1;
                    if calls == stage {
                        if stage == 1 {
                            sync_directory(path)?;
                        }
                        std::process::exit(97);
                    }
                    sync_directory(path)
                });
                panic!("crash hook was not reached: {result:?}");
            }
            create_dir_all(&target).unwrap();
            let receipt = target.join("acknowledged");
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&receipt)
                .unwrap();
            file.write_all(b"durable receipt").unwrap();
            file.sync_all().unwrap();
            super::super::sync_parent(&receipt).unwrap();
            return;
        }
        let root = root();
        for stage in 1..=4 {
            let first = root.path().join(format!("case-{stage}"));
            let target = first.join("nested").join("data");
            let spawn = |crash: bool| {
                let mut command = std::process::Command::new(std::env::current_exe().unwrap());
                command
                    .env("KUKU_MKDIR_CRASH_TARGET", &target)
                    .arg("--exact")
                    .arg(std::thread::current().name().unwrap());
                if crash {
                    command.env("KUKU_MKDIR_CRASH_STAGE", stage.to_string());
                }
                command.output().unwrap()
            };
            let crashed = spawn(true);
            assert_eq!(Some(97), crashed.status.code());
            if stage < 4 {
                assert!(intent_path(&first).is_file());
            }
            let recovered = spawn(false);
            assert!(
                recovered.status.success(),
                "recovery failed: {}",
                String::from_utf8_lossy(&recovered.stderr)
            );
            assert_eq!(
                b"durable receipt",
                fs::read(target.join("acknowledged")).unwrap().as_slice()
            );
            for directory in [&first, &first.join("nested"), &target] {
                assert!(!intent_path(directory).exists());
            }
        }
    }

    #[test]
    fn concurrent_creators_publish_the_same_chain() {
        let root = root();
        let target = Arc::new(root.path().join("shared").join("nested"));
        let barrier = Arc::new(Barrier::new(8));
        let mut threads = Vec::new();
        for _ in 0..8 {
            let target = Arc::clone(&target);
            let barrier = Arc::clone(&barrier);
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                create_dir_all(&target)
            }));
        }
        for thread in threads {
            thread.join().unwrap().unwrap();
        }
        assert!(target.is_dir());
        assert!(!intent_path(&target).exists());
        assert!(!intent_path(target.parent().unwrap()).exists());
    }

    #[test]
    fn waiter_reopens_a_replaced_intent_inode_before_publication() {
        let root = root();
        let target = Arc::new(root.path().join("directory"));
        let marker = intent_path(&target);
        let original = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&marker)
            .unwrap();
        original.lock().unwrap();
        let (opened_tx, opened_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let waiting_target = Arc::clone(&target);
        let waiter = std::thread::spawn(move || {
            let marker = intent_path(&waiting_target);
            let mut attempts = 0;
            let intent = acquire_with(
                &waiting_target,
                &marker,
                &mut |file| {
                    attempts += 1;
                    opened_tx.send(file.metadata()?.ino()).unwrap();
                    if attempts == 1 {
                        resume_rx.recv().unwrap();
                    }
                    Ok(())
                },
                &mut || Ok(()),
            )
            .unwrap()
            .unwrap();
            assert_eq!(2, attempts);
            drop(intent);
            publish_directory(&waiting_target, &mut sync_directory).unwrap();
        });
        let old_inode = opened_rx.recv().unwrap();
        assert_eq!(original.metadata().unwrap().ino(), old_inode);
        fs::create_dir(&*target).unwrap();
        sync_directory(&target).unwrap();
        sync_directory(root.path()).unwrap();
        fs::remove_file(&marker).unwrap();
        drop(original);
        let replacement = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&marker)
            .unwrap();
        replacement.lock().unwrap();
        resume_tx.send(()).unwrap();
        assert_eq!(
            replacement.metadata().unwrap().ino(),
            opened_rx.recv().unwrap()
        );
        drop(replacement);
        waiter.join().unwrap();
        assert!(!marker.exists());
    }

    #[test]
    fn resolves_parent_components_after_existing_symlinks() {
        let root = root();
        let actual_parent = root.path().join("real").join("nested");
        create_dir_all(&actual_parent).unwrap();
        std::os::unix::fs::symlink(&actual_parent, root.path().join("alias")).unwrap();
        let target = root.path().join("alias").join("..").join("actual");
        create_dir_all(&target).unwrap();
        assert!(root.path().join("real").join("actual").is_dir());
        assert!(!root.path().join("actual").exists());
        let blocker = actual_parent.join("file");
        fs::write(&blocker, b"not a directory").unwrap();
        assert_eq!(
            io::ErrorKind::NotADirectory,
            create_dir_all(&blocker.join("child")).unwrap_err().kind()
        );
    }

    #[test]
    fn missing_probe_waits_for_an_intent_inserted_before_the_directory_appears() {
        #[derive(Debug, PartialEq, Eq)]
        enum Event {
            Missing,
            Opened,
            Returned,
        }

        let root = root();
        let target = Arc::new(root.path().join("directory"));
        let (event_tx, event_rx) = mpsc::channel();
        let (probe_tx, probe_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let observer_target = Arc::clone(&target);
        let observer = std::thread::spawn(move || {
            let mut first_missing = true;
            let result = acquire_with(
                &observer_target,
                &intent_path(&observer_target),
                &mut |_| {
                    event_tx.send(Event::Opened).unwrap();
                    Ok(())
                },
                &mut || {
                    if first_missing {
                        first_missing = false;
                        event_tx.send(Event::Missing).unwrap();
                        probe_rx.recv().unwrap();
                    }
                    Ok(())
                },
            )
            .map(drop);
            event_tx.send(Event::Returned).unwrap();
            done_tx.send(result.is_ok()).unwrap();
            result
        });
        assert_eq!(Event::Missing, event_rx.recv().unwrap());
        let (created_tx, created_rx) = mpsc::channel();
        let (publish_tx, publish_rx) = mpsc::channel();
        let publisher_target = Arc::clone(&target);
        let publisher = std::thread::spawn(move || {
            publish_directory(&publisher_target, &mut |path| {
                if path == *publisher_target {
                    created_tx.send(()).unwrap();
                    publish_rx.recv().unwrap();
                }
                sync_directory(path)
            })
        });
        created_rx.recv().unwrap();
        assert!(intent_path(&target).exists());
        probe_tx.send(()).unwrap();
        let first_event = event_rx.recv().unwrap();
        let premature = done_rx.recv_timeout(std::time::Duration::from_millis(30));
        publish_tx.send(()).unwrap();
        publisher.join().unwrap().unwrap();
        observer.join().unwrap().unwrap();
        assert_eq!(Event::Opened, first_event);
        assert!(
            matches!(premature, Err(mpsc::RecvTimeoutError::Timeout)),
            "observer returned before directory publication: {premature:?}"
        );
        assert!(!intent_path(&target).exists());
    }
}
