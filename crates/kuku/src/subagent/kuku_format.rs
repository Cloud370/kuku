use std::path::Path;

use crate::error::{Error, Result};

use crate::util::yaml::split_yaml_frontmatter;

use super::definition::{DefinitionSource, SubagentDefinition, ToolProfile};

pub(crate) fn load_from_dir(
    dir: &Path,
    source: DefinitionSource,
) -> Result<Vec<SubagentDefinition>> {
    let mut defs = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("md") {
            let content = std::fs::read_to_string(&path)?;
            match parse_kuku_agent(&content, &path, &source) {
                Ok(def) => defs.push(def),
                Err(e) => {
                    eprintln!("warning: failed to parse agent {}: {e}", path.display());
                }
            }
        }
    }
    Ok(defs)
}

fn parse_kuku_agent(
    content: &str,
    path: &Path,
    source: &DefinitionSource,
) -> Result<SubagentDefinition> {
    let (frontmatter, body) = split_yaml_frontmatter(content);
    let mapping = frontmatter.ok_or_else(|| {
        Error::InvalidArgument(format!("missing YAML frontmatter in {}", path.display()))
    })?;
    let value = serde_yaml::Value::Mapping(mapping);

    let name = value["name"].as_str().map(String::from).unwrap_or_else(|| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unnamed")
            .to_string()
    });

    let description = value["description"].as_str().unwrap_or("").to_string();
    let model = value["model"].as_str().unwrap_or("balanced");
    let tier = map_model_to_tier(model);

    let tools: Option<Vec<String>> = match &value["tools"] {
        serde_yaml::Value::Sequence(seq) => {
            let list: Vec<String> = seq
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
            Some(list)
        }
        _ => None,
    };

    let tool_profile = infer_tool_profile_from_tools(&tools);
    let max_turns: u32 = value["max_turns"].as_u64().map(|n| n as u32).unwrap_or(5);

    let mut def = SubagentDefinition {
        name: name.clone(),
        description,
        instructions: body.trim().to_string(),
        tier,
        tool_profile,
        tools,
        max_turns,
        source: source.clone(),
        hash: String::new(),
        source_path: Some(path.display().to_string()),
        metadata: serde_json::Value::Null,
    };
    def.hash = def.compute_hash();
    Ok(def)
}

fn map_model_to_tier(model: &str) -> String {
    match model {
        "strong" | "opus" => "strong".into(),
        "balanced" | "sonnet" => "balanced".into(),
        "light" | "haiku" => "light".into(),
        other => {
            if other.contains("opus") {
                "strong".into()
            } else if other.contains("haiku") {
                "light".into()
            } else {
                "balanced".into()
            }
        }
    }
}

fn infer_tool_profile_from_tools(tools: &Option<Vec<String>>) -> ToolProfile {
    match tools {
        None => ToolProfile::Read,
        Some(list) if list.is_empty() => ToolProfile::None,
        Some(list) => {
            let has_write = list.iter().any(|t| {
                matches!(
                    t.as_str(),
                    "edit_file"
                        | "write_file"
                        | "run_command"
                        | "remember_memory"
                        | "forget_memory"
                )
            });
            if has_write {
                ToolProfile::ReadWrite
            } else {
                ToolProfile::Read
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_agent_tools_absent_is_inherit() {
        let content = "---\nname: test\n---\n\nDo your thing.\n";
        let def =
            parse_kuku_agent(content, Path::new("test.md"), &DefinitionSource::KukuUser).unwrap();
        assert_eq!(def.tools, None);
        assert_eq!(def.tool_profile, ToolProfile::Read);
    }

    #[test]
    fn parse_agent_empty_tools_is_no_tools() {
        let content = "---\nname: analyst\ntools: []\n---\n\nAnalysis only.\n";
        let def =
            parse_kuku_agent(content, Path::new("a.md"), &DefinitionSource::KukuUser).unwrap();
        assert_eq!(def.tools, Some(vec![]));
        assert_eq!(def.tool_profile, ToolProfile::None);
    }

    #[test]
    fn parse_agent_explicit_tools() {
        let content = "---\nname: reader\ntools: [find_files, read_file]\n---\n\nRead only.\n";
        let def =
            parse_kuku_agent(content, Path::new("r.md"), &DefinitionSource::KukuProject).unwrap();
        assert_eq!(
            def.tools,
            Some(vec!["find_files".into(), "read_file".into()])
        );
        assert_eq!(def.tool_profile, ToolProfile::Read);
    }

    #[test]
    fn parse_agent_with_write_tools_infers_read_write() {
        let content = "---\nname: writer\ntools: [find_files, edit_file]\n---\n\nWrite stuff.\n";
        let def =
            parse_kuku_agent(content, Path::new("w.md"), &DefinitionSource::KukuUser).unwrap();
        assert_eq!(def.tool_profile, ToolProfile::ReadWrite);
    }

    #[test]
    fn name_defaults_to_filename_stem() {
        let content = "---\ndescription: test\n---\n\nBody.\n";
        let def = parse_kuku_agent(
            content,
            Path::new("my-agent.md"),
            &DefinitionSource::KukuUser,
        )
        .unwrap();
        assert_eq!(def.name, "my-agent");
    }
}
