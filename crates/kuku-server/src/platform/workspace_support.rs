use std::path::Path;

impl super::WorkspaceCapability {
    pub(crate) fn process_path(&self) -> &Path {
        self.process_root.process_path()
    }
}

impl super::WorkspaceRegistry {
    pub(crate) fn generation_now(&self) -> u64 {
        let state = self
            .state
            .read()
            .expect("workspace state lock is not poisoned");
        let digest = self.digest_state(&state);
        u64::from_be_bytes(
            digest.as_bytes()[..8]
                .try_into()
                .expect("digest prefix is eight bytes"),
        )
    }
}

#[cfg(unix)]
pub(super) fn encode_link_target(target: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;

    target.as_os_str().as_bytes().to_vec()
}

#[cfg(windows)]
pub(super) fn encode_link_target(target: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;

    target
        .as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}
