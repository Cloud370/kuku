use crate::error::Result;
use crate::event::types::{ContextMessage, EventPayload, StoredEvent};

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

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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
    pub models: Vec<crate::config::ModelInfo>,
    /// Optional rendered runtime blocks (agent catalog, notices, etc.).
    /// These go into the runtime_context wrapper in the current user turn.
    pub runtime_blocks: Option<String>,
    pub enable_memory: bool,
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
                        MessageBlock::ToolUse(_)
                        | MessageBlock::ToolResult(_)
                        | MessageBlock::InputImage(_)
                        | MessageBlock::ProviderReplay { .. } => "",
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

/// Assemble the prelude for a standalone Query.
pub fn assemble_context(
    input: ContextInput,
    catalog: &crate::prompt::PromptCatalog,
) -> Result<ContextAssembly> {
    assemble_context_for_model(input, catalog, agent_prompt_facts(None, false))
}

pub(crate) fn assemble_context_for_model(
    input: ContextInput,
    catalog: &crate::prompt::PromptCatalog,
    agent: minijinja::Value,
) -> Result<ContextAssembly> {
    assemble_context_inner(input, catalog, false, agent).map(|(assembly, _)| assembly)
}

pub(crate) fn agent_prompt_facts(
    resolved: Option<&crate::provider::types::ResolvedProvider>,
    builtin_adapter: bool,
) -> minijinja::Value {
    use crate::config::ImageInputSupport;
    let image_input = resolved.map_or(ImageInputSupport::Unconfirmed, |route| {
        route.capabilities.image_input_support(builtin_adapter)
    });
    let declared_input = resolved
        .and_then(|route| route.capabilities.input_modalities.as_ref())
        .map(|entry| entry.value.as_slice());
    let declared_output = resolved
        .and_then(|route| route.capabilities.output_modalities.as_ref())
        .map(|entry| entry.value.as_slice());
    minijinja::context! {
        model => minijinja::context! {
            input_modalities => minijinja::value::Serde(declared_input),
            output_modalities => minijinja::value::Serde(declared_output),
        },
        image_input => minijinja::context! {
            supported => image_input == ImageInputSupport::Supported,
            status => minijinja::value::Serde(image_input),
        },
        effective_input_modalities => minijinja::value::Serde(image_input.modalities()),
        output => minijinja::value::Serde(resolved.map(|route| route.output).unwrap_or_default()),
    }
}

pub(crate) fn render_modalities(modalities: Option<&[String]>) -> String {
    modalities.map_or_else(
        || "unknown".into(),
        |values| serde_json::json!(values).to_string(),
    )
}

#[derive(serde::Serialize)]
struct PromptModel<'a> {
    connection_text: String,
    model_text: String,
    #[serde(flatten)]
    info: &'a crate::config::ModelInfo,
    effective_input_modalities: &'static [&'static str],
    input_modalities_text: String,
    output_modalities_text: String,
    image_input_label: &'static str,
}

fn segment_facts(
    input: &ContextInput,
    catalog: &crate::prompt::PromptCatalog,
    renderer: &crate::prompt::PromptRenderer,
    agent: minijinja::Value,
) -> Result<minijinja::Value> {
    let tools: Vec<_> = input.tools.iter().map(|tool| tool.name.as_str()).collect();
    let models = (tools.contains(&"create_agent") || tools.contains(&"fetch_web")).then(|| {
        input
            .models
            .iter()
            .map(|info| PromptModel {
                connection_text: serde_json::json!(info.connection).to_string(),
                model_text: serde_json::json!(info.model).to_string(),
                info,
                effective_input_modalities: info.image_input.modalities(),
                input_modalities_text: render_modalities(info.input_modalities.as_deref()),
                output_modalities_text: render_modalities(info.output_modalities.as_deref()),
                image_input_label: info.image_input.label(),
            })
            .collect::<Vec<_>>()
    });
    let project_instructions: Vec<_> = input
        .project_instructions
        .iter()
        .map(|source| source.content.as_str())
        .collect();
    let models_rendered = if models.is_some()
        && catalog
            .blocks
            .get("project-policy")
            .is_some_and(|asset| asset.text.contains("models_rendered"))
    {
        renderer
            .render(
                "blocks/models.md",
                minijinja::context! { models => minijinja::value::Serde(&models), agent => agent.clone(), tools => minijinja::value::Serde(&tools) },
            )?
            .trim_end()
            .to_owned()
    } else {
        String::new()
    };
    let project_instructions_rendered = if project_instructions.is_empty() {
        "No project instructions found.".to_owned()
    } else {
        project_instructions.join("\n\n")
    };
    Ok(minijinja::context! {
        agent => agent,
        tools => minijinja::value::Serde(&tools),
        models => minijinja::value::Serde(&models),
        workspace_root => &input.environment.workspace_path,
        platform => &input.environment.platform,
        current_date => &input.environment.current_date,
        project_instructions => minijinja::value::Serde(&project_instructions),
        project_instructions_rendered => project_instructions_rendered,
        models_rendered => models_rendered,
        memory_enabled => input.enable_memory,
    })
}

pub(crate) fn assemble_segment_context(
    input: ContextInput,
    catalog: &crate::prompt::PromptCatalog,
    agent: minijinja::Value,
) -> Result<(ContextAssembly, usize)> {
    assemble_context_inner(input, catalog, true, agent)
}

