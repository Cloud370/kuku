use std::path::Path;

use crate::error::Result;

use super::super::definition::{DefinitionSource, SubagentDefinition};

/// Load Claude Code agent definitions from a directory (`.claude/agents/` or `~/.claude/agents/`).
pub fn load_from_dir(dir: &Path, source: DefinitionSource) -> Result<Vec<SubagentDefinition>> {
    let mut definitions = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Ok(definitions),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "md") {
            continue;
        }
        if let Some(def) = parse_claude_code_agent(&path, source.clone())? {
            definitions.push(def);
        }
    }
    Ok(definitions)
}

fn parse_claude_code_agent(
    path: &Path,
    source: DefinitionSource,
) -> Result<Option<SubagentDefinition>> {
    let content = std::fs::read_to_string(path)?;
    let (frontmatter, body) = split_yaml_frontmatter(&content);
    let mut def = SubagentDefinition {
        name: String::new(),
        description: String::new(),
        instructions: body.trim().to_string(),
        tier: "balanced".into(),
        tool_profile: super::super::definition::ToolProfile::Read,
        permission: Default::default(),
        max_turns: 10,
        output_contract: Default::default(),
        source: source.clone(),
        hash: String::new(),
        source_path: Some(path.display().to_string()),
        metadata: serde_json::Value::Null,
    };

    if let Some(fm) = frontmatter {
        def.name = fm
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        def.description = fm
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if let Some(model) = fm.get("model").and_then(|v| v.as_str()) {
            def.tier = map_claude_model_to_tier(model);
        }
        if let Some(tools) = fm.get("tools").and_then(|v| v.as_sequence()) {
            def.tool_profile = infer_profile_from_tools(tools);
        }
        if let Some(mt) = fm.get("maxTurns").and_then(|v| v.as_u64()) {
            def.max_turns = mt as u32;
        }

        let known = [
            "name",
            "description",
            "model",
            "tools",
            "maxTurns",
            "disallowedTools",
            "permissionMode",
            "skills",
            "mcpServers",
            "hooks",
            "memory",
            "background",
            "effort",
            "isolation",
            "color",
            "initialPrompt",
        ];
        let mut metadata = serde_json::Map::new();
        for (key, value) in &fm {
            if let Some(key_str) = key.as_str() {
                if !known.contains(&key_str) {
                    if let Ok(json_value) = serde_json::to_value(value) {
                        metadata.insert(key_str.to_string(), json_value);
                    }
                }
            }
        }
        def.metadata = serde_json::Value::Object(metadata);
    }

    if def.name.is_empty() {
        def.name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();
    }

    def.hash = def.compute_hash();
    Ok(Some(def))
}

pub(crate) fn split_yaml_frontmatter(content: &str) -> (Option<serde_yaml::Mapping>, &str) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (None, content);
    }
    let after_first = &trimmed[3..];
    let Some(end) = after_first.find("\n---") else {
        return (None, content);
    };
    let yaml_str = &after_first[..end];
    let body = &after_first[end + 4..];
    let mapping = serde_yaml::from_str::<serde_yaml::Mapping>(yaml_str).ok();
    (mapping, body)
}

pub(crate) fn map_claude_model_to_tier(model: &str) -> String {
    match model.to_lowercase().as_str() {
        "opus" => "strong".into(),
        "sonnet" => "balanced".into(),
        "haiku" => "light".into(),
        _ => "balanced".into(),
    }
}

