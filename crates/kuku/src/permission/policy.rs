use std::path::Path;

use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionPolicy {
    allow: Vec<PolicyRule>,
    deny: Vec<PolicyRule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PolicyRule {
    tool: String,
    pattern: String,
}

pub fn parse_policy(markdown: &str) -> Result<PermissionPolicy> {
    let mut allow = Vec::new();
    let mut deny = Vec::new();
    let mut section: Option<&str> = None;

    for line in markdown.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed == "# policy" {
            continue;
        }
        if trimmed == "## allow" {
            section = Some("allow");
            continue;
        }
        if trimmed == "## deny" {
            section = Some("deny");
            continue;
        }
        if !trimmed.starts_with("- ") {
            continue;
        }

        let section = section
            .ok_or_else(|| Error::InvalidPolicy(format!("rule outside section: {trimmed}")))?;
        let rule = parse_rule(&trimmed[2..])?;
        match section {
            "allow" => allow.push(rule),
            "deny" => deny.push(rule),
            _ => unreachable!(),
        }
    }

    Ok(PermissionPolicy { allow, deny })
}

impl PermissionPolicy {
    pub fn matches_allow(&self, tool: &str, candidate: &str) -> bool {
        self.allow.iter().any(|rule| rule.matches(tool, candidate))
    }

    pub fn matches_deny(&self, tool: &str, candidate: &str) -> bool {
        self.deny.iter().any(|rule| rule.matches(tool, candidate))
    }
}

pub fn load_project_policy(path: &Path) -> Result<PermissionPolicy> {
    match std::fs::read_to_string(path) {
        Ok(markdown) => parse_policy(&markdown),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(PermissionPolicy {
            allow: Vec::new(),
            deny: Vec::new(),
        }),
        Err(error) => Err(error.into()),
    }
}

pub fn append_project_allow_rule(path: &Path, tool: &str, pattern: &str) -> Result<()> {
    let policy = load_project_policy(path)?;
    let mut content = String::from("# policy\n\n## allow\n");
    for rule in &policy.allow {
        content.push_str(&format!("- {}({})\n", rule.tool, rule.pattern));
    }
    content.push_str(&format!("- {}({})\n", tool, pattern));
    content.push_str("\n## deny\n");
    for rule in &policy.deny {
        content.push_str(&format!("- {}({})\n", rule.tool, rule.pattern));
    }
    std::fs::write(path, content)?;
    Ok(())
}

impl PolicyRule {
    fn matches(&self, tool: &str, candidate: &str) -> bool {
        if self.tool != tool {
            return false;
        }
        if let Some(prefix) = self.pattern.strip_suffix("/**") {
            return candidate == prefix || candidate.starts_with(&format!("{prefix}/"));
        }
        if let Some(prefix) = self.pattern.strip_suffix('*') {
            return candidate.starts_with(prefix);
        }
        candidate == self.pattern
    }
}

fn parse_rule(value: &str) -> Result<PolicyRule> {
    let open = value
        .find('(')
        .ok_or_else(|| Error::InvalidPolicy(format!("invalid rule: {value}")))?;
    let close = value
        .rfind(')')
        .ok_or_else(|| Error::InvalidPolicy(format!("invalid rule: {value}")))?;
    if close <= open + 1 || close != value.len() - 1 {
        return Err(Error::InvalidPolicy(format!("invalid rule: {value}")));
    }

    let tool = value[..open].trim();
    let pattern = value[open + 1..close].trim();
    if tool.is_empty() || pattern.is_empty() {
        return Err(Error::InvalidPolicy(format!("invalid rule: {value}")));
    }

    Ok(PolicyRule {
        tool: tool.to_string(),
        pattern: pattern.to_string(),
    })
}
