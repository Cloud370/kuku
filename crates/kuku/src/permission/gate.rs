use std::borrow::Cow;
use std::path::Path;

use crate::event::{EventPayload, StoredEvent};
use crate::execution::CommandInvocation;

const GUARDED_COMMANDS: &[(&str, &str)] = &[
    ("git reset --hard", "git reset --hard discards local work"),
    ("git clean -f", "git clean deletes untracked files"),
    ("rm -rf", "recursive force delete is destructive"),
    ("rm -fr", "recursive force delete is destructive"),
    ("rmdir /s /q", "recursive force delete is destructive"),
    ("del /s", "recursive delete is destructive"),
    (
        "remove-item -recurse -force",
        "recursive force delete is destructive",
    ),
    (
        "cargo publish",
        "publish affects external package registries",
    ),
    ("npm publish", "publish affects external package registries"),
    (
        "pnpm publish",
        "publish affects external package registries",
    ),
    (
        "yarn publish",
        "publish affects external package registries",
    ),
    ("bun publish", "publish affects external package registries"),
    ("npm run deploy", "deploy affects external systems"),
    ("pnpm deploy", "deploy affects external systems"),
    ("yarn deploy", "deploy affects external systems"),
    ("make deploy", "deploy affects external systems"),
    ("cargo release", "release affects external systems"),
];

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
    pub(crate) denial_reason: Option<&'static str>,
}

/// Extract session-scoped permission grants from previously recorded permission facts.
pub fn recover_session_grants(events: &[StoredEvent]) -> Vec<SessionGrant> {
    events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::PermissionAllow {
                tool,
                scope,
                matcher,
                ..
            } if scope == "session" => Some(SessionGrant {
                tool: tool.clone(),
                pattern: matcher_from_rule(matcher),
            }),
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
    if let Some(reason) = hard_guard_reason(tool, risk, candidate) {
        return GateDecision {
            kind: GateDecisionKind::Deny,
            scope: GateScope::Once,
            source: GateSource::HardGuard,
            rule: candidate.to_string(),
            denial_reason: Some(reason),
        };
    }

    if policy.matches_deny(tool, candidate) {
        return GateDecision {
            kind: GateDecisionKind::Deny,
            scope: GateScope::Once,
            source: GateSource::ProjectPolicy,
            rule: candidate.to_string(),
            denial_reason: Some("workspace policy denied this action"),
        };
    }

    if session_grants
        .iter()
        .any(|grant| grant.tool == tool && pattern_matches(tool, &grant.pattern, candidate))
    {
        return GateDecision {
            kind: GateDecisionKind::Allow,
            scope: GateScope::Session,
            source: GateSource::SessionGrant,
            rule: candidate.to_string(),
            denial_reason: None,
        };
    }

    if policy.matches_allow(tool, candidate) {
        return GateDecision {
            kind: GateDecisionKind::Allow,
            scope: GateScope::Project,
            source: GateSource::ProjectPolicy,
            rule: candidate.to_string(),
            denial_reason: None,
        };
    }

    match tool {
        "run_command" => GateDecision {
            kind: GateDecisionKind::Ask,
            scope: GateScope::Once,
            source: GateSource::DefaultAsk,
            rule: candidate.to_string(),
            denial_reason: None,
        },
        _ => GateDecision {
            kind: GateDecisionKind::Allow,
            scope: GateScope::Once,
            source: GateSource::TrustPosture,
            rule: tool.to_string(),
            denial_reason: None,
        },
    }
}

fn matcher_from_rule(rule: &str) -> String {
    let Some(open) = rule.find('(') else {
        return rule.to_string();
    };
    let Some(close) = rule.rfind(')') else {
        return rule.to_string();
    };
    if close <= open + 1 || close != rule.len() - 1 {
        return rule.to_string();
    }
    rule[open + 1..close].trim().to_string()
}

fn hard_guard_reason(tool: &str, risk: &str, candidate: &str) -> Option<&'static str> {
    if (risk == "read" || risk == "edit") && is_hard_guarded_path(candidate) {
        return Some("the target is protected by a filesystem safety rule");
    }
    if tool == "run_command" {
        return guarded_command_candidate_reason(candidate);
    }
    None
}

fn is_hard_guarded_path(candidate: &str) -> bool {
    let normalized = crate::util::path::normalize_path_sep(candidate);
    normalized
        .split('/')
        .any(|part| part == ".git" || part == ".ssh")
        || (normalized.split('/').any(|part| part == ".kuku") && normalized.ends_with("/policy.md"))
        || crate::util::path::is_sensitive_file_name(
            normalized.rsplit('/').next().unwrap_or(&normalized),
        )
}

