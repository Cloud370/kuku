use std::path::Path;

use crate::error::{Error, Result};
use crate::subagent::compat::claude_code::split_yaml_frontmatter;

use super::definition::{SkillDefinition, SkillSource};

pub(crate) fn load_from_dir(dir: &Path, source: SkillSource) -> Result<Vec<SkillDefinition>> {
    let mut defs = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let skill_dir = entry.path();
        if !skill_dir.is_dir() {
            continue;
        }
        let skill_md = skill_dir.join("SKILL.md");
        if !skill_md.exists() {
            continue;
        }
        let content = std::fs::read_to_string(&skill_md)?;
        match parse_skill(&content, &skill_dir, &source) {
            Ok(def) => defs.push(def),
            Err(e) => {
                eprintln!("warning: skipping skill at {}: {e}", skill_dir.display());
            }
        }
    }
    Ok(defs)
}

fn parse_skill(
    content: &str,
    skill_dir: &Path,
    source: &SkillSource,
) -> Result<SkillDefinition> {
    let (frontmatter, body) = split_yaml_frontmatter(content);
    let mapping = frontmatter.ok_or_else(|| {
        Error::InvalidArgument(format!(
            "missing YAML frontmatter in {}",
            skill_dir.join("SKILL.md").display()
        ))
    })?;
    let value = serde_yaml::Value::Mapping(mapping.clone());

    let dir_name = skill_dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unnamed");

    let name = value["name"]
        .as_str()
        .map(String::from)
        .unwrap_or_else(|| dir_name.to_string());

    let description = value["description"].as_str().unwrap_or("").to_string();

    if name.is_empty() || !is_valid_skill_name(&name) {
        return Err(Error::InvalidArgument(format!(
            "skill in {} has invalid name '{name}' (must be 1-64 chars, lowercase alphanumeric with single hyphens, matching directory name)",
            skill_dir.display()
        )));
    }
    if name != dir_name {
        return Err(Error::InvalidArgument(format!(
            "skill name '{name}' does not match directory name '{dir_name}' in {}",
            skill_dir.display()
        )));
    }
    if description.is_empty() {
        return Err(Error::InvalidArgument(format!(
            "skill '{name}' in {} has no description",
            skill_dir.display()
        )));
    }

    let allowed_tools = parse_string_array(&value, "allowed-tools");
    let disallowed_tools = parse_string_array(&value, "disallowed-tools");
    let max_turns = value["max-turns"].as_u64().map(|n| n as u32);
    let model = value["model"].as_str().map(String::from);
    let license = value["license"].as_str().map(String::from);
    let compatibility = value["compatibility"].as_str().map(String::from);

    let known_keys = [
        "name",
        "description",
        "allowed-tools",
        "disallowed-tools",
        "max-turns",
        "model",
        "license",
        "compatibility",
        "metadata",
    ];
    let metadata = collect_unknown_metadata(&value, &known_keys);

    let mut def = SkillDefinition {
        name,
        description,
        instructions: body.trim().to_string(),
        source: source.clone(),
        hash: String::new(),
        source_path: Some(skill_dir.display().to_string()),
        allowed_tools,
        disallowed_tools,
        max_turns,
        model,
        license,
        compatibility,
        metadata,
    };
    def.hash = def.compute_hash();
    Ok(def)
}

fn is_valid_skill_name(name: &str) -> bool {
    if name.len() > 64 || name.is_empty() {
        return false;
    }
    let mut prev_hyphen = false;
    for ch in name.chars() {
        if ch == '-' {
            if prev_hyphen || name.starts_with('-') || name.ends_with('-') {
                return false;
            }
            prev_hyphen = true;
        } else if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            prev_hyphen = false;
        } else {
            return false;
        }
    }
    true
}

fn collect_unknown_metadata(
    value: &serde_yaml::Value,
    known_keys: &[&str],
) -> serde_json::Value {
    let serde_yaml::Value::Mapping(ref map) = *value else {
        return serde_json::Value::Null;
    };
    let mut meta = serde_json::Map::new();
    for (key, val) in map {
        let Some(key_str) = key.as_str() else {
            continue;
        };
        if known_keys.contains(&key_str) || key_str == "metadata" {
            if key_str == "metadata" {
                if let Ok(json_val) = serde_json::to_value(val) {
                    return json_val;
                }
            }
            continue;
        }
        if let Ok(json_val) = serde_json::to_value(val) {
            meta.insert(key_str.to_string(), json_val);
        }
    }
    if meta.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::Value::Object(meta)
    }
}

