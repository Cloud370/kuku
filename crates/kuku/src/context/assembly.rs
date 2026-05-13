use super::message::CanonicalMessage;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionSource {
    pub path: String,
    pub kind: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemorySource {
    pub path: String,
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
    pub project_instructions: Vec<InstructionSource>,
    pub global_memory: Option<MemorySource>,
    pub project_memory: Option<MemorySource>,
    pub history: Vec<CanonicalMessage>,
    pub tools: Vec<ToolSchema>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ContextSource {
    ProjectInstructions(Vec<InstructionSource>),
    GlobalMemory(MemorySource),
    ProjectMemory(MemorySource),
    History(Vec<CanonicalMessage>),
    Tools(Vec<ToolSchema>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContextAssembly {
    pub sources: Vec<ContextSource>,
}

pub fn assemble_context(input: ContextInput) -> ContextAssembly {
    let mut sources = Vec::new();

    if !input.project_instructions.is_empty() {
        sources.push(ContextSource::ProjectInstructions(
            input.project_instructions,
        ));
    }

    if let Some(global_memory) = input.global_memory {
        sources.push(ContextSource::GlobalMemory(global_memory));
    }

    if let Some(project_memory) = input.project_memory {
        sources.push(ContextSource::ProjectMemory(project_memory));
    }

    sources.push(ContextSource::History(input.history));
    sources.push(ContextSource::Tools(input.tools));

    ContextAssembly { sources }
}

#[cfg(test)]
mod tests {
    use crate::context::{
        assemble_context, CanonicalMessage, ContextInput, ContextSource, InstructionSource,
        MemorySource, ToolSchema,
    };
    use serde_json::json;

    fn instruction(path: &str, kind: &str, content: &str) -> InstructionSource {
        InstructionSource {
            path: path.to_string(),
            kind: kind.to_string(),
            content: content.to_string(),
        }
    }

    fn memory(path: &str, content: &str) -> MemorySource {
        MemorySource {
            path: path.to_string(),
            content: content.to_string(),
        }
    }

    #[test]
    fn assembles_context_sources_in_documented_order() {
        let project_instructions = vec![
            instruction("/workspace/AGENTS.md", "agents", "primary instructions"),
            instruction(
                "/workspace/CLAUDE.md",
                "claude",
                "compatibility instructions",
            ),
        ];
        let global_memory = memory("/home/user/.kuku/memory.md", "global memory");
        let project_memory = memory("/home/user/.kuku/p/workspace/memory.md", "project memory");
        let history = vec![CanonicalMessage::user_text("hello")];
        let tools = vec![ToolSchema {
            name: "read".to_string(),
            description: "Read a file".to_string(),
            input_schema: json!({"type": "object"}),
        }];

        let assembly = assemble_context(ContextInput {
            project_instructions: project_instructions.clone(),
            global_memory: Some(global_memory.clone()),
            project_memory: Some(project_memory.clone()),
            history: history.clone(),
            tools: tools.clone(),
        });

        assert_eq!(
            assembly.sources,
            vec![
                ContextSource::ProjectInstructions(project_instructions),
                ContextSource::GlobalMemory(global_memory),
                ContextSource::ProjectMemory(project_memory),
                ContextSource::History(history),
                ContextSource::Tools(tools),
            ]
        );
    }

    #[test]
    fn omits_empty_optional_sources_without_reordering_history_or_tools() {
        let history = vec![CanonicalMessage::user_text("hello")];
        let tools = vec![ToolSchema {
            name: "grep".to_string(),
            description: "Search text".to_string(),
            input_schema: json!({"type": "object"}),
        }];

        let assembly = assemble_context(ContextInput {
            project_instructions: Vec::new(),
            global_memory: None,
            project_memory: None,
            history: history.clone(),
            tools: tools.clone(),
        });

        assert_eq!(
            assembly.sources,
            vec![ContextSource::History(history), ContextSource::Tools(tools)]
        );
    }
}
