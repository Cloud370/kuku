use crate::error::Result;
use crate::event::types::{ContextMessage, EventPayload, StoredEvent};
use crate::prompt::{render_project_context, ProjectContextInput};

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
pub struct HostResponseContract {
    pub surface: String,
    pub locale: String,
    pub preferences: Option<String>,
}

/// All sources needed to assemble a provider request context.
#[derive(Debug, Clone, PartialEq)]
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
    pub enable_memory: bool,
    pub agent_name: String,
    pub agent_instructions: String,
    pub response_contract: Option<HostResponseContract>,
}

#[derive(Debug, Clone, PartialEq)]
/// Assembled system prompt, prelude messages, history, and tool schemas ready for the provider.
pub struct ContextAssembly {
    pub system_prompt: String,
    /// Snapshot prelude messages (layers 2-6): project_policy, identity,
    /// catalog+skills (injected by caller), tool_guidance, memory*.
    pub prelude_messages: Vec<CanonicalMessage>,
    pub history: Vec<CanonicalMessage>,
    pub tools: Vec<ToolSchema>,
    pub prompt_asset_sources: Vec<FileSource>,
    pub project_instruction_sources: Vec<InstructionSource>,
    pub memory_sources: Vec<MemorySource>,
    /// Runtime context text injected into the current user turn (before human input).
    /// None if no runtime blocks are present.
    pub runtime_context: Option<String>,
    /// Handoff summary from the most recent handoff event, if any.
    /// Set by the caller after `rebuild_history()` returns it.
    pub handoff_summary: Option<String>,
}

