use clap::Parser;
use kuku::PermissionRequest;
use kuku::conversation::address::ConversationAddress;
use kuku_client::HistoryEntry;
use serde_json::{Value, json};

use crate::cli_args::{Cli, Command};

use super::render::tool_record;
use super::{
    parse_slash_command, permission_ask_line, permission_decision_line, run_input,
    slash_command_candidate,
};

#[test]
fn reply_requires_one_explicit_decision() {
    assert!(Cli::try_parse_from(["kuku", "reply", "s_test", "request"]).is_err());
    assert!(
        Cli::try_parse_from(["kuku", "reply", "s_test", "request", "--allow", "--deny"]).is_err()
    );
    assert!(Cli::try_parse_from(["kuku", "reply", "s_test", "request", "--allow"]).is_ok());
    assert!(Cli::try_parse_from(["kuku", "reply", "s_test", "request", "--deny"]).is_ok());
    assert!(
        Cli::try_parse_from(["kuku", "reply", "s_test", "request", "--answer", "text"]).is_ok()
    );
    assert!(
        Cli::try_parse_from([
            "kuku", "reply", "s_test", "request", "--answer", "text", "--allow"
        ])
        .is_err()
    );
}

#[test]
fn script_connection_flags_remain_available_on_the_run_command() {
    let cli = Cli::try_parse_from([
        "kuku",
        "run",
        "--remote",
        "https://example.test",
        "--token",
        "secret",
        "--workspace",
        "/work",
        "--json",
        "inspect",
        "--a-prompt-flag",
    ])
    .unwrap();
    let Some(Command::Run(args)) = cli.command else {
        panic!("expected run command")
    };
    assert_eq!(Some("https://example.test"), cli.target.remote.as_deref());
    assert_eq!(
        Some("/work"),
        cli.target
            .workspace
            .as_deref()
            .and_then(std::path::Path::to_str)
    );
    assert_eq!(["inspect", "--a-prompt-flag"], args.prompt.as_slice());
    assert!(args.json);
}

#[test]
fn stream_json_permission_lines_include_request_conversation() {
    let request = PermissionRequest {
        id: "perm_review".into(),
        conversation: ConversationAddress::parse("review").unwrap(),
        turn: 1,
        tool_call_id: "toolu_review".into(),
        tool: "run_command".into(),
        risk: "command".into(),
        summary: "run gated command".into(),
        candidate: "cargo test".into(),
        source: "default_ask".into(),
        workspace: None,
        arguments: None,
    };
    let ask: serde_json::Value =
        serde_json::from_str(&permission_ask_line(&request).to_json_line()).unwrap();
    let decision: serde_json::Value = serde_json::from_str(
        &permission_decision_line(&request, "deny".into(), "noninteractive".into()).to_json_line(),
    )
    .unwrap();
    assert_eq!("review", ask["conversation"]);
    assert_eq!("review", decision["conversation"]);
}

#[test]
fn stream_json_tool_call_includes_agent_and_literal_arguments() {
    let args = json!({"program":"fixture","args":["a & b",""],"cwd":".","timeout":8});
    let value = serialized_tool(json!({
        "id":1,"agent":"review","type":"tool_call","tool":"run_command",
        "tool_call_id":"check","args":args
    }));
    assert_eq!("review", value["conversation"]);
    assert_eq!(args, value["args"]);
}

#[test]
fn stream_json_tool_result_keeps_the_durable_evidence() {
    let value = serialized_tool(json!({
        "id":2,"agent":"review","type":"tool_result","tool_call_id":"check",
        "status":"ok","summary":"finished","model_content":"checked","truncated":true,
        "structured":{"exit_code":14,"output_ref":{"id":"saved","bytes":100,"offset":0}},
    }));
    assert_eq!("review", value["conversation"]);
    assert_eq!("checked", value["output"]);
    assert_eq!(true, value["truncated"]);
    assert_eq!(14, value["structured"]["exit_code"]);
    assert_eq!("saved", value["structured"]["output_ref"]["id"]);
}

fn serialized_tool(entry: Value) -> Value {
    let entry: HistoryEntry = serde_json::from_value(entry).unwrap();
    let line = tool_record(&entry).unwrap();
    serde_json::from_str(&line.to_json_line()).unwrap()
}

#[test]
fn slash_command_splits_a_skill_from_the_remaining_prompt() {
    for (input, name, prompt) in [
        ("/tdd implement login", "tdd", "implement login"),
        ("/review", "review", ""),
        (
            "/code-review check auth module",
            "code-review",
            "check auth module",
        ),
        ("/  tdd implement login", "tdd", "implement login"),
    ] {
        assert_eq!((name.into(), prompt.into()), parse_slash_command(input));
        assert_eq!(
            Some((name.into(), prompt.into())),
            slash_command_candidate(input)
        );
    }
}

#[test]
fn path_like_prompts_stay_plain_text() {
    assert!(slash_command_candidate("/tmp/foo").is_none());
    assert!(slash_command_candidate("/etc/hosts").is_none());
    assert!(slash_command_candidate("/").is_none());
    let cli = Cli::try_parse_from(["kuku", "run", "--no-agents", "/tmp/foo"]).unwrap();
    let Some(Command::Run(args)) = cli.command else {
        panic!("expected run command")
    };
    let (text, config) = run_input(&args);
    assert_eq!("/tmp/foo", text);
    assert_eq!(None, config.bootstrap_skill);
    assert!(config.no_agents);
}
