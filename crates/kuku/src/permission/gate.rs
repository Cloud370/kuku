use crate::event::{EventPayload, StoredEvent};

#[derive(Debug, Clone, PartialEq, Eq)]
/// A permission grant scoped to the current session.
pub struct SessionGrant {
    pub tool: String,
    pub pattern: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Whether a tool call is allowed, denied, or requires user confirmation.
pub enum GateDecisionKind {
    Allow,
    Deny,
    Ask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// How long a permission decision applies.
pub enum GateScope {
    Once,
    Session,
    Project,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Which authority produced the permission decision.
pub enum GateSource {
    HardGuard,
    ProjectPolicy,
    SessionGrant,
    TrustPosture,
    Host,
    DefaultAsk,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// The complete result of a permission gate evaluation.
pub struct GateDecision {
    pub kind: GateDecisionKind,
    pub scope: GateScope,
    pub source: GateSource,
    pub rule: String,
}

/// Extract session-scoped permission grants from previously recorded permission decisions.
pub fn recover_session_grants(events: &[StoredEvent]) -> Vec<SessionGrant> {
    events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::PermissionDecision {
                decision,
                scope,
                rule,
                ..
            } if decision == "allow" && scope == "session" => parse_session_rule(rule),
            _ => None,
        })
        .collect()
}

/// Evaluate a tool call against hard guards, policy, session grants, and simplified defaults.
///
/// Simplified model:
/// - Known operations (read tools, edit tools, memory tools, agent tool) → allow by default
/// - Commands (run_command) → ask by default
/// - Hard guard violations → deny (overrides everything)
/// - Policy deny → deny (overrides defaults)
/// - Policy allow → allow (overrides defaults for commands)
/// - Session grants → allow (override defaults for commands in current session)
pub fn decide_tool_call(
    tool: &str,
    risk: &str,
    candidate: &str,
    policy: &crate::permission::PermissionPolicy,
    session_grants: &[SessionGrant],
) -> GateDecision {
    if let Some(rule) = hard_guard_rule(tool, risk, candidate) {
        return GateDecision {
            kind: GateDecisionKind::Deny,
            scope: GateScope::Once,
            source: GateSource::HardGuard,
            rule,
        };
    }

    if policy.matches_deny(tool, candidate) {
        return GateDecision {
            kind: GateDecisionKind::Deny,
            scope: GateScope::Once,
            source: GateSource::ProjectPolicy,
            rule: candidate.to_string(),
        };
    }

    if session_grants
        .iter()
        .any(|grant| grant.tool == tool && pattern_matches(&grant.pattern, candidate))
    {
        return GateDecision {
            kind: GateDecisionKind::Allow,
            scope: GateScope::Session,
            source: GateSource::SessionGrant,
            rule: candidate.to_string(),
        };
    }

    if policy.matches_allow(tool, candidate) {
        return GateDecision {
            kind: GateDecisionKind::Allow,
            scope: GateScope::Project,
            source: GateSource::ProjectPolicy,
            rule: candidate.to_string(),
        };
    }

    match tool {
        "run_command" => GateDecision {
            kind: GateDecisionKind::Ask,
            scope: GateScope::Once,
            source: GateSource::DefaultAsk,
            rule: candidate.to_string(),
        },
        _ => GateDecision {
            kind: GateDecisionKind::Allow,
            scope: GateScope::Once,
            source: GateSource::TrustPosture,
            rule: tool.to_string(),
        },
    }
}

fn parse_session_rule(rule: &str) -> Option<SessionGrant> {
    let open = rule.find('(')?;
    let close = rule.rfind(')')?;
    if close <= open + 1 || close != rule.len() - 1 {
        return None;
    }
    Some(SessionGrant {
        tool: rule[..open].trim().to_string(),
        pattern: rule[open + 1..close].trim().to_string(),
    })
}

fn hard_guard_rule(tool: &str, risk: &str, candidate: &str) -> Option<String> {
    if (risk == "read" || risk == "edit") && is_hard_guarded_path(candidate) {
        return Some(candidate.to_string());
    }
    if tool == "run_command" && is_hard_guarded_command(candidate) {
        return Some(candidate.to_string());
    }
    None
}

fn is_hard_guarded_path(candidate: &str) -> bool {
    let normalized = crate::tool::builtin::common::normalize_path_sep(candidate);
    normalized
        .split('/')
        .any(|part| part == ".git" || part == ".ssh")
        || (normalized.split('/').any(|part| part == ".kuku") && normalized.ends_with("/policy.md"))
        || crate::tool::builtin::common::is_sensitive_file_name(
            normalized.rsplit('/').next().unwrap_or(&normalized),
        )
}

fn is_hard_guarded_command(candidate: &str) -> bool {
    normalized_command_segments(candidate)
        .into_iter()
        .any(|segment| is_dangerous_command_segment(&segment))
}

#[allow(clippy::collapsible_str_replace)]
fn split_command_separators(input: &str) -> String {
    // Order matters: && must be replaced before & to avoid double-splitting
    input
        .replace("&&", "\x00")
        .replace("||", "\n")
        .replace('&', "\n")
        .replace('\x00', "\n")
        .replace(';', "\n")
}

fn normalized_command_segments(candidate: &str) -> Vec<String> {
    let normalized = split_command_separators(&candidate.to_ascii_lowercase());

    normalized
        .lines()
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(|segment| segment.strip_prefix("sudo ").unwrap_or(segment).trim())
        .flat_map(unwrap_shell_wrapper)
        .collect()
}

fn unwrap_shell_wrapper(segment: &str) -> Vec<String> {
    for prefix in [
        "sh -c ",
        "bash -lc ",
        "zsh -lc ",
        "cmd /c ",
        "powershell -command ",
        "pwsh -command ",
    ] {
        if let Some(rest) = segment.strip_prefix(prefix) {
            let rest = rest.trim().trim_matches('"').trim_matches('\'');
            return split_command_separators(rest)
                .lines()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect();
        }
    }
    vec![segment.to_string()]
}

fn is_dangerous_command_segment(segment: &str) -> bool {
    [
        "git push",
        "git reset --hard",
        "git clean -f",
        "rm -rf",
        "rm -fr",
        "cargo publish",
        "npm publish",
        "pnpm publish",
        "yarn publish",
        "bun publish",
        "npm run deploy",
        "pnpm deploy",
        "yarn deploy",
        "make deploy",
        "cargo release",
        "rmdir /s /q",
        "del /s",
        "remove-item -recurse -force",
    ]
    .iter()
    .any(|prefix| segment.starts_with(prefix))
}

fn pattern_matches(pattern: &str, candidate: &str) -> bool {
    let candidate = crate::tool::builtin::common::normalize_path_sep(candidate);
    if let Some(prefix) = pattern.strip_suffix("/**") {
        return candidate == prefix || candidate.starts_with(&format!("{prefix}/"));
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return candidate.starts_with(prefix);
    }
    candidate == pattern
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_policy() -> crate::permission::PermissionPolicy {
        crate::permission::parse_policy("# policy\n\n## allow\n\n## deny\n").unwrap()
    }

    #[test]
    fn simplified_model_allow_known_tools_by_default() {
        let policy = empty_policy();
        let grants = vec![];

        for (tool, candidate) in [
            ("find_files", "."),
            ("read_file", "README.md"),
            ("search_text", "TODO"),
        ] {
            let decision = decide_tool_call(tool, "read", candidate, &policy, &grants);
            assert_eq!(
                decision.kind,
                GateDecisionKind::Allow,
                "read tool {tool} should allow"
            );
        }

        for (tool, candidate) in [
            ("edit_file", "src/main.rs"),
            ("write_file", "src/lib.rs"),
            ("remember_memory", "memory.md"),
            ("forget_memory", "memory.md"),
        ] {
            let decision = decide_tool_call(tool, "edit", candidate, &policy, &grants);
            assert_eq!(
                decision.kind,
                GateDecisionKind::Allow,
                "edit tool {tool} should allow"
            );
        }
    }

    #[test]
    fn simplified_model_ask_for_commands() {
        let policy = empty_policy();
        let grants = vec![];
        let decision = decide_tool_call("run_command", "command", "cargo test", &policy, &grants);
        assert_eq!(decision.kind, GateDecisionKind::Ask);
        assert!(matches!(decision.source, GateSource::DefaultAsk));
    }

    #[test]
    fn simplified_model_hard_guard_deny_overrides_allow() {
        let policy = empty_policy();
        let grants = vec![];
        let decision = decide_tool_call("edit_file", "edit", ".git/config", &policy, &grants);
        assert_eq!(decision.kind, GateDecisionKind::Deny);
        assert!(matches!(decision.source, GateSource::HardGuard));
    }

    #[test]
    fn simplified_model_policy_deny_overrides_default_allow() {
        let policy = crate::permission::parse_policy(
            "# policy\n\n## allow\n\n## deny\n- edit_file(docs/secret.md)\n",
        )
        .unwrap();
        let grants = vec![];
        let decision = decide_tool_call("edit_file", "edit", "docs/secret.md", &policy, &grants);
        assert_eq!(decision.kind, GateDecisionKind::Deny);
    }

    #[test]
    fn simplified_model_policy_allow_allows_commands() {
        let policy = crate::permission::parse_policy(
            "# policy\n\n## allow\n- run_command(cargo test *)\n\n## deny\n",
        )
        .unwrap();
        let grants = vec![];
        let decision = decide_tool_call(
            "run_command",
            "command",
            "cargo test --lib",
            &policy,
            &grants,
        );
        assert_eq!(decision.kind, GateDecisionKind::Allow);
    }
}
