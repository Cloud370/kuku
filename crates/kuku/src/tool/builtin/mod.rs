pub(crate) mod common;
mod edit_file;
mod fetch_url;
mod fetch_web;
mod find_files;
mod memory;
pub(crate) mod query_session;
mod read_file;
mod run_command;
mod search_text;
mod write_file;

#[cfg(test)]
mod test_helpers;

pub(crate) use edit_file::edit_file;
pub(crate) use fetch_url::fetch_url;
pub(crate) use fetch_web::fetch_web;
pub(crate) use find_files::find_files;
pub(crate) use memory::{forget_memory_with_home, remember_memory_with_home};
pub(crate) use query_session::query_session;
pub(crate) use read_file::read_file;
pub(crate) use run_command::{run_command, CommandEvent};
pub(crate) use search_text::search_text;
pub(crate) use write_file::write_file;

pub(crate) fn agent_definition() -> crate::tool::ToolDefinition {
    crate::tool::ToolDefinition {
        name: "agent".to_string(),
        description: "Delegate a task to a named subagent (child session). Use this for work that benefits from isolated context: explore search, code review, plan exploration.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the subagent to dispatch (from the available catalog below)"
                },
                "prompt": {
                    "type": "string",
                    "description": "The task to delegate, with enough context for the subagent to work independently"
                }
            },
            "required": ["name", "prompt"]
        }),
        read_only: false,
        max_result_chars: 20_000,
        risk: "read".to_string(),
    }
}

pub(crate) fn use_skill_definition() -> crate::tool::ToolDefinition {
    crate::tool::ToolDefinition {
        name: "use_skill".to_string(),
        description: "Load a skill's full instructions into the current session. Use this when you want to follow a skill's workflow.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "skill_name": {
                    "type": "string",
                    "description": "Name of the skill to load (from the available skill catalog)"
                }
            },
            "required": ["skill_name"]
        }),
        read_only: true,
        max_result_chars: 80_000,
        risk: "read".to_string(),
    }
}

pub(crate) fn list_skills_definition() -> crate::tool::ToolDefinition {
    crate::tool::ToolDefinition {
        name: "list_skills".to_string(),
        description: "Browse the current skill snapshot available in this session. Use this to inspect skill names and descriptions before loading one.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "offset": {
                    "type": "integer",
                    "description": "Number of skills to skip before returning results. Defaults to 0."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum skills to return. Defaults to 20, capped at 50."
                }
            }
        }),
        read_only: true,
        max_result_chars: 20_000,
        risk: "read".to_string(),
    }
}

pub(crate) fn search_skills_definition() -> crate::tool::ToolDefinition {
    crate::tool::ToolDefinition {
        name: "search_skills".to_string(),
        description: "Search the current skill snapshot by name, description, title, headings, and body text to find relevant workflows.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "What kind of skill you want to find."
                },
                "offset": {
                    "type": "integer",
                    "description": "Number of matches to skip before returning results. Defaults to 0."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum matches to return. Defaults to 10, capped at 25."
                }
            },
            "required": ["query"]
        }),
        read_only: true,
        max_result_chars: 20_000,
        risk: "read".to_string(),
    }
}
