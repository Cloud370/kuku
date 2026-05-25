use crate::error::{Error, Result};

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

/// Render the runtime_context wrapper with dynamic blocks (catalog, notices) inserted.
pub(crate) fn render_runtime_context(template: &str, blocks: &str) -> Result<String> {
    let rendered = template.replace("{{runtime_blocks}}", blocks);
    Ok(rendered)
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
    fn renders_runtime_context_wrapper() {
        let template = "<kuku_runtime_context>\n{{runtime_blocks}}\n</kuku_runtime_context>";
        let rendered =
            render_runtime_context(template, "<kuku_agent_catalog>...</kuku_agent_catalog>")
                .unwrap();
        assert!(rendered.contains("<kuku_agent_catalog>"));
        assert!(rendered.contains("<kuku_runtime_context>"));
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
}
