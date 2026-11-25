use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(test)]
use sha2::{Digest, Sha256};

use crate::context::ToolSchema;
use crate::execution::CommandShell;

use super::builtin;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub read_only: bool,
    pub max_result_chars: usize,
    pub risk: String,
}

pub(crate) fn builtin_registry(skills_enabled: bool) -> Vec<ToolDefinition> {
    registry(skills_enabled, false)
}

pub(crate) fn session_registry(skills_enabled: bool) -> Vec<ToolDefinition> {
    registry(skills_enabled, true)
}

fn registry(skills_enabled: bool, session: bool) -> Vec<ToolDefinition> {
    let shells = if session {
        Vec::new()
    } else {
        builtin::run_command::available_shells()
    };
    let mut tools = vec![
        tool(
            "find_files",
            "Find files by name using a glob, or omit the pattern to see the immediate files and folders. Directories end in /. Common build and dependency directories are skipped.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Absolute or workspace-relative search root. Defaults to the workspace root."},
                    "pattern": {"type": "string", "description": "File glob pattern, e.g. *.md or docs/**/*.md."},
                    "max_depth": {"type": "integer", "minimum": 0, "description": "How many levels below the selected directory to look. Defaults to 0 for a listing. With a pattern, the default searches recursively."}
                }
            }),
            true,
            8_000,
            "read",
        ),
        tool(
            "read_file",
            "Read lines from a file, including reference files in a Skill directory. Use the returned offset to continue reading.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Absolute or workspace-relative file path."},
                    "offset": {"type": "integer", "minimum": 1, "description": "Starting line, counted from 1. Defaults to 1."},
                    "limit": {"type": "integer", "minimum": 1, "description": "Maximum number of lines to return."}
                },
                "required": ["path"]
            }),
            true,
            65_536,
            "read",
        ),
        tool(
            "search_text",
            "Find text inside workspace files using a regex.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Regular expression pattern to search for."},
                    "path": {"type": "string", "description": "Folder or file to search. Defaults to the workspace root."},
                    "include": {"type": "string", "description": "Only search files matching this glob, such as **/*.rs."},
                    "view": {"type": "string", "enum": ["files", "lines", "count"], "description": "Defaults to lines with paths, line numbers and excerpts. Use files for paths alone, or count for per-file counts."},
                    "offset": {"type": "integer", "description": "Skip the first N matches (default: 0)."},
                    "limit": {"type": "integer", "minimum": 1, "description": "Maximum matches shown on the page (default: 100)."},
                    "context": {"type": "integer", "description": "Nearby lines before and after each match, as space allows (view=lines only, default: 0)."}
                },
                "required": ["pattern"]
            }),
            true,
            65_536,
            "read",
        ),
        tool(
            "stat_file",
            "Check whether a workspace path is a file, directory, special entry or missing. Use version=true to calculate the SHA-256 version of a file.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path within the workspace."},
                    "version": {"type": "boolean", "description": "Compute content_hash for regular files. Defaults to false."}
                },
                "required": ["path"]
            }),
            true,
            1_000,
            "read",
        ),
        tool(
            "edit_file",
            "Replace exact text in a file. Include enough surrounding text to identify one occurrence. A unique match needs no preliminary read.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path relative to the workspace."},
                    "old_text": {"type": "string", "description": "Exact text to replace."},
                    "new_text": {"type": "string", "description": "Replacement text."},
                    "replace_all": {"type": "boolean", "description": "Replace all occurrences of the text. Requires a full read or your own confirmed full write of the current version. Defaults to false."},
                    "expected_version": {"type": "string", "description": "Optional content_hash of the version to edit."},
                    "brief": {"type": "string", "description": "What this edit does, in 3-5 words."}
                },
                "required": ["path", "old_text", "new_text"]
            }),
            false,
            20_000,
            "edit",
        ),
        tool(
            "write_file",
            "Write a complete file, creating missing parent directories. Overwrite an existing file after a full read or your own confirmed full write of the same version.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path relative to the workspace."},
                    "content": {"type": "string", "description": "Complete file content."},
                    "expected_version": {"type": "string", "description": "Optional content_hash of the version to overwrite."},
                    "brief": {"type": "string", "description": "What this file is for, in 3-5 words."}
                },
                "required": ["path", "content"]
            }),
            false,
            20_000,
            "edit",
        ),
        tool(
            "remove_file",
            "Remove one regular workspace file using an expected version or a full prior read of its current version. Use the file's own path.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path of the regular file inside the workspace."},
                    "expected_version": {"type": "string", "description": "Pass content_hash from stat_file({path, version: true}) or read_file. May be omitted after a full read of the current version."},
                    "brief": {"type": "string", "description": "Why this file is being removed, in 3-5 words."}
                },
                "required": ["path"]
            }),
            false,
            1_000,
            "edit",
        ),
        tool(
            "remember_memory",
            "Append one natural-language bullet to global or project memory under a supported section.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "scope": {"type": "string", "enum": ["global", "project"], "description": "Which memory file to update."},
                    "kind": {"type": "string", "enum": ["how_to_work", "what_is_true", "where_to_look"], "description": "Which memory section to append to."},
                    "text": {"type": "string", "description": "Natural-language bullet text without the leading '- '."}
                },
                "required": ["scope", "kind", "text"]
            }),
            false,
            20_000,
            "edit",
        ),
        tool(
            "forget_memory",
            "Remove exactly one matching bullet from global or project memory by exact text.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "scope": {"type": "string", "enum": ["global", "project"], "description": "Which memory file to update."},
                    "text": {"type": "string", "description": "Exact natural-language bullet text to remove, without the leading '- '."}
                },
                "required": ["scope", "text"]
            }),
            false,
            20_000,
            "edit",
        ),
        run_command_definition(&shells, session),
        tool(
            "fetch_url",
            "Download a URL. Use read_output with the returned output ID when available.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string", "description": "The URL to download."}
                },
                "required": ["url"]
            }),
            true,
            80_000,
            "read",
        ),
        tool(
            "fetch_web",
            "Read a web page as Markdown. A large page may return a summary.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string", "description": "The URL to fetch."},
                    "prompt": {"type": "string", "description": "How to process the content."},
                    "connection": {"type": "string", "description": "Named connection for summarization. Use list_models if uncertain."},
                    "model": {"type": "string", "description": "Exact configured model ID on that connection."}
                },
                "required": ["url", "prompt", "connection", "model"]
            }),
            true,
            80_000,
            "read",
        ),
    ];
    tools.push(builtin::list_models::definition());
    tools.push(builtin::query_session::query_session_definition());
    if skills_enabled {
        tools.push(builtin::list_skills_definition());
        tools.push(builtin::search_skills_definition());
        tools.push(builtin::use_skill_definition());
    }
    tools
}