fn assemble_context_inner(
    input: ContextInput,
    catalog: &crate::prompt::PromptCatalog,
    common_first: bool,
    agent: minijinja::Value,
) -> Result<(ContextAssembly, usize)> {
    let renderer = catalog.renderer();
    let facts = segment_facts(&input, catalog, &renderer, agent)?;
    let project_policy_text = renderer.render("blocks/project-policy.md", facts.clone())?;

    // ---- Layer 3: agent identity ----
    let identity_text = input.agent_instructions.clone();

    let has_tool = |name: &str| input.tools.iter().any(|tool| tool.name == name);
    let guidance = [
        ("tool-guidance", !input.tools.is_empty()),
        (
            "delegation",
            ["create_agent", "send_message", "list_agents"]
                .iter()
                .any(|name| has_tool(name)),
        ),
        (
            "memory",
            input.enable_memory && (has_tool("remember_memory") || has_tool("forget_memory")),
        ),
    ];
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
        renderer.render(
            "memory/global.md",
            minijinja::context! {
                memory_content => global_memory_text,
                ..facts.clone()
            },
        )?
    } else {
        String::new()
    };
    let project_rendered = if input.enable_memory {
        renderer.render(
            "memory/project.md",
            minijinja::context! {
                memory_content => project_memory_text,
                ..facts.clone()
            },
        )?
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
            renderer.render(
                "runtime/context.md",
                minijinja::context! {
                    runtime_blocks => blocks,
                    ..facts.clone()
                },
            )
        })
        .transpose()?;

    // ---- Assemble prelude messages (snapshot layers 2-6) ----
    let mut prelude = Vec::new();
    let mut rendered_guidance = Vec::new();
    for (key, _) in guidance.iter().filter(|(_, selected)| *selected) {
        if let Some(asset) = catalog.blocks.get(*key) {
            let text = renderer.render(&format!("blocks/{key}.md"), facts.clone())?;
            if !text.trim().is_empty() {
                prelude.push(CanonicalMessage::user_text(text));
                rendered_guidance.push(asset);
            }
        }
    }
    let identity_slot = if common_first {
        let slot = prelude.len();
        if !identity_text.is_empty() {
            prelude.push(CanonicalMessage::user_text(identity_text));
        }
        if !project_policy_text.is_empty() {
            prelude.push(CanonicalMessage::user_text(project_policy_text));
        }
        slot
    } else {
        let mut leading = Vec::new();
        if !project_policy_text.is_empty() {
            leading.push(CanonicalMessage::user_text(project_policy_text));
        }
        let slot = leading.len();
        if !identity_text.is_empty() {
            leading.push(CanonicalMessage::user_text(identity_text));
        }
        leading.append(&mut prelude);
        prelude = leading;
        slot
    };
    if !global_rendered.is_empty() {
        prelude.push(CanonicalMessage::user_text(global_rendered));
    }
    if !project_rendered.is_empty() {
        prelude.push(CanonicalMessage::user_text(project_rendered));
    }
    // Layer 6 (agent catalog + loaded skills) is appended by the caller after
    // this prelude is built, immediately following the memory blocks above.

    // ---- Sources for provenance ----
    let mut prompt_asset_sources: Vec<FileSource> = vec![FileSource {
        path: catalog.system.path.clone(),
        hash: catalog.system.hash.clone(),
    }];
    for asset in catalog
        .blocks
        .get("project-policy")
        .into_iter()
        .chain(rendered_guidance)
    {
        prompt_asset_sources.push(FileSource {
            path: asset.path.clone(),
            hash: asset.hash.clone(),
        });
    }

    let mut memory_sources = Vec::new();
    if let Some(gm) = input.global_memory.clone() {
        memory_sources.push(gm);
    }
    if let Some(pm) = input.project_memory.clone() {
        memory_sources.push(pm);
    }

    Ok((
        ContextAssembly {
            system_prompt: renderer.render("system.md", facts)?,
            prelude_messages: prelude,
            history: input.history,
            tools: input.tools,
            prompt_asset_sources,
            project_instruction_sources: input.project_instructions,
            memory_sources,
            runtime_context, // per-turn runtime blocks (consumed by query provider)
            handoff_summary: None,
        },
        identity_slot,
    ))
}

#[cfg(test)]
mod tests {
    use crate::context::{
        CanonicalMessage, ContextInput, EnvironmentSource, InstructionSource, MemorySource,
        MessageBlock, ToolSchema, assemble_context,
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
                    name: "remember_memory".into(),
                    description: "r".into(),
                    input_schema: json!({"type": "object"}),
                }],
                models: vec![],
                runtime_blocks: None,
                enable_memory: true,
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
        assert!(!msg0.contains("Current date:"));

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
                models: vec![],
                runtime_blocks: None,
                enable_memory: false,
                agent_instructions: String::new(),
                response_contract: None,
            },
            &builtin_prompt_catalog(),
        )
        .unwrap();

        assert_eq!(assembly.prelude_messages.len(), 1);
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
                models: vec![],
                runtime_blocks: None,
                enable_memory: false,
                agent_instructions: "<kuku_identity>code reviewer</kuku_identity>".into(),
                response_contract: None,
            },
            &builtin_prompt_catalog(),
        )
        .unwrap();

        assert_eq!(assembly.prelude_messages.len(), 2);

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
                models: vec![],
                runtime_blocks: Some(
                    "<kuku_agent_catalog><agent name=\"r\"/></kuku_agent_catalog>".into(),
                ),
                enable_memory: false,
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
                models: vec![],
                runtime_blocks: None,
                enable_memory: false,
                agent_instructions: String::new(),
                response_contract: None,
            },
            &builtin_prompt_catalog(),
        )
        .unwrap();
        assert!(assembly.runtime_context.is_none());
    }
}
