use std::io;
use std::os::fd::{AsFd, AsRawFd};

use tokio::process::Command;

use crate::util::bound_directory::BoundDirectory;

pub(super) fn configure(directory: &BoundDirectory, command: &mut Command) -> io::Result<()> {
    let directory = directory.as_fd().try_clone_to_owned()?;
    // SAFETY: The closure owns the directory descriptor and only calls async-signal-safe fchdir.
    unsafe {
        command.pre_exec(move || {
            if libc::fchdir(directory.as_raw_fd()) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::symlink;

    use super::{BoundDirectory, configure};

    #[tokio::test]
    async fn process_uses_the_open_directory_if_the_path_is_replaced() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let original = root.join("approved");
        let moved = root.join("moved");
        let replacement = root.join("replacement");
        std::fs::create_dir(&original).unwrap();
        std::fs::create_dir(&replacement).unwrap();
        let directory = BoundDirectory::open(&original).unwrap();

        std::fs::rename(&original, &moved).unwrap();
        symlink(&replacement, &original).unwrap();
        assert!(BoundDirectory::open(&original).is_err());

        let mut command = tokio::process::Command::new("pwd");
        configure(&directory, &mut command).unwrap();
        let output = command.output().await.unwrap();
        assert!(output.status.success());
        assert_eq!(
            moved.to_str().unwrap(),
            std::str::from_utf8(&output.stdout).unwrap().trim()
        );
    }
}
