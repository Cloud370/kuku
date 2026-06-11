use std::sync::{Mutex, OnceLock};

use super::{
    build_query, conversation_for_tool_kind, nested_permission_parent_tool_id,
    noninteractive_permission_choice, parse_slash_command, permission_ask_line,
    permission_decision_line, slash_command_candidate, tool_call_line, tool_result_line,
};
use crate::cli_args::RunArgs;
use kuku::{PermissionChoice, UiEvent};

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn temp_workspace() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "kuku-cli-run-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn test_config_path(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("config.toml");
    std::fs::write(&path, kuku::config::generate_default()).unwrap();
    path
}

#[test]
fn stream_json_without_yes_denies_permission_requests() {
    assert_eq!(
        noninteractive_permission_choice(false, true),
        Some(PermissionChoice::Deny)
    );
}

#[test]
fn yes_auto_allows_permission_requests() {
    assert_eq!(
        noninteractive_permission_choice(true, false),
        Some(PermissionChoice::Once)
    );
    assert_eq!(
        noninteractive_permission_choice(true, true),
        Some(PermissionChoice::Once)
    );
}

#[test]
fn interactive_without_yes_waits_for_user_input() {
    assert_eq!(noninteractive_permission_choice(false, false), None);
}

#[test]
fn nested_permission_event_exposes_parent_tool_id() {
    let request = kuku::query::PermissionRequest {
        id: "perm_child".to_string(),
        conversation: kuku::conversation::address::ConversationAddress::parse("review").unwrap(),
        turn: 1,
        tool_call_id: "toolu_child".to_string(),
        tool: "run_command".to_string(),
        risk: "command".to_string(),
        summary: "run gated command".to_string(),
        candidate: "cargo test".to_string(),
        source: "default_ask".to_string(),
    };
    let event = UiEvent::ToolOutput {
        id: "toolu_agent_parent".to_string(),
        event: kuku::query::ToolEvent::PermissionRequested { request },
    };

    assert_eq!(
        nested_permission_parent_tool_id(&event),
        Some("toolu_agent_parent")
    );
}

#[test]
fn stream_json_permission_lines_include_request_conversation() {
    let request = kuku::query::PermissionRequest {
        id: "perm_review".to_string(),
        conversation: kuku::conversation::address::ConversationAddress::parse("review").unwrap(),
        turn: 1,
        tool_call_id: "toolu_review".to_string(),
        tool: "run_command".to_string(),
        risk: "command".to_string(),
        summary: "run gated command".to_string(),
        candidate: "cargo test".to_string(),
        source: "default_ask".to_string(),
    };

    let ask: serde_json::Value =
        serde_json::from_str(&permission_ask_line(&request).to_json_line()).unwrap();
    let decision: serde_json::Value = serde_json::from_str(
        &permission_decision_line(&request, "deny".to_string(), "noninteractive".to_string())
            .to_json_line(),
    )
    .unwrap();

    assert_eq!(ask["conversation"], "review");
    assert_eq!(decision["conversation"], "review");
}

#[test]
fn stream_json_agent_tool_call_includes_conversation() {
    let kind = kuku::query::ToolKind::Agent {
        conversation: kuku::conversation::address::ConversationAddress::parse("review").unwrap(),
        binding_id: "binding_1".to_string(),
    };

    let line = tool_call_line(
        "agent".to_string(),
        "toolu_agent".to_string(),
        "run review agent".to_string(),
        &kind,
    );
    let value: serde_json::Value = serde_json::from_str(&line.to_json_line()).unwrap();

    assert_eq!(
        conversation_for_tool_kind(&kind),
        Some("review".to_string())
    );
    assert_eq!(value["conversation"], "review");
}

#[test]
fn stream_json_agent_tool_result_includes_conversation() {
    let line = tool_result_line(
        "toolu_agent".to_string(),
        "ok".to_string(),
        "review complete".to_string(),
        Some("review".to_string()),
    );
    let value: serde_json::Value = serde_json::from_str(&line.to_json_line()).unwrap();

    assert_eq!(value["conversation"], "review");
}

#[test]
fn slash_command_with_prompt() {
    let (name, rest) = parse_slash_command("/tdd implement login");
    assert_eq!(name, "tdd");
    assert_eq!(rest, "implement login");
}

#[test]
fn slash_command_without_prompt() {
    let (name, rest) = parse_slash_command("/review");
    assert_eq!(name, "review");
    assert_eq!(rest, "");
}

#[test]
fn slash_command_with_multiple_words() {
    let (name, rest) = parse_slash_command("/code-review check auth module");
    assert_eq!(name, "code-review");
    assert_eq!(rest, "check auth module");
}

#[test]
fn slash_command_trims_leading_whitespace() {
    let (name, rest) = parse_slash_command("/  tdd implement login");
    assert_eq!(name, "tdd");
    assert_eq!(rest, "implement login");
}

#[test]
fn slash_command_candidate_rejects_path_like_prompts() {
    assert!(slash_command_candidate("/tmp/foo").is_none());
    assert!(slash_command_candidate("/etc/hosts").is_none());
    assert!(slash_command_candidate("/").is_none());
}

#[test]
fn build_query_treats_path_like_prompt_as_plain_text() {
    let _guard = env_lock().lock().unwrap();
    let workspace = temp_workspace();
    let previous_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(&workspace).unwrap();

    let args = RunArgs {
        prompt: vec!["/tmp/foo".to_string()],
        auto_yes: false,
        model: None,
        session: None,
        cont: false,
        json: false,
        stream_json: false,
        show_thinking: false,
        raw: false,
        verbose: false,
        config: None,
        prompts_dir: None,
        no_agents: true,
        no_skills: false,
        skill_body: None,
        bootstrap_skill_name: None,
    };

    let result = build_query(&args, test_config_path(&workspace)).unwrap();

    std::env::set_current_dir(previous_cwd).unwrap();
    let _ = std::fs::remove_dir_all(&workspace);

    assert_eq!(result.query.prompt(), "/tmp/foo");
}

#[test]
fn slash_command_candidate_accepts_valid_skill_names() {
    assert_eq!(
        slash_command_candidate("/review check this"),
        Some(("review".to_string(), "check this".to_string()))
    );
    assert_eq!(
        slash_command_candidate("/code-review"),
        Some(("code-review".to_string(), String::new()))
    );
}

#[test]
fn slash_command_surfaces_skill_discovery_errors() {
    let _guard = env_lock().lock().unwrap();
    let workspace = temp_workspace();
    let previous_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(&workspace).unwrap();

    let skill_dir = workspace.join(".kuku").join("skills").join("broken-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: other-name\ndescription: broken\n---\n\n# Broken\n",
    )
    .unwrap();

    let args = RunArgs {
        prompt: vec!["/broken-skill do something".to_string()],
        auto_yes: false,
        model: None,
        session: None,
        cont: false,
        json: false,
        stream_json: false,
        show_thinking: false,
        raw: false,
        verbose: false,
        config: None,
        prompts_dir: None,
        no_agents: true,
        no_skills: false,
        skill_body: None,
        bootstrap_skill_name: None,
    };

    let error = match build_query(&args, test_config_path(&workspace)) {
        Ok(_) => panic!("expected discovery failure"),
        Err(error) => error,
    };

    std::env::set_current_dir(previous_cwd).unwrap();
    let _ = std::fs::remove_dir_all(&workspace);

    assert!(!error.to_string().is_empty());
}
