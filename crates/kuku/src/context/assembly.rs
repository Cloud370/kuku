use crate::error::Result;
use crate::event::types::ContextMessage;
use crate::prompt::{render_project_context, render_runtime_context, ProjectContextInput};

use super::message::{CanonicalMessage, MessageBlock};
use super::provenance::FileSource;

#[derive(Debug, Clone, PartialEq, Eq)]
/// Runtime environment facts passed into context assembly.
pub struct EnvironmentSource {
    pub workspace_path: String,
    pub platform: String,
    pub current_date: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// A project instruction file (AGENTS.md, CLAUDE.md) with its content and hash.
pub struct InstructionSource {
    pub path: String,
    pub kind: String,
    pub hash: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// A memory file with its content and content hash.
pub struct MemorySource {
    pub path: String,
    pub hash: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq)]
/// Tool definition passed to the provider in the request.
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq)]
/// All sources needed to assemble a provider request context.
pub struct ContextInput {
    pub environment: EnvironmentSource,
    pub project_instructions: Vec<InstructionSource>,
    pub global_memory: Option<MemorySource>,
    pub project_memory: Option<MemorySource>,
    pub history: Vec<CanonicalMessage>,
    pub tools: Vec<ToolSchema>,
    pub model_tiers: Vec<crate::config::TierInfo>,
    /// Optional rendered runtime blocks (agent catalog, notices, etc.).
    /// These go into the runtime_context wrapper in the current user turn.
    pub runtime_blocks: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
/// Assembled system prompt, prelude messages, history, and tool schemas ready for the provider.
pub struct ContextAssembly {
    pub system_prompt: String,
    /// messages[0]: project_context (behavior framework) + messages[1]: tool_guidance
    pub prelude_messages: Vec<CanonicalMessage>,
    pub history: Vec<CanonicalMessage>,
    pub tools: Vec<ToolSchema>,
    pub prompt_asset_sources: Vec<FileSource>,
    pub project_instruction_sources: Vec<InstructionSource>,
    pub memory_sources: Vec<MemorySource>,
    /// Runtime context text injected into the current user turn (before human input).
    /// None if no runtime blocks are present.
    pub runtime_context: Option<String>,
}

impl ContextAssembly {
    /// Snapshot the clean prelude messages (project_context + tool_guidance) before
    /// any turn-specific content (runtime_context) is added.
    pub fn snapshot_prelude(&self) -> Vec<ContextMessage> {
        self.prelude_messages
            .iter()
            .map(|msg| {
                let content = msg
                    .blocks
                    .iter()
                    .map(|b| match b {
                        MessageBlock::Text(t) => t.as_str(),
                        MessageBlock::Thinking(t) => t.as_str(),
                        MessageBlock::ToolUse(_) | MessageBlock::ToolResult(_) => "",
                    })
                    .collect::<Vec<_>>()
                    .join("");
                ContextMessage {
                    role: "user".to_string(),
                    content,
                }
            })
            .collect()
    }
}

/// Build a complete context assembly with A2b two-layer structure.
pub fn assemble_context(
    input: ContextInput,
    catalog: crate::prompt::PromptCatalog,
) -> Result<ContextAssembly> {
    let project_instructions_text = if input.project_instructions.is_empty() {
        "No project instructions found.".to_string()
    } else {
        input
            .project_instructions
            .iter()
            .map(|s| s.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n")
    };

    let global_memory_text = input
        .global_memory
        .as_ref()
        .map(|s| s.content.clone())
        .unwrap_or_else(|| "No global memory.".to_string());
    let project_memory_text = input
        .project_memory
        .as_ref()
        .map(|s| s.content.clone())
        .unwrap_or_else(|| "No project memory.".to_string());

    let model_tiers_text = if input.model_tiers.is_empty() {
        "No model tiers configured.".to_string()
    } else {
        input
            .model_tiers
            .iter()
            .map(|info| format!("{} — {}", info.name, info.purpose))
            .collect::<Vec<_>>()
            .join("\n")
    };

    // Layer 1: project_context (behavior framework) — messages[0]
    let project_context_text = render_project_context(
        &catalog.project_context.text,
        &ProjectContextInput {
            workspace_root: input.environment.workspace_path.clone(),
            platform: input.environment.platform.clone(),
            current_date: input.environment.current_date.clone(),
            project_instructions_rendered: project_instructions_text,
            global_memory_rendered: global_memory_text,
            project_memory_rendered: project_memory_text,
            model_tiers_rendered: model_tiers_text,
        },
    )?;

    // Layer 2: runtime_context (dynamic catalogs + notices) — injected into current user turn
    let runtime_context = input
        .runtime_blocks
        .filter(|blocks| !blocks.is_empty())
        .map(|blocks| render_runtime_context(&catalog.runtime_context.text, &blocks))
        .transpose()?;

    let mut memory_sources = Vec::new();
    if let Some(global_memory) = input.global_memory.clone() {
        memory_sources.push(global_memory);
    }
    if let Some(project_memory) = input.project_memory.clone() {
        memory_sources.push(project_memory);
    }

    Ok(ContextAssembly {
        system_prompt: catalog.system.text,
        prelude_messages: vec![
            CanonicalMessage::user_text(project_context_text),
            CanonicalMessage::user_text(catalog.tool_guidance.text),
        ],
        history: input.history,
        tools: input.tools,
        prompt_asset_sources: vec![
            FileSource {
                path: catalog.system.path,
                hash: catalog.system.hash,
            },
            FileSource {
                path: catalog.project_context.path,
                hash: catalog.project_context.hash,
            },
            FileSource {
                path: catalog.tool_guidance.path,
                hash: catalog.tool_guidance.hash,
            },
        ],
        project_instruction_sources: input.project_instructions,
        memory_sources,
        runtime_context,
    })
}

#[cfg(test)]
mod tests {
    use crate::context::{
        assemble_context, CanonicalMessage, ContextInput, EnvironmentSource, InstructionSource,
        MemorySource, MessageBlock, ToolSchema,
    };
    use crate::prompt::builtin_prompt_catalog;
    use serde_json::json;

    fn instruction(path: &str, kind: &str, hash: &str, content: &str) -> InstructionSource {
        InstructionSource {
            path: path.into(),
            kind: kind.into(),
            hash: hash.into(),
            content: content.into(),
        }
    }

    fn memory(path: &str, hash: &str, content: &str) -> MemorySource {
        MemorySource {
            path: path.into(),
            hash: hash.into(),
            content: content.into(),
        }
    }

    #[test]
    fn a2b_assembles_project_context_as_first_prelude_message() {
        let assembly = assemble_context(
            ContextInput {
                environment: EnvironmentSource {
                    workspace_path: "/ws".into(),
                    platform: "linux".into(),
                    current_date: "2026-05-18".into(),
                },
                project_instructions: vec![instruction(
                    "/ws/AGENTS.md",
                    "agents",
                    "sha:a",
                    "instr",
                )],
                global_memory: Some(memory("mem.md", "sha:m", "mem")),
                project_memory: None,
                history: vec![CanonicalMessage::user_text("hello")],
                tools: vec![ToolSchema {
                    name: "read".into(),
                    description: "r".into(),
                    input_schema: json!({"type": "object"}),
                }],
                model_tiers: vec![],
                runtime_blocks: None,
            },
            builtin_prompt_catalog(),
        )
        .unwrap();

        assert_eq!(assembly.prelude_messages.len(), 2);
        let first = match &assembly.prelude_messages[0].blocks[..] {
            [MessageBlock::Text(t)] => t.clone(),
            other => panic!("expected text, got {other:?}"),
        };
        assert!(
            first.contains("<kuku_project_context>"),
            "messages[0] should be project_context, got: {first}"
        );
        assert!(first.contains("instr"), "should contain project instructions");

        let second = match &assembly.prelude_messages[1].blocks[..] {
            [MessageBlock::Text(t)] => t.clone(),
            other => panic!("expected text, got {other:?}"),
        };
        assert!(
            second.contains("<kuku_tool_guidance>"),
            "messages[1] should be tool_guidance"
        );
    }

    #[test]
    fn a2b_runtime_context_is_separate_from_prelude() {
        let assembly = assemble_context(
            ContextInput {
                environment: EnvironmentSource {
                    workspace_path: "/ws".into(),
                    platform: "linux".into(),
                    current_date: "2026-05-18".into(),
                },
                project_instructions: vec![],
                global_memory: None,
                project_memory: None,
                history: vec![],
                tools: vec![],
                model_tiers: vec![],
                runtime_blocks: Some(
                    "<kuku_agent_catalog><agent name=\"r\"/></kuku_agent_catalog>".into(),
                ),
            },
            builtin_prompt_catalog(),
        )
        .unwrap();

        let rt = assembly
            .runtime_context
            .as_ref()
            .expect("runtime_context should be set");
        assert!(
            rt.contains("<kuku_agent_catalog>"),
            "catalog should be in runtime_context"
        );
        assert!(
            rt.contains("<kuku_runtime_context>"),
            "should wrap in runtime_context template"
        );

        // prelude snapshot must NOT contain runtime_context
        let snapshot = assembly.snapshot_prelude();
        for msg in &snapshot {
            assert!(
                !msg.content.contains("<kuku_agent_catalog>"),
                "prelude must not contain catalog"
            );
        }
    }

    #[test]
    fn a2b_no_runtime_context_when_empty_blocks() {
        let assembly = assemble_context(
            ContextInput {
                environment: EnvironmentSource {
                    workspace_path: "/ws".into(),
                    platform: "linux".into(),
                    current_date: "2026-05-18".into(),
                },
                project_instructions: vec![],
                global_memory: None,
                project_memory: None,
                history: vec![],
                tools: vec![],
                model_tiers: vec![],
                runtime_blocks: None,
            },
            builtin_prompt_catalog(),
        )
        .unwrap();
        assert!(assembly.runtime_context.is_none());
    }
}