pub(crate) fn infer_profile_from_tools(
    tools: &[serde_yaml::Value],
) -> super::super::definition::ToolProfile {
    let tool_names: Vec<&str> = tools.iter().filter_map(|v| v.as_str()).collect();
    let has_write = tool_names
        .iter()
        .any(|t| matches!(*t, "Edit" | "Write" | "Bash"));
    if has_write {
        super::super::definition::ToolProfile::ReadWrite
    } else {
        super::super::definition::ToolProfile::Read
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subagent::definition::{DefinitionSource, SubagentDefinition, ToolProfile};

    #[test]
    fn parse_yaml_frontmatter_basic() {
        let input = "---\nname: review\ndescription: Review code\n---\nReview carefully.\n";
        let (fm, body) = split_yaml_frontmatter(input);
        let fm = fm.expect("should parse frontmatter");
        assert_eq!(fm.get("name").and_then(|v| v.as_str()), Some("review"));
        assert_eq!(
            fm.get("description").and_then(|v| v.as_str()),
            Some("Review code")
        );
        assert_eq!(body.trim(), "Review carefully.");
    }

    #[test]
    fn parse_yaml_frontmatter_no_delimiters_returns_none() {
        let input = "Just a plain text file.\nNo frontmatter here.\n";
        let (fm, body) = split_yaml_frontmatter(input);
        assert!(fm.is_none());
        assert_eq!(body, input);
    }

    #[test]
    fn parse_yaml_frontmatter_only_opening_delimiter_returns_none() {
        let input = "---\nname: x\nNo closing delimiter";
        let (fm, _) = split_yaml_frontmatter(input);
        assert!(fm.is_none());
    }

    #[test]
    fn map_model_to_tier_converts_claude_models() {
        assert_eq!(map_claude_model_to_tier("opus"), "strong");
        assert_eq!(map_claude_model_to_tier("sonnet"), "balanced");
        assert_eq!(map_claude_model_to_tier("haiku"), "light");
        assert_eq!(map_claude_model_to_tier("unknown"), "balanced");
    }

    #[test]
    fn infer_profile_read_from_empty_or_read_tools() {
        let tools: Vec<serde_yaml::Value> = vec![];
        assert_eq!(infer_profile_from_tools(&tools), ToolProfile::Read);

        let read_tools = vec![
            serde_yaml::Value::String("Read".into()),
            serde_yaml::Value::String("Grep".into()),
            serde_yaml::Value::String("Glob".into()),
        ];
        assert_eq!(infer_profile_from_tools(&read_tools), ToolProfile::Read);
    }

    #[test]
    fn infer_profile_read_write_from_edit_or_bash_tools() {
        let tools = vec![
            serde_yaml::Value::String("Read".into()),
            serde_yaml::Value::String("Edit".into()),
        ];
        assert_eq!(infer_profile_from_tools(&tools), ToolProfile::ReadWrite);

        let bash_tools = vec![serde_yaml::Value::String("Bash".into())];
        assert_eq!(
            infer_profile_from_tools(&bash_tools),
            ToolProfile::ReadWrite
        );
    }

    #[test]
    fn parse_claude_code_agent_from_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let agent_path = dir.path().join("review.md");
        std::fs::write(
            &agent_path,
            "---\nname: my-review\ndescription: Custom review\nmodel: opus\ntools:\n  - Read\n  - Grep\nmaxTurns: 5\n---\n\nBe thorough.\n",
        )
        .unwrap();

        let def = parse_claude_code_agent(&agent_path, DefinitionSource::ClaudeCodeProject)
            .unwrap()
            .expect("should parse");
        assert_eq!(def.name, "my-review");
        assert_eq!(def.description, "Custom review");
        assert_eq!(def.tier, "strong");
        assert_eq!(def.tool_profile, ToolProfile::Read);
        assert_eq!(def.max_turns, 5);
        assert_eq!(def.instructions.trim(), "Be thorough.");
        assert_eq!(def.source, DefinitionSource::ClaudeCodeProject);
        assert!(def.source_path.is_some());
        assert!(def.hash.starts_with("sha256:"));
    }

    #[test]
    fn parse_claude_code_agent_falls_back_to_filename() {
        let dir = tempfile::tempdir().unwrap();
        let agent_path = dir.path().join("custom-agent.md");
        std::fs::write(&agent_path, "No frontmatter, just instructions.\n").unwrap();

        let def = parse_claude_code_agent(&agent_path, DefinitionSource::ClaudeCodeUser)
            .unwrap()
            .expect("should parse");
        assert_eq!(def.name, "custom-agent");
        assert_eq!(
            def.instructions.trim(),
            "No frontmatter, just instructions."
        );
    }

    #[test]
    fn load_from_dir_skips_non_md_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.md"), "---\nname: a\n---\nBody").unwrap();
        std::fs::write(dir.path().join("b.txt"), "---\nname: b\n---\nBody").unwrap();
        let defs = load_from_dir(dir.path(), DefinitionSource::ClaudeCodeUser).unwrap();
        assert_eq!(defs.len(), 1);
    }
}