#[cfg(test)]
pub(crate) fn registry_hash(registry: &[ToolDefinition]) -> String {
    let canonical = serde_json::to_vec(registry).expect("tool registry serializes");
    let digest = Sha256::digest(canonical);
    format!("sha256:{digest:x}")
}

#[cfg(test)]
pub(crate) fn tool_names(registry: &[ToolDefinition]) -> Vec<String> {
    registry.iter().map(|tool| tool.name.clone()).collect()
}

pub(crate) fn to_tool_schemas(registry: &[ToolDefinition]) -> Vec<ToolSchema> {
    registry
        .iter()
        .map(|tool| ToolSchema {
            name: tool.name.clone(),
            description: tool.description.clone(),
            input_schema: tool.input_schema.clone(),
        })
        .collect()
}

pub(crate) fn run_command_definition(shells: &[CommandShell], session: bool) -> ToolDefinition {
    let mut description = "Run a command in the Agent workspace.".to_string();
    let mut schema = serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "program": {"type": "string", "description": "Executable name or path. Prefix a path relative to cwd with './'."},
            "args": {"type": "array", "items": {"type": "string"}, "default": [], "description": "Literal arguments to program."},
            "cwd": {"type": "string", "default": ".", "description": "Optional absolute or workspace-relative working directory."}
        },
        "required": ["program"]
    });
    if session {
        schema["properties"]["background"] = serde_json::json!({
            "type":"boolean",
            "description":"Return a Job handle if the command is still running after one second."
        });
        schema["properties"]["title"] = action_title_schema();
        return tool(
            "run_command",
            r#"Run a program, for example {"program":"git","args":["status"]}. To run a shell script, pass its interpreter as program. The host controls process deadlines. A Job means the command is still running. Check the exit code when it finishes. Use read_output for more output."#,
            schema,
            false,
            80_000,
            "command",
        );
    }
    schema["properties"]["timeout"] = serde_json::json!({
        "type":"integer","minimum":1,"maximum":u32::MAX,
        "description":"Optional process deadline in seconds."
    });
    schema["properties"]["brief"] = serde_json::json!({
        "type":"string","description":"Optional short description of the action."
    });
    if !shells.is_empty() {
        let platform_shell = if cfg!(windows) {
            CommandShell::Powershell
        } else {
            CommandShell::Sh
        };
        let mut command_fields = vec!["command"];
        schema["properties"]["shell"] = serde_json::json!({
            "type": "string", "enum": shells, "description": "Interpreter for command text."
        });
        if shells.contains(&platform_shell) {
            let default = serde_json::json!(platform_shell);
            description.push_str(&format!(
                r#" Example: {{"command":"git status"}} runs in {default}."#
            ));
            schema["properties"]["shell"]["default"] = default;
        } else {
            description.push_str(" For shell syntax, supply command and a listed shell.");
            command_fields.push("shell");
        }
        if shells.contains(&CommandShell::Cmd) {
            description.push_str(" Use shell='cmd' for .bat or .cmd files.");
        }
        schema["properties"]["command"] = serde_json::json!({
            "type": "string", "description": "Shell text evaluated by the selected interpreter. The result reports the outer shell's exit code."
        });
        schema.as_object_mut().unwrap().remove("required");
        schema["oneOf"] = serde_json::json!([
            {"required": ["program"], "not": {"anyOf": [{"required": ["shell"]}, {"required": ["command"]}]}},
            {"required": command_fields, "not": {"required": ["program"]}, "properties": {"args": {"maxItems": 0}}}
        ]);
    }
    description.push_str(r#" For literal arguments use {"program":"git","args":["status"]}. Prefix a workspace-relative program with './'. Reports exit status, elapsed time and the retained output ID."#);
    tool(
        "run_command",
        &description,
        schema,
        false,
        80_000,
        "command",
    )
}

