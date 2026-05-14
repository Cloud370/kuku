use crate::event::{EventPayload, StoredEvent};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionGrant {
    pub tool: String,
    pub pattern: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateDecisionKind {
    Allow,
    Deny,
    Ask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateScope {
    Once,
    Session,
    Project,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateSource {
    HardGuard,
    ProjectPolicy,
    SessionGrant,
    TrustPosture,
    Host,
    DefaultAsk,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateDecision {
    pub kind: GateDecisionKind,
    pub scope: GateScope,
    pub source: GateSource,
    pub rule: String,
}

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

    if risk == "read" {
        return GateDecision {
            kind: GateDecisionKind::Allow,
            scope: GateScope::Once,
            source: GateSource::TrustPosture,
            rule: "read".to_string(),
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

    GateDecision {
        kind: GateDecisionKind::Ask,
        scope: GateScope::Once,
        source: GateSource::DefaultAsk,
        rule: candidate.to_string(),
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
    candidate
        .split('/')
        .any(|part| part == ".git" || part == ".ssh")
        || candidate.split('/').any(|part| part == ".kuku") && candidate.ends_with("/policy.md")
        || is_sensitive_file_name(candidate.rsplit('/').next().unwrap_or(candidate))
}

fn is_sensitive_file_name(name: &str) -> bool {
    matches!(
        name,
        ".env" | "id_rsa" | "id_dsa" | "id_ecdsa" | "id_ed25519"
    ) || name.starts_with(".env.")
        || name.ends_with(".pem")
        || name.ends_with(".key")
}

fn is_hard_guarded_command(candidate: &str) -> bool {
    normalized_command_segments(candidate)
        .into_iter()
        .any(|segment| is_dangerous_command_segment(&segment))
}

fn normalized_command_segments(candidate: &str) -> Vec<String> {
    let normalized = candidate
        .to_ascii_lowercase()
        .replace("&&", "\n")
        .replace("||", "\n")
        .replace(';', "\n");

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
            return rest
                .replace("&&", "\n")
                .replace("||", "\n")
                .replace(';', "\n")
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
    if let Some(prefix) = pattern.strip_suffix("/**") {
        return candidate == prefix || candidate.starts_with(&format!("{prefix}/"));
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return candidate.starts_with(prefix);
    }
    candidate == pattern
}
