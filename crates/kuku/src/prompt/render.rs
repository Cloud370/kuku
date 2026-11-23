use regex::Regex;

use crate::error::{Error, Result};
use crate::prompt::PromptCatalog;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectContextInput {
    pub(crate) workspace_root: String,
    pub(crate) platform: String,
    pub(crate) current_date: String,
    pub(crate) project_instructions_rendered: String,
    pub(crate) model_tiers_rendered: String,
}

pub(crate) fn render_project_context(
    template: &str,
    input: &ProjectContextInput,
) -> Result<String> {
    validate_placeholders(
        template,
        &[
            "workspace_root",
            "platform",
            "current_date",
            "project_instructions_rendered",
            "model_tiers_rendered",
        ],
    )?;

    let mut rendered = template.to_string();
    rendered = rendered.replace("{{workspace_root}}", &input.workspace_root);
    rendered = rendered.replace("{{platform}}", &input.platform);
    rendered = rendered.replace("{{current_date}}", &input.current_date);
    rendered = rendered.replace(
        "{{project_instructions_rendered}}",
        &input.project_instructions_rendered,
    );
    rendered = rendered.replace("{{model_tiers_rendered}}", &input.model_tiers_rendered);
    Ok(rendered)
}

pub(crate) fn render_runtime_date(catalog: &PromptCatalog, current_date: &str) -> Result<String> {
    let template = catalog
        .runtime
        .get("current-date")
        .ok_or_else(|| Error::PromptRender("missing current-date runtime template".into()))?;
    validate_placeholders(&template.text, &["current_date"])?;
    Ok(template.text.replace("{{current_date}}", current_date))
}

pub(crate) fn project_policy_has_date(text: &str, template: &str, current_date: &str) -> bool {
    if !template.contains("{{current_date}}") {
        return false;
    }
    let mut pattern = String::from(r"\A");
    let mut rest = template;
    while let Some((literal, placeholder)) = rest.split_once("{{") {
        pattern.push_str(&regex::escape(literal));
        let Some((name, tail)) = placeholder.split_once("}}") else {
            return false;
        };
        match name {
            "current_date" => pattern.push_str(&regex::escape(current_date)),
            "workspace_root"
            | "platform"
            | "project_instructions_rendered"
            | "model_tiers_rendered" => {
                pattern.push_str("(?s:.*?)");
            }
            _ => return false,
        }
        rest = tail;
    }
    pattern.push_str(&regex::escape(rest));
    pattern.push_str(r"\z");
    Regex::new(&pattern).is_ok_and(|pattern| pattern.is_match(text))
}

fn validate_placeholders(template: &str, known: &[&str]) -> Result<()> {
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let after_start = &rest[start + 2..];
        let Some(end) = after_start.find("}}") else {
            break;
        };
        let name = &after_start[..end];
        if !known.contains(&name) {
            return Err(Error::PromptRender(format!(
                "unknown template variable: {name}"
            )));
        }
        rest = &after_start[end + 2..];
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_project_context_placeholders() {
        let input = ProjectContextInput {
            workspace_root: "/code/kuku/kuku".to_string(),
            platform: "linux".to_string(),
            current_date: "2026-05-18".to_string(),
            project_instructions_rendered: "No project instructions found.".to_string(),
            model_tiers_rendered: "No model tiers configured.".to_string(),
        };

        let template = "Workspace: {{workspace_root}}\nPlatform: {{platform}}";
        let rendered = render_project_context(template, &input).unwrap();
        assert!(rendered.contains("/code/kuku/kuku"));
        assert!(rendered.contains("linux"));
    }

    #[test]
    fn reports_unknown_template_variables() {
        let input = ProjectContextInput {
            workspace_root: "".into(),
            platform: "".into(),
            current_date: "".into(),
            project_instructions_rendered: "".into(),
            model_tiers_rendered: "".into(),
        };
        let error = render_project_context("{{missing_key}}", &input).unwrap_err();
        assert!(error.to_string().contains("missing_key"));
    }

    #[test]
    fn renders_current_date_from_the_runtime_asset() {
        let rendered =
            render_runtime_date(&crate::prompt::builtin_prompt_catalog(), "2026-05-18").unwrap();
        assert!(rendered.contains("Current date: 2026-05-18"));
        assert!(rendered.starts_with("<kuku_runtime_date>"));
    }

    #[test]
    fn rejects_unknown_runtime_date_placeholders() {
        let mut catalog = crate::prompt::builtin_prompt_catalog();
        catalog.runtime.get_mut("current-date").unwrap().text = "{{timezone}}".into();

        let error = render_runtime_date(&catalog, "2026-05-18").unwrap_err();

        assert!(error.to_string().contains("timezone"));
    }

    #[test]
    fn project_date_matches_the_rendered_template_not_incidental_dates() {
        let template =
            "Project policy\n- Date: {{current_date}}\n{{project_instructions_rendered}}";
        assert!(project_policy_has_date(
            "Project policy\n- Date: 2026-05-19\nfirst note\nsecond note",
            template,
            "2026-05-19"
        ));
        for text in [
            "Project policy\n- Date: 2026-05-18\nnotes dated 2026-05-19",
            "memory recorded on 2026-05-19",
            "Project policy\n- Date: 2026-05-19",
            "- Date: 2026-05-19",
            "",
        ] {
            assert!(!project_policy_has_date(text, template, "2026-05-19"));
        }
        assert!(!project_policy_has_date("2026-05-19", "", "2026-05-19"));
        assert!(!project_policy_has_date(
            "2026-05-19",
            "{{broken {{current_date}}",
            "2026-05-19"
        ));
    }
}
