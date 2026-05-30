use std::path::Path;

use crate::util::yaml::split_yaml_frontmatter;

use super::definition::{DefinitionSource, SubagentDefinition, ToolProfile};

pub(crate) fn load_from_dir(
    dir: &Path,
    source: DefinitionSource,
) -> crate::error::Result<Vec<SubagentDefinition>> {
    let mut defs = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Ok(defs),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "md") {
            continue;
        }
        let content = std::fs::read_to_string(&path)?;
        if let Some(def) = parse_definition(&content, source, &path) {
            defs.push(def);
        }
    }
    Ok(defs)
}

fn parse_definition(
    content: &str,
    source: DefinitionSource,
    path: &Path,
) -> Option<SubagentDefinition> {
    let (mapping, body) = split_yaml_frontmatter(content);
    let body = body.trim();

    let (name, description, tools, max_turns, tier, tool_profile, metadata) = if let Some(ref m) =
        mapping
    {
        let name = extract_str(m, "name")
            .or_else(|| path.file_stem().and_then(|s| s.to_str()).map(String::from))
            .unwrap_or_default();
        let description = extract_str(m, "description").unwrap_or_default();
        let model = extract_str(m, "model");
        let tools = extract_str_list(m, "tools").or_else(|| extract_str_list(m, "allowedTools"));
        let max_turns = extract_u32(m, "max_turns")
            .or_else(|| extract_u32(m, "maxTurns"))
            .unwrap_or(10);
        let tier = extract_str(m, "tier")
            .or_else(|| model.as_deref().map(map_model_to_tier))
            .unwrap_or_else(|| "balanced".to_string());
        let tool_profile =
            extract_tool_profile(m).unwrap_or_else(|| infer_profile_from_tools(&tools));
        let metadata = collect_unknown_metadata(
            m,
            &[
                "name",
                "description",
                "model",
                "tools",
                "allowedTools",
                "max_turns",
                "maxTurns",
                "tier",
                "tool_profile",
                "mode",
            ],
        );
        (
            name,
            description,
            tools,
            max_turns,
            tier,
            tool_profile,
            metadata,
        )
    } else {
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        (
            name,
            String::new(),
            None,
            10_u32,
            "balanced".to_string(),
            ToolProfile::Read,
            serde_json::Value::Null,
        )
    };

    if name.is_empty() {
        return None;
    }

    let instructions = body.to_string();
    let source_path = Some(path.display().to_string());

    let mut def = SubagentDefinition {
        name,
        description,
        instructions,
        tier,
        tool_profile,
        tools,
        max_turns,
        source,
        hash: String::new(),
        source_path,
        metadata,
    };
    def.hash = def.compute_hash();
    Some(def)
}

fn extract_str(mapping: &serde_yaml::Mapping, key: &str) -> Option<String> {
    mapping
        .get(serde_yaml::Value::String(key.to_string()))
        .and_then(|v| v.as_str().map(String::from))
}

fn extract_u32(mapping: &serde_yaml::Mapping, key: &str) -> Option<u32> {
    mapping
        .get(serde_yaml::Value::String(key.to_string()))
        .and_then(|v| v.as_u64().map(|n| n as u32))
}

fn extract_str_list(mapping: &serde_yaml::Mapping, key: &str) -> Option<Vec<String>> {
    mapping
        .get(serde_yaml::Value::String(key.to_string()))
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
}

fn extract_tool_profile(mapping: &serde_yaml::Mapping) -> Option<ToolProfile> {
    extract_str(mapping, "tool_profile").and_then(|s| match s.as_str() {
        "Read" => Some(ToolProfile::Read),
        "ReadWrite" => Some(ToolProfile::ReadWrite),
        "None" => Some(ToolProfile::None),
        _ => None,
    })
}

const WRITE_TOOLS: &[&str] = &[
    "Edit",
    "Write",
    "Bash",
    "edit_file",
    "write_file",
    "run_command",
    "remember_memory",
    "forget_memory",
];

fn infer_profile_from_tools(tools: &Option<Vec<String>>) -> ToolProfile {
    let Some(tools) = tools else {
        return ToolProfile::Read;
    };
    if tools.is_empty() {
        return ToolProfile::None;
    }
    let has_write = tools.iter().any(|t| WRITE_TOOLS.contains(&t.as_str()));
    if has_write {
        ToolProfile::ReadWrite
    } else {
        ToolProfile::Read
    }
}

