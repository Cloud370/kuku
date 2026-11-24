#[cfg(unix)]
use std::fs;
use std::io::Write;

use super::read_private_key;
use crate::util::private_directory;

fn staged_file(contents: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let credentials = directory.path().join("credentials");
    private_directory(&credentials).unwrap();
    let mut file = tempfile::NamedTempFile::new_in(credentials).unwrap();
    file.write_all(contents).unwrap();
    file.as_file().sync_all().unwrap();
    #[cfg(windows)]
    crate::util::set_config_temp_security(None, file.path(), file.as_file()).unwrap();
    let (_, path) = file.keep().unwrap();
    (directory, path)
}

#[test]
fn reads_a_private_single_line_key() {
    let (_directory, path) = staged_file(b"fixture-not-a-real-key");
    assert!(read_private_key(&path).unwrap() == "fixture-not-a-real-key");
}

#[test]
fn rejects_invalid_content_without_echoing_it() {
    for contents in [
        b"".as_slice(),
        b"fixture-not-a-real-key\n".as_slice(),
        b"fixture-not-a-real-key\tmore".as_slice(),
        b"\xff".as_slice(),
    ] {
        let (_directory, path) = staged_file(contents);
        let error = read_private_key(&path).unwrap_err();
        assert!(!error.to_string().contains("fixture-not-a-real-key"));
    }
}

#[cfg(unix)]
#[test]
fn rejects_fifo_without_blocking() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("credential-pipe");
    let c_path = CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: The CString provides a live NUL-terminated path in the test directory.
    assert_eq!(0, unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) });
    assert_eq!(
        std::io::ErrorKind::PermissionDenied,
        read_private_key(&path).unwrap_err().kind()
    );
}

#[cfg(unix)]
#[test]
fn rejects_files_that_are_not_owner_only_or_have_multiple_links() {
    use std::os::unix::fs::PermissionsExt;

    let (directory, path) = staged_file(b"fixture-not-a-real-key");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    assert_eq!(
        std::io::ErrorKind::PermissionDenied,
        read_private_key(&path).unwrap_err().kind()
    );

    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let link = directory.path().join("second-link");
    fs::hard_link(&path, &link).unwrap();
    assert_eq!(
        std::io::ErrorKind::PermissionDenied,
        read_private_key(&path).unwrap_err().kind()
    );
}

#[cfg(unix)]
#[test]
fn rejects_symbolic_links() {
    use std::os::unix::fs::symlink;

    let (directory, path) = staged_file(b"fixture-not-a-real-key");
    let link = directory.path().join("link");
    symlink(&path, &link).unwrap();
    assert!(read_private_key(&link).is_err());
}

#[test]
fn rejects_a_directory_as_a_credential_file() {
    let directory = tempfile::tempdir().unwrap();
    assert!(read_private_key(directory.path()).is_err());
}