impl ContextAssembly {
    /// Snapshot the clean prelude messages (layers 2-6)
    /// before any turn-specific content is added.
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

/// Restore frozen prompt snapshot messages for a conversation.
pub fn restore_prompt_snapshot(
    events: &[StoredEvent],
    conversation: &str,
) -> Option<Vec<CanonicalMessage>> {
    let filtered = super::revert::filter_rolled_back_events(events);
    let prelude = filtered.iter().rev().find_map(|ev| match &ev.payload {
        EventPayload::PromptSnapshot {
            conversation: event_conversation,
            messages,
            ..
        } if event_conversation == conversation => Some(messages),
        _ => None,
    })?;

    Some(
        prelude
            .iter()
            .map(|cm| CanonicalMessage::user_text(&cm.content))
            .collect(),
    )
}

/// Build a complete context assembly with 6-layer snapshot.
///
/// Layers:
///   1. system_prompt (catalog.system.text)
///   2. project_policy (blocks["project-policy"] + render_project_context)
///   3. agent identity (input.agent_instructions)
///   4. tool_guidance (blocks["tool-guidance"])
///   5. memory (optional): blocks["memory"] + memory["global"] + memory["project"]
///   6. agent catalog + loaded skills (appended by caller after memory blocks)
///
/// Per-turn runtime_context is built from input.runtime_blocks (and
/// optionally response_contract) by wrapping with the catalog's
/// runtime/context template. The caller injects it into the current user turn.
pub fn assemble_context(
    input: ContextInput,
    catalog: &crate::prompt::PromptCatalog,
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

    // ---- Layer 2: project_policy ----
    let project_policy = catalog.blocks.get("project-policy").ok_or_else(|| {
        crate::error::Error::PromptRender("missing project-policy template".into())
    })?;
    let project_policy_text = render_project_context(
        &project_policy.text,
        &ProjectContextInput {
            workspace_root: input.environment.workspace_path.clone(),
            platform: input.environment.platform.clone(),
            current_date: input.environment.current_date.clone(),
            project_instructions_rendered: project_instructions_text,
            model_tiers_rendered: model_tiers_text,
        },
    )?;

    // ---- Layer 3: agent identity ----
    let identity_text = input.agent_instructions.clone();

    // ---- Layer 4: agent catalog + loaded skills ----
    // (injected by caller via prelude push during Phase 7)

    // ---- Layer 5: tool-guidance ----
    let tool_guidance = catalog
        .blocks
        .get("tool-guidance")
        .map(|a| a.text.clone())
        .unwrap_or_default();

    // ---- Layer 6: memory (optional) ----
    let memory_guidance = if input.enable_memory {
        catalog.blocks.get("memory").map(|a| a.text.clone())
    } else {
        None
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

    let global_rendered = if input.enable_memory {
        catalog
            .memory
            .get("global")
            .map(|a| a.text.replace("{{memory_content}}", &global_memory_text))
            .unwrap_or_default()
    } else {
        String::new()
    };
    let project_rendered = if input.enable_memory {
        catalog
            .memory
            .get("project")
            .map(|a| a.text.replace("{{memory_content}}", &project_memory_text))
            .unwrap_or_default()
    } else {
        String::new()
    };

    // ---- Runtime context (per-turn) ----
    let runtime_blocks = if let Some(ref contract) = input.response_contract {
        let mut parts = Vec::new();
        parts.push(format!(
            "<kuku_response_contract>\nsurface: {}\nlocale: {}\npreferences: {}\n</kuku_response_contract>",
            contract.surface,
            contract.locale,
            contract.preferences.as_deref().unwrap_or(""),
        ));
        if let Some(ref blocks) = input.runtime_blocks {
            parts.push(blocks.clone());
        }
        Some(parts.join("\n"))
    } else {
        input.runtime_blocks.clone()
    };
    let runtime_context = runtime_blocks
        .filter(|blocks| !blocks.is_empty())
        .map(|blocks| {
            let wrapper = catalog
                .runtime
                .get("context")
                .map(|a| a.text.clone())
                .unwrap_or_else(|| {
                    "<kuku_runtime_context>\n{{runtime_blocks}}\n</kuku_runtime_context>"
                        .to_string()
                });
            Ok::<_, crate::error::Error>(wrapper.replace("{{runtime_blocks}}", &blocks))
        })
        .transpose()?;

    // ---- Assemble prelude messages (snapshot layers 2-6) ----
    let mut prelude = Vec::new();
    if !project_policy_text.is_empty() {
        prelude.push(CanonicalMessage::user_text(project_policy_text));
    }
    if !identity_text.is_empty() {
        prelude.push(CanonicalMessage::user_text(identity_text));
    }
    // Layer 4 (catalog + skills) injected by caller via prelude push
    if !tool_guidance.is_empty() {
        prelude.push(CanonicalMessage::user_text(tool_guidance));
    }
    if let Some(mem) = memory_guidance {
        if !mem.is_empty() {
            prelude.push(CanonicalMessage::user_text(mem));
        }
    }
    if !global_rendered.is_empty() {
        prelude.push(CanonicalMessage::user_text(global_rendered));
    }
    if !project_rendered.is_empty() {
        prelude.push(CanonicalMessage::user_text(project_rendered));
    }

    // ---- Sources for provenance ----
    let mut prompt_asset_sources: Vec<FileSource> = vec![FileSource {
        path: catalog.system.path.clone(),
        hash: catalog.system.hash.clone(),
    }];
    for key in &["project-policy", "tool-guidance", "memory"] {
        if let Some(a) = catalog.blocks.get(*key) {
            prompt_asset_sources.push(FileSource {
                path: a.path.clone(),
                hash: a.hash.clone(),
            });
        }
    }

    let mut memory_sources = Vec::new();
    if let Some(gm) = input.global_memory.clone() {
        memory_sources.push(gm);
    }
    if let Some(pm) = input.project_memory.clone() {
        memory_sources.push(pm);
    }

    Ok(ContextAssembly {
        system_prompt: catalog.system.text.clone(),
        prelude_messages: prelude,
        history: input.history,
        tools: input.tools,
        prompt_asset_sources,
        project_instruction_sources: input.project_instructions,
        memory_sources,
        runtime_context, // per-turn runtime blocks (consumed by query provider)
        handoff_summary: None,
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
    fn assembles_six_layer_prelude_with_memory_enabled() {
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
                enable_memory: true,
                agent_name: "main".into(),
                agent_instructions: String::new(),
                response_contract: None,
            },
            &builtin_prompt_catalog(),
        )
        .unwrap();

        // With enable_memory=true and empty identity:
        // [0] project_policy, [1] tool_guidance, [2] memory_guidance,
        // [3] global_memory, [4] project_memory
        assert_eq!(assembly.prelude_messages.len(), 5);

        // [0] project_policy
        let msg0 = match &assembly.prelude_messages[0].blocks[..] {
            [MessageBlock::Text(t)] => t.clone(),
            other => panic!("expected text, got {other:?}"),
        };
        assert!(msg0.contains("<kuku_project_context>"));
        assert!(msg0.contains("instr"));

        // [1] tool_guidance
        let msg1 = match &assembly.prelude_messages[1].blocks[..] {
            [MessageBlock::Text(t)] => t.clone(),
            other => panic!("expected text, got {other:?}"),
        };
        assert!(msg1.contains("<kuku_tool_guidance>"));

        // [2] memory_guidance
        let msg2 = match &assembly.prelude_messages[2].blocks[..] {
            [MessageBlock::Text(t)] => t.clone(),
            other => panic!("expected text, got {other:?}"),
        };
        assert!(msg2.contains("<kuku_memory_guidance>"));

        // [3] global_memory
        let msg3 = match &assembly.prelude_messages[3].blocks[..] {
            [MessageBlock::Text(t)] => t.clone(),
            other => panic!("expected text, got {other:?}"),
        };
        assert!(msg3.contains("<kuku_global_memory>"));
        assert!(msg3.contains("mem"));

        // [4] project_memory (no content but template still rendered)
        let msg4 = match &assembly.prelude_messages[4].blocks[..] {
            [MessageBlock::Text(t)] => t.clone(),
            other => panic!("expected text, got {other:?}"),
        };
        assert!(msg4.contains("<kuku_project_memory>"));
    }

    #[test]
    fn memory_disabled_skips_memory_layers() {
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
                enable_memory: false,
                agent_name: "main".into(),
                agent_instructions: String::new(),
                response_contract: None,
            },
            &builtin_prompt_catalog(),
        )
        .unwrap();

        // Without memory and empty identity: only project_policy + tool_guidance
        assert_eq!(assembly.prelude_messages.len(), 2);
    }