fn map_model_to_tier(model: &str) -> String {
    let lower = model.to_lowercase();
    if lower.contains("opus") || lower.contains("gpt-4o") {
        "strong".to_string()
    } else if lower.contains("sonnet") || lower.contains("gpt-4") {
        "balanced".to_string()
    } else if lower.contains("haiku") {
        "light".to_string()
    } else {
        match lower.as_str() {
            "strong" | "opus" => "strong".to_string(),
            "balanced" | "sonnet" => "balanced".to_string(),
            "light" | "haiku" => "light".to_string(),
            _ => "balanced".to_string(),
        }
    }
}

fn collect_unknown_metadata(mapping: &serde_yaml::Mapping, known: &[&str]) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (k, v) in mapping {
        if let Some(key) = k.as_str() {
            if !known.contains(&key) {
                if let Ok(json_val) = serde_json::to_value(v) {
                    map.insert(key.to_string(), json_val);
                }
            }
        }
    }
    if map.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::Value::Object(map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_agent(dir: &Path, filename: &str, content: &str) -> std::path::PathBuf {
        let path = dir.join(filename);
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn loads_claude_code_format() {
        let tmp = tempfile::tempdir().unwrap();
        let content = "---\nname: reviewer\ndescription: review code\ntools:\n  - Read\n  - Edit\nmaxTurns: 5\n---\nYou are a reviewer.";
        write_agent(tmp.path(), "reviewer.md", content);
        let defs = load_from_dir(tmp.path(), DefinitionSource::User).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "reviewer");
        assert_eq!(defs[0].max_turns, 5);
        assert_eq!(defs[0].tool_profile, ToolProfile::ReadWrite);
    }

    #[test]
    fn loads_kuku_format() {
        let tmp = tempfile::tempdir().unwrap();
        let content = "---\nname: explorer\ndescription: explore\ntools:\n  - find_files\n  - read_file\nmax_turns: 15\ntier: light\ntool_profile: Read\n---\nExplore the codebase.";
        write_agent(tmp.path(), "explorer.md", content);
        let defs = load_from_dir(tmp.path(), DefinitionSource::Project).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "explorer");
        assert_eq!(defs[0].max_turns, 15);
        assert_eq!(defs[0].tier, "light");
        assert_eq!(defs[0].source, DefinitionSource::Project);
    }

    #[test]
    fn loads_opencode_format_with_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let content = "---\nname: coder\ntools:\n  - Read\n  - Write\nmode: code\n---\nWrite code.";
        write_agent(tmp.path(), "coder.md", content);
        let defs = load_from_dir(tmp.path(), DefinitionSource::User).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "coder");
    }

    #[test]
    fn missing_frontmatter_uses_filename() {
        let tmp = tempfile::tempdir().unwrap();
        write_agent(
            tmp.path(),
            "helper.md",
            "Just instructions, no frontmatter.",
        );
        let defs = load_from_dir(tmp.path(), DefinitionSource::User).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "helper");
        assert_eq!(defs[0].max_turns, 10);
        assert_eq!(defs[0].tool_profile, ToolProfile::Read);
    }

    #[test]
    fn missing_name_uses_filename_stem() {
        let tmp = tempfile::tempdir().unwrap();
        let content = "---\ndescription: no name field\n---\nInstructions.";
        write_agent(tmp.path(), "auto-named.md", content);
        let defs = load_from_dir(tmp.path(), DefinitionSource::User).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "auto-named");
    }

    #[test]
    fn empty_name_skips_definition() {
        let tmp = tempfile::tempdir().unwrap();
        let content = "---\nname: \"\"\n---\nInstructions.";
        write_agent(tmp.path(), ".md", content);
        let defs = load_from_dir(tmp.path(), DefinitionSource::User).unwrap();
        assert!(defs.is_empty());
    }

    #[test]
    fn non_md_files_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("readme.txt"), "not an agent").unwrap();
        let defs = load_from_dir(tmp.path(), DefinitionSource::User).unwrap();
        assert!(defs.is_empty());
    }

    #[test]
    fn tool_name_translation_both_conventions() {
        let tools_cc = Some(vec!["Edit".to_string(), "Bash".to_string()]);
        assert_eq!(infer_profile_from_tools(&tools_cc), ToolProfile::ReadWrite);

        let tools_kuku = Some(vec!["edit_file".to_string(), "write_file".to_string()]);
        assert_eq!(
            infer_profile_from_tools(&tools_kuku),
            ToolProfile::ReadWrite
        );

        let tools_read = Some(vec!["Read".to_string(), "find_files".to_string()]);
        assert_eq!(infer_profile_from_tools(&tools_read), ToolProfile::Read);

        assert_eq!(infer_profile_from_tools(&None), ToolProfile::Read);
    }

    #[test]
    fn max_turns_variant_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let c1 = "---\nname: a\nmax_turns: 7\n---\nbody";
        write_agent(tmp.path(), "a.md", c1);
        let defs = load_from_dir(tmp.path(), DefinitionSource::User).unwrap();
        assert_eq!(defs[0].max_turns, 7);
    }

    #[test]
    fn max_turns_camel_case_key() {
        let tmp = tempfile::tempdir().unwrap();
        let c2 = "---\nname: b\nmaxTurns: 3\n---\nbody";
        write_agent(tmp.path(), "b.md", c2);
        let defs = load_from_dir(tmp.path(), DefinitionSource::User).unwrap();
        assert_eq!(defs[0].max_turns, 3);
    }

    #[test]
    fn unknown_keys_collected_in_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let content = "---\nname: test\ncustom_field: value\nanother: 42\n---\nbody";
        write_agent(tmp.path(), "test.md", content);
        let defs = load_from_dir(tmp.path(), DefinitionSource::User).unwrap();
        assert_eq!(defs[0].metadata["custom_field"], "value");
        assert_eq!(defs[0].metadata["another"], 42);
    }

    #[test]
    fn empty_tools_list_gives_none_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let content = "---\nname: restricted\ntools: []\n---\nbody";
        write_agent(tmp.path(), "restricted.md", content);
        let defs = load_from_dir(tmp.path(), DefinitionSource::User).unwrap();
        assert_eq!(defs[0].tool_profile, ToolProfile::None);
    }

    #[test]
    fn allowed_tools_key_variant() {
        let tmp = tempfile::tempdir().unwrap();
        let content = "---\nname: alt\nallowedTools:\n  - Edit\n  - Read\n---\nbody";
        write_agent(tmp.path(), "alt.md", content);
        let defs = load_from_dir(tmp.path(), DefinitionSource::User).unwrap();
        assert_eq!(
            defs[0].tools.as_ref().unwrap(),
            &vec!["Edit".to_string(), "Read".to_string()]
        );
    }

    #[test]
    fn model_infers_tier_when_no_explicit_tier() {
        let tmp = tempfile::tempdir().unwrap();
        let content = "---\nname: smart\nmodel: claude-opus-4\n---\nbody";
        write_agent(tmp.path(), "smart.md", content);
        let defs = load_from_dir(tmp.path(), DefinitionSource::User).unwrap();
        assert_eq!(defs[0].tier, "strong");
    }

    #[test]
    fn explicit_tier_overrides_model_inference() {
        let tmp = tempfile::tempdir().unwrap();
        let content = "---\nname: override\nmodel: claude-opus-4\ntier: light\n---\nbody";
        write_agent(tmp.path(), "override.md", content);
        let defs = load_from_dir(tmp.path(), DefinitionSource::User).unwrap();
        assert_eq!(defs[0].tier, "light");
    }

    #[test]
    fn hash_matches_compute_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let content = "---\nname: hashcheck\n---\nInstructions.";
        write_agent(tmp.path(), "hashcheck.md", content);
        let defs = load_from_dir(tmp.path(), DefinitionSource::User).unwrap();
        assert_eq!(defs[0].hash, defs[0].compute_hash());
    }

    #[test]
    fn empty_tools_for_claude_convention() {
        let tmp = tempfile::tempdir().unwrap();
        let content = "---\nname: empty-cc\ntools: []\n---\nbody";
        write_agent(tmp.path(), "empty-cc.md", content);
        let defs = load_from_dir(tmp.path(), DefinitionSource::User).unwrap();
        assert_eq!(defs[0].tool_profile, ToolProfile::None);
    }
}
