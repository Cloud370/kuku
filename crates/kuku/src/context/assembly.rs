use crate::error::Result;
use crate::prompt::{builtin_prompt_catalog, render_synthetic_user, SyntheticUserTemplateInput};

use super::message::CanonicalMessage;
use super::provenance::FileSource;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentSource {
    pub workspace_path: String,
    pub platform: String,
    pub current_date: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionSource {
    pub path: String,
    pub kind: String,
    pub hash: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemorySource {
    pub path: String,
    pub hash: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContextInput {
    pub environment: EnvironmentSource,
    pub current_task: String,
    pub project_instructions: Vec<InstructionSource>,
    pub global_memory: Option<MemorySource>,
    pub project_memory: Option<MemorySource>,
    pub history: Vec<CanonicalMessage>,
    pub tools: Vec<ToolSchema>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContextAssembly {
    pub system_prompt: String,
    pub prelude_messages: Vec<CanonicalMessage>,
    pub history: Vec<CanonicalMessage>,
    pub tools: Vec<ToolSchema>,
    pub prompt_asset_sources: Vec<FileSource>,
    pub project_instruction_sources: Vec<InstructionSource>,
    pub memory_sources: Vec<MemorySource>,
}

pub fn assemble_context(input: ContextInput) -> Result<ContextAssembly> {
    let catalog = builtin_prompt_catalog();
    let project_instructions_text = if input.project_instructions.is_empty() {
        "No project instructions found.".to_string()
    } else {
        input
            .project_instructions
            .iter()
            .map(|source| source.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n")
    };

    let global_memory_text = input
        .global_memory
        .as_ref()
        .map(|source| source.content.clone())
        .unwrap_or_else(|| "No global memory.".to_string());
    let project_memory_text = input
        .project_memory
        .as_ref()
        .map(|source| source.content.clone())
        .unwrap_or_else(|| "No project memory.".to_string());

    let synthetic_text = render_synthetic_user(
        catalog.synthetic_user.text,
        &SyntheticUserTemplateInput {
            workspace_root: input.environment.workspace_path.clone(),
            platform: input.environment.platform.clone(),
            current_date: input.environment.current_date.clone(),
            project_instructions_rendered: project_instructions_text,
            global_memory_rendered: global_memory_text,
            project_memory_rendered: project_memory_text,
            current_task_rendered: input.current_task.clone(),
        },
    )?;

    let mut memory_sources = Vec::new();
    if let Some(global_memory) = input.global_memory.clone() {
        memory_sources.push(global_memory);
    }
    if let Some(project_memory) = input.project_memory.clone() {
        memory_sources.push(project_memory);
    }

    Ok(ContextAssembly {
        system_prompt: catalog.system.text.to_string(),
        prelude_messages: vec![
            CanonicalMessage::user_text(synthetic_text),
            CanonicalMessage::user_text(catalog.tool_guidance.text),
        ],
        history: input.history,
        tools: input.tools,
        prompt_asset_sources: vec![
            FileSource {
                path: catalog.system.path.to_string(),
                hash: catalog.system.hash,
            },
            FileSource {
                path: catalog.synthetic_user.path.to_string(),
                hash: catalog.synthetic_user.hash,
            },
            FileSource {
                path: catalog.tool_guidance.path.to_string(),
                hash: catalog.tool_guidance.hash,
            },
        ],
        project_instruction_sources: input.project_instructions,
        memory_sources,
    })
}

#[cfg(test)]
mod tests {
    use crate::context::{
        assemble_context, CanonicalMessage, ContextInput, EnvironmentSource, InstructionSource,
        MemorySource, MessageBlock, ToolSchema,
    };
    use serde_json::json;

    fn instruction(path: &str, kind: &str, hash: &str, content: &str) -> InstructionSource {
        InstructionSource {
            path: path.to_string(),
            kind: kind.to_string(),
            hash: hash.to_string(),
            content: content.to_string(),
        }
    }

    fn memory(path: &str, hash: &str, content: &str) -> MemorySource {
        MemorySource {
            path: path.to_string(),
            hash: hash.to_string(),
            content: content.to_string(),
        }
    }

    #[test]
    fn assembles_context_sources_in_documented_order() {
        let project_instructions = vec![
            instruction(
                "/workspace/AGENTS.md",
                "agents",
                "sha256:agents",
                "primary instructions",
            ),
            instruction(
                "/workspace/CLAUDE.md",
                "claude",
                "sha256:claude",
                "compatibility instructions",
            ),
        ];
        let global_memory = memory(
            "/home/user/.kuku/memory.md",
            "sha256:global",
            "global memory",
        );
        let project_memory = memory(
            "/home/user/.kuku/p/workspace/memory.md",
            "sha256:project",
            "project memory",
        );
        let history = vec![CanonicalMessage::user_text("hello")];
        let tools = vec![ToolSchema {
            name: "read".to_string(),
            description: "Read a file".to_string(),
            input_schema: json!({"type": "object"}),
        }];

        let assembly = assemble_context(ContextInput {
            environment: EnvironmentSource {
                workspace_path: "/workspace".to_string(),
                platform: "linux".to_string(),
                current_date: "2026-05-14".to_string(),
            },
            current_task: "hello".to_string(),
            project_instructions: project_instructions.clone(),
            global_memory: Some(global_memory.clone()),
            project_memory: Some(project_memory.clone()),
            history: history.clone(),
            tools: tools.clone(),
        })
        .unwrap();

        assert!(assembly.system_prompt.contains("<kuku_identity>"));
        assert!(assembly.system_prompt.contains("<kuku_hard_rules>"));
        assert!(assembly.system_prompt.contains("<kuku_working_style>"));
        assert_eq!(assembly.prelude_messages.len(), 2);
        assert_eq!(assembly.history, history);
        assert_eq!(assembly.tools, tools);
        assert_eq!(assembly.project_instruction_sources, project_instructions);
        assert_eq!(assembly.memory_sources, vec![global_memory, project_memory]);
        assert_eq!(assembly.prompt_asset_sources.len(), 3);
    }

    #[test]
    fn keeps_empty_optional_sources_without_removing_placeholders() {
        let assembly = assemble_context(ContextInput {
            environment: EnvironmentSource {
                workspace_path: "/workspace".to_string(),
                platform: "windows".to_string(),
                current_date: "2026-05-14".to_string(),
            },
            current_task: "No current task framing.".to_string(),
            project_instructions: Vec::new(),
            global_memory: None,
            project_memory: None,
            history: Vec::new(),
            tools: Vec::new(),
        })
        .unwrap();

        match &assembly.prelude_messages[0].blocks[..] {
            [MessageBlock::Text(text)] => {
                assert!(text.contains("No project instructions found."));
                assert!(text.contains("No global memory."));
                assert!(text.contains("No project memory."));
            }
            other => panic!("expected one synthetic-user text block, got {other:?}"),
        }
    }
}
