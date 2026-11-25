use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use std::collections::BTreeMap;

pub const HEADER: &[u8] = b"kuku-skill-resources-v1\n";
pub const MAX_MANIFEST_BYTES: usize = 1 << 20;
pub const MAX_FILE_BYTES: usize = 8 << 20;
pub const MAX_CONTENT_BYTES: usize = 32 << 20;

#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_md: Option<String>,
    pub source_revision: String,
    pub files: BTreeMap<String, FileEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FileEntry {
    pub bytes: usize,
    pub sha256: String,
}

pub fn fingerprint(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 512
        && path.split('/').all(|part| {
            !part.is_empty()
                && !matches!(part, "." | "..")
                && !part.ends_with([' ', '.'])
                && !part.contains(['\\', ':', '<', '>', '"', '|', '?', '*'])
                && !part.chars().any(char::is_control)
        })
        && Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}
