use std::path::Path;

use crate::error::Result;

use super::definition::{SkillDefinition, SkillSource};

pub(crate) fn load_from_dir(_dir: &Path, _source: SkillSource) -> Result<Vec<SkillDefinition>> {
    Ok(Vec::new())
}