fn parse_string_array(value: &serde_yaml::Value, key: &str) -> Option<Vec<String>> {
    match &value[key] {
        serde_yaml::Value::Sequence(seq) => {
            let list: Vec<String> = seq
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
            Some(list)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_skill(dir: &Path, name: &str, body: &str) {
        let skill_dir = dir.join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Test skill\n---\n\n{body}\n"),
        )
        .unwrap();
    }

    #[test]
    fn load_from_dir_finds_skills() {
        let dir = tempfile::tempdir().unwrap();
        create_skill(dir.path(), "tdd", "Write tests first.");
        create_skill(dir.path(), "review", "Review carefully.");

        let defs = load_from_dir(dir.path(), SkillSource::KukuProject).unwrap();
        assert_eq!(defs.len(), 2);
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"tdd"));
        assert!(names.contains(&"review"));
    }

    #[test]
    fn load_from_dir_skips_non_directories() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("not-a-dir.md"), "ignored").unwrap();
        create_skill(dir.path(), "real", "Real skill.");

        let defs = load_from_dir(dir.path(), SkillSource::KukuProject).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "real");
    }

    #[test]
    fn load_from_dir_skips_missing_skill_md() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("empty");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("README.md"), "no SKILL.md here").unwrap();

        let defs = load_from_dir(dir.path(), SkillSource::KukuProject).unwrap();
        assert!(defs.is_empty());
    }

    #[test]
    fn parse_skill_with_optional_fields() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("custom");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: custom\ndescription: Custom skill\nallowed-tools:\n  - read_file\n  - search_text\ndisallowed-tools:\n  - run_command\nmax-turns: 10\nmodel: strong\nlicense: MIT\ncompatibility: linux/macos\n---\n\nInstructions here.\n",
        ).unwrap();

        let defs = load_from_dir(dir.path(), SkillSource::KukuUser).unwrap();
        assert_eq!(defs.len(), 1);
        let def = &defs[0];
        assert_eq!(def.name, "custom");
        assert_eq!(def.allowed_tools.as_ref().unwrap().len(), 2);
        assert_eq!(def.disallowed_tools.as_ref().unwrap().len(), 1);
        assert_eq!(def.max_turns, Some(10));
        assert_eq!(def.model.as_deref(), Some("strong"));
        assert_eq!(def.license.as_deref(), Some("MIT"));
        assert_eq!(def.compatibility.as_deref(), Some("linux/macos"));
        assert_eq!(def.instructions, "Instructions here.");
    }

    #[test]
    fn parse_skill_name_defaults_to_dir_name() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\ndescription: A skill\n---\n\nBody.\n",
        )
        .unwrap();

        let defs = load_from_dir(dir.path(), SkillSource::KukuProject).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "my-skill");
    }

    #[test]
    fn parse_skill_rejects_missing_description() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("bad");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: bad\n---\n\nBody.\n",
        )
        .unwrap();

        let defs = load_from_dir(dir.path(), SkillSource::KukuProject).unwrap();
        assert!(defs.is_empty(), "should skip skill with empty description");
    }

    #[test]
    fn parse_skill_rejects_missing_frontmatter() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("no-fm");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "Just plain text.\n").unwrap();

        let defs = load_from_dir(dir.path(), SkillSource::KukuProject).unwrap();
        assert!(defs.is_empty());
    }

    #[test]
    fn parse_skill_rejects_invalid_name_uppercase() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("Bad-Name");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: Bad-Name\ndescription: Test\n---\n\nBody.\n",
        )
        .unwrap();

        let defs = load_from_dir(dir.path(), SkillSource::KukuProject).unwrap();
        assert!(defs.is_empty(), "should reject uppercase name");
    }

    #[test]
    fn parse_skill_rejects_name_not_matching_directory() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("dir-name");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: other-name\ndescription: Test\n---\n\nBody.\n",
        )
        .unwrap();

        let defs = load_from_dir(dir.path(), SkillSource::KukuProject).unwrap();
        assert!(defs.is_empty(), "should reject name not matching directory");
    }

    #[test]
    fn parse_skill_rejects_name_too_long() {
        let dir = tempfile::tempdir().unwrap();
        let long_name = "a".repeat(65);
        let skill_dir = dir.path().join(&long_name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {long_name}\ndescription: Test\n---\n\nBody.\n"),
        )
        .unwrap();

        let defs = load_from_dir(dir.path(), SkillSource::KukuProject).unwrap();
        assert!(defs.is_empty(), "should reject name > 64 chars");
    }

    #[test]
    fn parse_skill_collects_unknown_frontmatter_into_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("meta");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: meta\ndescription: Test\nauthor: someone\nversion: 2\n---\n\nBody.\n",
        )
        .unwrap();

        let defs = load_from_dir(dir.path(), SkillSource::KukuProject).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].metadata["author"], "someone");
        assert_eq!(defs[0].metadata["version"], 2);
    }
}