    #[test]
    fn identity_added_as_prelude_when_provided() {
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
                enable_memory: false,
                agent_name: "review".into(),
                agent_instructions: "<kuku_identity>code reviewer</kuku_identity>".into(),
                response_contract: None,
            },
            &builtin_prompt_catalog(),
        )
        .unwrap();

        // project_policy, identity, tool_guidance = 3 messages
        assert_eq!(assembly.prelude_messages.len(), 3);

        // [1] should be identity
        let msg1 = match &assembly.prelude_messages[1].blocks[..] {
            [MessageBlock::Text(t)] => t.clone(),
            other => panic!("expected text, got {other:?}"),
        };
        assert_eq!(msg1, "<kuku_identity>code reviewer</kuku_identity>");
    }

    #[test]
    fn runtime_context_not_in_prelude_snapshot() {
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
                enable_memory: false,
                agent_name: "main".into(),
                agent_instructions: String::new(),
                response_contract: None,
            },
            &builtin_prompt_catalog(),
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

        // prelude snapshot must NOT contain runtime context
        let snapshot = assembly.snapshot_prelude();
        for msg in &snapshot {
            assert!(
                !msg.content.contains("<kuku_agent_catalog>"),
                "prelude must not contain catalog"
            );
        }
    }

    #[test]
    fn runtime_context_none_when_empty() {
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
                enable_memory: false,
                agent_name: "main".into(),
                agent_instructions: String::new(),
                response_contract: None,
            },
            &builtin_prompt_catalog(),
        )
        .unwrap();
        assert!(assembly.runtime_context.is_none());
    }
}
