use std::path::Path;

use crate::util::yaml::split_yaml_frontmatter;

use super::definition::{AgentDefinition, DefinitionSource, ToolProfile};

pub(crate) fn load_from_dir(
    dir: &Path,
    source: DefinitionSource,
) -> crate::error::Result<Vec<AgentDefinition>> {
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
) -> Option<AgentDefinition> {
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

    let mut def = AgentDefinition {
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
        "read" | "Read" => Some(ToolProfile::Read),
        "read_write" | "readwrite" | "ReadWrite" | "write" | "Write" => {
            Some(ToolProfile::ReadWrite)
        }
        "none" | "None" => Some(ToolProfile::None),
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

/// Split YAML frontmatter (between --- markers) from the body.
/// Returns (Some(frontmatter_str), body_str) or (None, full_text).
pub fn split_frontmatter(text: &str) -> (Option<String>, String) {
    let text = text.trim();
    if let Some(rest) = text.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            let fm = rest[..end].trim().to_string();
            let body = rest[end + 4..].trim().to_string();
            return (Some(fm), body);
        }
    }
    (None, text.to_string())
}

/// Parse agent definition from YAML frontmatter and body.
pub fn parse_agent_frontmatter(name: &str, fm: &str, body: &str) -> Option<AgentDefinition> {
    let parsed: serde_yaml::Value = serde_yaml::from_str(fm).ok()?;
    let map = parsed.as_mapping()?;
    Some(AgentDefinition {
        name: map
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(name)
            .to_string(),
        description: map
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        instructions: body.to_string(),
        tier: map
            .get("tier")
            .and_then(|v| v.as_str())
            .unwrap_or("balanced")
            .to_string(),
        tool_profile: map
            .get("tool_profile")
            .and_then(|v| v.as_str())
            .map(|s| match s {
                "read_write" | "readwrite" | "ReadWrite" | "write" | "Write" => {
                    ToolProfile::ReadWrite
                }
                "none" | "None" => ToolProfile::None,
                _ => ToolProfile::Read,
            })
            .unwrap_or(ToolProfile::Read),
        tools: map.get("tools").and_then(|v| v.as_sequence()).map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        }),
        max_turns: map.get("max_turns").and_then(|v| v.as_u64()).unwrap_or(10) as u32,
        source: DefinitionSource::Builtin,
        hash: String::new(),
        source_path: None,
        metadata: serde_json::Value::Null,
    })
}