pub(crate) fn action_title_schema() -> Value {
    serde_json::json!({
        "type":"string",
        "maxLength":80,
        "description":"Optional short purpose shown to the user. Omit when the action is self-explanatory."
    })
}

fn tool(
    name: &str,
    description: &str,
    input_schema: Value,
    read_only: bool,
    max_result_chars: usize,
    risk: &str,
) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: description.to_string(),
        input_schema,
        read_only,
        max_result_chars,
        risk: risk.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{builtin_registry, registry_hash, run_command_definition, tool_names};
    use crate::execution::CommandShell;
    use crate::tool::builtin::run_command::{command_permission_candidate, prepare_command_args};
    use serde_json::json;

    #[test]
    fn literal_command_defaults_authorize_the_same_invocation_and_only_sessions_accept_background()
    {
        let workspace = tempfile::tempdir().unwrap();
        let minimal = json!({"program":"git"});
        let mut resolved = minimal.clone();
        let standalone = run_command_definition(&[], false);
        prepare_command_args(&mut resolved, workspace.path(), &standalone).unwrap();
        assert_eq!(resolved["args"], json!([]));
        assert!(resolved.get("timeout").is_none());
        assert_eq!(
            command_permission_candidate(&minimal),
            command_permission_candidate(&json!({"program":"git","args":[],"cwd":"."}))
        );
        let mut background = json!({"program":"git","background":true});
        let error =
            prepare_command_args(&mut background, workspace.path(), &standalone).unwrap_err();
        assert!(
            error
                .model_content
                .contains("background on the outer run_js call")
        );
        let session = run_command_definition(&[], true);
        assert!(prepare_command_args(&mut background, workspace.path(), &session).is_ok());
        background["background"] = json!("yes");
        assert!(prepare_command_args(&mut background, workspace.path(), &session).is_err());
        for (field, value) in [
            ("command", json!("git status")),
            ("shell", json!("sh")),
            ("timeout", json!(1)),
            ("brief", json!("Git status")),
        ] {
            let mut args = minimal.clone();
            args[field] = value;
            let result = prepare_command_args(&mut args, workspace.path(), &session).unwrap_err();
            assert!(
                result
                    .model_content
                    .contains(&format!("does not accept {field:?}"))
            );
            if field == "timeout" {
                assert!(
                    result
                        .model_content
                        .contains("The host controls process deadlines.")
                );
            }
        }
    }

    #[test]
    fn shell_text_requires_a_listed_interpreter_when_the_platform_default_is_absent() {
        let workspace = tempfile::tempdir().unwrap();
        let definition = run_command_definition(&[CommandShell::Bash], false);
        let schema = &definition.input_schema;
        assert!(schema["properties"]["shell"].get("default").is_none());
        assert_eq!(schema["oneOf"][1]["required"], json!(["command", "shell"]));
        let mut args = json!({"command":"echo ready", "cwd":".", "timeout":5});
        assert!(prepare_command_args(&mut args, workspace.path(), &definition).is_err());
        args["shell"] = json!("bash");
        assert!(prepare_command_args(&mut args, workspace.path(), &definition).is_ok());
        for conflict in [
            json!({"args":["-c", "other"]}),
            json!({"program":"other","args":[]}),
        ] {
            let mut mixed = args.clone();
            mixed
                .as_object_mut()
                .unwrap()
                .extend(conflict.as_object().unwrap().clone());
            assert!(prepare_command_args(&mut mixed, workspace.path(), &definition).is_err());
        }
    }

    #[test]
    fn builtin_registry_matches_documented_public_tool_surface() {
        let registry = builtin_registry(false);

        assert_eq!(
            tool_names(&registry),
            vec![
                "find_files",
                "read_file",
                "search_text",
                "stat_file",
                "edit_file",
                "write_file",
                "remove_file",
                "remember_memory",
                "forget_memory",
                "run_command",
                "fetch_url",
                "fetch_web",
                "list_models",
                "query_session",
            ]
        );
        assert_eq!(registry[0].risk, "read");
        assert!(registry[0].read_only);
        assert_eq!(registry[0].max_result_chars, 8_000);
        assert_eq!(
            registry_hash(&registry),
            registry_hash(&builtin_registry(false))
        );

        let remember = registry
            .iter()
            .find(|tool| tool.name == "remember_memory")
            .expect("remember_memory registered");
        assert!(!remember.read_only);
        assert_eq!(remember.risk, "edit");

        let forget = registry
            .iter()
            .find(|tool| tool.name == "forget_memory")
            .expect("forget_memory registered");
        assert!(!forget.read_only);
        assert_eq!(forget.risk, "edit");
    }

    #[test]
    fn builtin_registry_includes_skill_tools_when_enabled() {
        let registry = builtin_registry(true);
        let names = tool_names(&registry);
        let base_count = builtin_registry(false).len();
        assert_eq!(
            names[base_count..],
            ["list_skills", "search_skills", "use_skill"]
        );
        let resources = &registry[base_count + 2];
        assert!(resources.read_only);
        assert_eq!(resources.risk, "read");
    }

    #[test]
    fn builtin_registry_excludes_skill_tools_when_disabled() {
        let registry = builtin_registry(false);
        let names = tool_names(&registry);
        assert!(!names.contains(&"use_skill".to_string()));
        assert!(!names.contains(&"list_skills".to_string()));
        assert!(!names.contains(&"search_skills".to_string()));
    }
}
