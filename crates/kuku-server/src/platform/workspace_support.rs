use std::path::{Path, PathBuf};

use cap_std::fs::Dir;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::api::ApiError;

/// A persisted path used to register a workspace beneath an active root.
///
/// Unlike capability paths, a registration may name the root itself as `.`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum WorkspaceRegistrationPath {
    Root,
    Descendant(super::NormalizedRelativePath),
}

impl WorkspaceRegistrationPath {
    pub(super) fn parse(value: &str) -> Result<Self, ApiError> {
        if value == "." {
            return Ok(Self::Root);
        }
        Ok(Self::Descendant(super::NormalizedRelativePath::parse(
            value,
        )?))
    }

    pub(super) fn process_path(&self, root: &Path) -> PathBuf {
        match self {
            Self::Root => root.to_owned(),
            Self::Descendant(relative) => root.join(relative.as_path()),
        }
    }
}

impl Serialize for WorkspaceRegistrationPath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Root => serializer.serialize_str("."),
            Self::Descendant(relative) => relative.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for WorkspaceRegistrationPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value)
            .map_err(|_| serde::de::Error::custom("invalid workspace registration path"))
    }
}

pub(super) fn open_workspace_root(
    root: &Dir,
    relative: &WorkspaceRegistrationPath,
) -> Result<Dir, ApiError> {
    match relative {
        WorkspaceRegistrationPath::Root => root
            .try_clone()
            .map_err(|_| super::unavailable("workspace root is unavailable")),
        WorkspaceRegistrationPath::Descendant(relative) => {
            super::open_directory_relative(root, relative)
        }
    }
}

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
