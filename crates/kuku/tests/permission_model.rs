use kuku::error::Error;

#[test]
fn parses_allow_and_deny_sections_with_exact_and_terminal_wildcards() {
    let markdown = r#"# policy

## allow
- edit_file(docs/**)
- run_command(cargo test *)
- run_command(git status)

## deny
- read_file(.env*)
- run_command(git push *)
"#;

    let policy = kuku::permission::parse_policy(markdown).unwrap();

    assert!(policy.matches_allow("edit_file", "docs/guide.md"));
    assert!(policy.matches_allow("run_command", "cargo test -p kuku"));
    assert!(policy.matches_allow("run_command", "git status"));
    assert!(policy.matches_deny("read_file", ".env.local"));
    assert!(policy.matches_deny("run_command", "git push origin main"));
}

#[test]
fn rejects_invalid_policy_rule_shape() {
    let markdown = r#"# policy

## allow
- run_command(
"#;

    let error = kuku::permission::parse_policy(markdown).unwrap_err();
    assert!(matches!(error, Error::InvalidPolicy(_)));
}

#[test]
fn missing_policy_file_loads_as_empty_policy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("policy.md");

    let policy = kuku::permission::load_project_policy(&path).unwrap();
    assert!(!policy.matches_allow("run_command", "cargo test"));
    assert!(!policy.matches_deny("run_command", "cargo test"));
}

#[test]
fn append_project_allow_rule_persists_rule() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("policy.md");

    kuku::permission::append_project_allow_rule(&path, "run_command", "cargo test *").unwrap();
    let policy = kuku::permission::load_project_policy(&path).unwrap();

    assert!(policy.matches_allow("run_command", "cargo test -p kuku"));
}

#[test]
fn read_tools_default_allow_after_guard_checks() {
    let policy = kuku::permission::parse_policy("# policy\n").unwrap();
    let decision =
        kuku::permission::decide_tool_call("read_file", "read", "README.md", &policy, &[]);

    assert_eq!(decision.kind, kuku::permission::GateDecisionKind::Allow);
    assert_eq!(decision.source, kuku::permission::GateSource::TrustPosture);
}

#[test]
fn deny_rule_wins_over_allow_paths() {
    let policy = kuku::permission::parse_policy(
        r#"# policy

## allow
- run_command(cargo test *)

## deny
- run_command(cargo test -p kuku *)
"#,
    )
    .unwrap();

    let decision = kuku::permission::decide_tool_call(
        "run_command",
        "command",
        "cargo test -p kuku --lib",
        &policy,
        &[],
    );

    assert_eq!(decision.kind, kuku::permission::GateDecisionKind::Deny);
    assert_eq!(decision.source, kuku::permission::GateSource::ProjectPolicy);
}

#[test]
fn session_allow_is_recovered_from_prior_permission_allow_events() {
    let events = vec![kuku::event::StoredEvent {
        id: 7,
        payload: kuku::event::EventPayload::PermissionAllow {
            turn: 1,
            ts: "2026-05-14T00:00:00Z".to_string(),
            tool_call_id: "toolu_1".to_string(),
            tool: "run_command".to_string(),
            scope: "session".to_string(),
            matcher: "run_command(cargo test *)".to_string(),
            source: "host".to_string(),
        },
    }];

    let grants = kuku::permission::recover_session_grants(&events);
    let policy = kuku::permission::parse_policy("# policy\n").unwrap();
    let decision = kuku::permission::decide_tool_call(
        "run_command",
        "command",
        "cargo test -p kuku",
        &policy,
        &grants,
    );

    assert_eq!(decision.kind, kuku::permission::GateDecisionKind::Allow);
    assert_eq!(decision.source, kuku::permission::GateSource::SessionGrant);
}

#[test]
fn hard_guard_blocks_git_and_secret_paths() {
    let policy = kuku::permission::parse_policy("# policy\n").unwrap();

    let git_decision =
        kuku::permission::decide_tool_call("write_file", "edit", ".git/config", &policy, &[]);
    assert_eq!(git_decision.kind, kuku::permission::GateDecisionKind::Deny);
    assert_eq!(git_decision.source, kuku::permission::GateSource::HardGuard);

    let env_decision =
        kuku::permission::decide_tool_call("read_file", "read", ".env.local", &policy, &[]);
    assert_eq!(env_decision.kind, kuku::permission::GateDecisionKind::Deny);
    assert_eq!(env_decision.source, kuku::permission::GateSource::HardGuard);
}

#[test]
fn hard_guard_blocks_destructive_commands() {
    let policy = kuku::permission::parse_policy("# policy\n").unwrap();

    for command in [
        "git reset --hard HEAD~1",
        "rm -rf target",
        "npm publish",
        "make deploy",
    ] {
        let decision =
            kuku::permission::decide_tool_call("run_command", "command", command, &policy, &[]);

        assert_eq!(decision.kind, kuku::permission::GateDecisionKind::Deny);
        assert_eq!(decision.source, kuku::permission::GateSource::HardGuard);
    }
}

#[test]
fn hard_guard_does_not_block_git_push_or_gh_pr_create() {
    let policy = kuku::permission::parse_policy("# policy\n").unwrap();

    for command in ["git push origin main", "gh pr create --fill"] {
        let decision =
            kuku::permission::decide_tool_call("run_command", "command", command, &policy, &[]);

        assert_eq!(decision.kind, kuku::permission::GateDecisionKind::Ask);
        assert_eq!(decision.source, kuku::permission::GateSource::DefaultAsk);
    }
}

#[test]
fn hard_guard_allows_workspace_policy_md_name() {
    let policy = kuku::permission::parse_policy("# policy\n").unwrap();

    let decision =
        kuku::permission::decide_tool_call("write_file", "edit", "docs/policy.md", &policy, &[]);

    assert_ne!(decision.kind, kuku::permission::GateDecisionKind::Deny);
}

#[test]
fn hard_guard_blocks_wrapped_destructive_commands() {
    let policy = kuku::permission::parse_policy("# policy\n").unwrap();

    let decision = kuku::permission::decide_tool_call(
        "run_command",
        "command",
        "sh -c 'sudo git reset --hard HEAD~1'",
        &policy,
        &[],
    );

    assert_eq!(decision.kind, kuku::permission::GateDecisionKind::Deny);
    assert_eq!(decision.source, kuku::permission::GateSource::HardGuard);
}
