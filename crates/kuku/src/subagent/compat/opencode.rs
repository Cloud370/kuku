use std::path::Path;

use crate::error::Result;

use super::super::definition::{DefinitionSource, SubagentDefinition};

/// Load OpenCode agent definitions from a directory.
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
        if let Some(def) = parse_opencode_agent(&path, source.clone())? {
            definitions.push(def);
        }
    }
    Ok(definitions)
}

fn parse_opencode_agent(
    path: &Path,
    source: DefinitionSource,
) -> Result<Option<SubagentDefinition>> {
    let content = std::fs::read_to_string(path)?;
    let (frontmatter, body) = super::claude_code::split_yaml_frontmatter(&content);

    let mut def = SubagentDefinition {
        name: String::new(),
        description: String::new(),
        instructions: body.trim().to_string(),
        tier: "balanced".into(),
        tool_profile: super::super::definition::ToolProfile::Read,
        tools: None,
        max_turns: 10,
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
            def.tier = super::claude_code::map_claude_model_to_tier(model);
        }
        if let Some(tools) = fm.get("tools").and_then(|v| v.as_sequence()) {
            def.tool_profile = super::claude_code::infer_profile_from_tools(tools);
        }
        let known = ["name", "description", "model", "tools", "mode"];
        let mut meta = serde_json::Map::new();
        for (key, value) in &fm {
            if let Some(key_str) = key.as_str() {
                if !known.contains(&key_str) {
                    if let Ok(json_value) = serde_json::to_value(value) {
                        meta.insert(key_str.to_string(), json_value);
                    }
                }
            }
        }
        if let Some(mode) = fm.get("mode").and_then(|v| v.as_str()) {
            meta.insert("mode".into(), serde_json::Value::String(mode.into()));
        }
        def.metadata = serde_json::Value::Object(meta);
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
