use clap::Parser;
use kuku::conversation::address::ConversationAddress;
use kuku::{PermissionRequest, ToolEvent, ToolKind, UiEvent};

use crate::cli_args::{Cli, Command};

use super::render::{
    conversation_for_tool_kind, nested_model_recovery, tool_call_line, tool_result_line,
};
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
    assert!(Cli::try_parse_from([
        "kuku", "reply", "s_test", "request", "--answer", "text", "--allow"
    ])
    .is_err());
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
fn nested_model_recovery_exposes_child_invalidation_boundary() {
    let info = kuku::query::ModelRecoveryInfo {
        conversation: ConversationAddress::parse("review").unwrap(),
        turn: 2,
        from_request_id: "req_1".into(),
        to_request_id: "req_2".into(),
        reason: kuku::event::ModelStopReason::Length,
        attempt: 1,
        max_attempts: 1,
    };
    let event = UiEvent::ToolOutput {
        id: "toolu_agent_parent".into(),
        event: ToolEvent::ModelRecovery { info: info.clone() },
    };
    let wire = kuku::wire::to_wire(&event).unwrap();
    assert_eq!(Some(info), nested_model_recovery(&wire["event"]));
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
fn stream_json_agent_tool_call_includes_conversation() {
    let kind = ToolKind::Agent {
        conversation: ConversationAddress::parse("review").unwrap(),
        binding_id: "binding_1".into(),
    };
    let line = tool_call_line(
        "agent".into(),
        "toolu_agent".into(),
        "run review agent".into(),
        &kind,
    );
    let value: serde_json::Value = serde_json::from_str(&line.to_json_line()).unwrap();
    assert_eq!(Some("review".into()), conversation_for_tool_kind(&kind));
    assert_eq!("review", value["conversation"]);
}

#[test]
fn stream_json_agent_tool_result_includes_conversation() {
    let line = tool_result_line(
        "toolu_agent".into(),
        "ok".into(),
        "review complete".into(),
        Some("review".into()),
    );
    let value: serde_json::Value = serde_json::from_str(&line.to_json_line()).unwrap();
    assert_eq!("review", value["conversation"]);
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