fn guarded_command_candidate_reason(candidate: &str) -> Option<&'static str> {
    #[derive(serde::Deserialize)]
    struct Candidate {
        invocation: CommandInvocation,
    }

    if candidate.starts_with('{') {
        if let Ok(candidate) = serde_json::from_str::<Candidate>(candidate) {
            return blocked_command_reason(&candidate.invocation);
        }
    }
    shell_guard_reason(candidate)
}

pub(crate) fn blocked_command_reason(invocation: &CommandInvocation) -> Option<&'static str> {
    match invocation {
        CommandInvocation::Program { program, args } => native_guard_reason(program, args),
        CommandInvocation::Shell { command, .. } => shell_guard_reason(command),
    }
}

fn native_guard_reason(program: &str, args: &[String]) -> Option<&'static str> {
    let program = command_name(program)?;
    let (program, args) = if program == "sudo" {
        let (program, args) = args.split_first()?;
        (command_name(program)?, args)
    } else {
        (program, args)
    };
    for (rule, reason) in GUARDED_COMMANDS {
        let mut parts = rule.split_whitespace().peekable();
        let mut args = args.iter();
        if parts.next() != Some(program.as_str()) {
            continue;
        }
        while let Some(part) = parts.next() {
            let Some(arg) = args.next() else {
                break;
            };
            if parts.peek().is_none() && arg.to_ascii_lowercase().starts_with(part) {
                return Some(reason);
            }
            if !arg.eq_ignore_ascii_case(part) {
                break;
            }
        }
    }
    let flags: &[&str] = match program.as_str() {
        "sh" | "bash" | "zsh" => &["-c", "-lc", "-cl"],
        "cmd" => &["/c", "/k"],
        "powershell" | "pwsh" => &["-command", "-c"],
        _ => return None,
    };
    let command = args
        .iter()
        .position(|arg| flags.iter().any(|flag| arg.eq_ignore_ascii_case(flag)))?;
    shell_guard_reason(args.get(command + 1)?)
}

fn command_name(program: &str) -> Option<String> {
    let name = Path::new(program)
        .file_name()?
        .to_str()?
        .to_ascii_lowercase();
    Some(name.strip_suffix(".exe").unwrap_or(&name).to_string())
}

fn shell_guard_reason(command: &str) -> Option<&'static str> {
    normalized_command_segments(command)
        .into_iter()
        .find_map(|segment| {
            GUARDED_COMMANDS
                .iter()
                .find(|(prefix, _)| segment.starts_with(prefix))
                .map(|(_, reason)| *reason)
        })
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
                .map(|value| value.strip_prefix("sudo ").unwrap_or(value).trim())
                .map(ToString::to_string)
                .collect();
        }
    }
    vec![segment.to_string()]
}

fn pattern_matches(tool: &str, pattern: &str, candidate: &str) -> bool {
    let candidate = if tool == "run_command" {
        Cow::Borrowed(candidate)
    } else {
        Cow::Owned(crate::util::path::normalize_path_sep(candidate))
    };
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
    fn command_candidates_keep_native_argument_boundaries_and_shell_identity() {
        use crate::tool::builtin::run_command::command_permission_candidate;

        let native =
            serde_json::json!({"program":"printf","args":["text & rm -rf literal"],"cwd":"."});
        let shell =
            serde_json::json!({"shell":"sh","command":"printf text & rm -rf literal","cwd":"."});
        assert_eq!(
            None,
            guarded_command_candidate_reason(&command_permission_candidate(&native))
        );
        assert_eq!(
            Some("recursive force delete is destructive"),
            guarded_command_candidate_reason(&command_permission_candidate(&shell))
        );
        let candidate = |args| command_permission_candidate(&args);
        let granted = candidate(serde_json::json!({"program":"name","args":["one two"],"cwd":"."}));
        let grants = [SessionGrant {
            tool: "run_command".into(),
            pattern: granted.clone(),
        }];
        for request in [
            serde_json::json!({"program":"name","args":["one","two"],"cwd":"."}),
            serde_json::json!({"program":"name","args":["one two"],"cwd":"subdir"}),
        ] {
            let decision = decide_tool_call(
                "run_command",
                "command",
                &candidate(request),
                &empty_policy(),
                &grants,
            );
            assert_eq!(GateDecisionKind::Ask, decision.kind);
        }
        let decision =
            decide_tool_call("run_command", "command", &granted, &empty_policy(), &grants);
        assert_eq!(GateDecisionKind::Allow, decision.kind);
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
