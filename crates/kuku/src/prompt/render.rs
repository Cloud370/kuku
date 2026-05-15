use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyntheticUserTemplateInput {
    pub(crate) workspace_root: String,
    pub(crate) platform: String,
    pub(crate) current_date: String,
    pub(crate) project_instructions_rendered: String,
    pub(crate) global_memory_rendered: String,
    pub(crate) project_memory_rendered: String,
    pub(crate) current_task_rendered: String,
}

pub(crate) fn render_synthetic_user(
    template: &str,
    input: &SyntheticUserTemplateInput,
) -> Result<String> {
    let mut rendered = template.to_string();
    for (name, value) in [
        ("workspace_root", input.workspace_root.as_str()),
        ("platform", input.platform.as_str()),
        ("current_date", input.current_date.as_str()),
        (
            "project_instructions_rendered",
            input.project_instructions_rendered.as_str(),
        ),
        (
            "global_memory_rendered",
            input.global_memory_rendered.as_str(),
        ),
        (
            "project_memory_rendered",
            input.project_memory_rendered.as_str(),
        ),
        (
            "current_task_rendered",
            input.current_task_rendered.as_str(),
        ),
    ] {
        rendered = rendered.replace(&format!("{{{{{name}}}}}"), value);
    }

    if let Some(start) = rendered.find("{{") {
        if let Some(end) = rendered[start + 2..].find("}}") {
            let name = &rendered[start + 2..start + 2 + end];
            return Err(Error::PromptRender(format!(
                "missing template variable: {name}"
            )));
        }
    }

    Ok(rendered)
}

#[cfg(test)]
mod tests {
    use super::{render_synthetic_user, SyntheticUserTemplateInput};

    #[test]
    fn renders_synthetic_user_placeholders_and_reports_missing_keys() {
        let input = SyntheticUserTemplateInput {
            workspace_root: "/code/kuku/kuku".to_string(),
            platform: "linux".to_string(),
            current_date: "2026-05-14".to_string(),
            project_instructions_rendered: "No project instructions found.".to_string(),
            global_memory_rendered: "No global memory.".to_string(),
            project_memory_rendered: "No project memory.".to_string(),
            current_task_rendered: "No current task framing.".to_string(),
        };

        let rendered = render_synthetic_user(
            "Workspace: {{workspace_root}}\nPlatform: {{platform}}\nDate: {{current_date}}",
            &input,
        )
        .unwrap();
        assert!(rendered.contains("/code/kuku/kuku"));
        assert!(rendered.contains("linux"));

        let error = render_synthetic_user("{{missing_key}}", &input).unwrap_err();
        assert_eq!(
            error.to_string(),
            "prompt render error: missing template variable: missing_key"
        );
    }
}
