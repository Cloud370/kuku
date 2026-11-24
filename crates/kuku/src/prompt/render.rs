use std::borrow::Cow;
use std::io::{self, Write};
use std::num::NonZeroUsize;
use std::sync::{Arc, LazyLock, Mutex};

use lru::LruCache;
use minijinja::{Environment, UndefinedBehavior, Value, syntax::SyntaxConfig};

use regex::Regex;

use crate::error::{Error, Result};
use crate::prompt::PromptCatalog;

#[derive(Clone)]
pub(crate) struct PromptRenderer(Arc<Environment<'static>>);

impl PromptCatalog {
    /// Render a handoff with the retained summary and the tools available to its recipient.
    pub fn render_handoff(
        &self,
        summary: &str,
        tools: &[crate::context::ToolSchema],
    ) -> Result<String> {
        let names: Vec<_> = tools.iter().map(|tool| tool.name.as_str()).collect();
        self.render(
            "runtime/handoff-context.md",
            minijinja::context! {
                handoff_summary => summary,
                tools => minijinja::value::Serde(&names),
            },
        )
    }

    pub(crate) fn render(&self, name: &str, facts: Value) -> Result<String> {
        self.renderer().render(name, facts)
    }

    pub(crate) fn renderer(&self) -> PromptRenderer {
        static RENDERERS: LazyLock<Mutex<LruCache<PromptCatalog, PromptRenderer>>> =
            LazyLock::new(|| Mutex::new(LruCache::new(NonZeroUsize::new(32).unwrap())));
        let mut cache = RENDERERS.lock().unwrap();
        if let Some(renderer) = cache.get(self) {
            return renderer.clone();
        }
        let renderer = PromptRenderer::new(self.clone());
        cache.put(self.clone(), renderer.clone());
        renderer
    }
}

impl PromptRenderer {
    fn new(catalog: PromptCatalog) -> Self {
        static BUILTIN: LazyLock<PromptCatalog> =
            LazyLock::new(crate::prompt::builtin_prompt_catalog);
        let mut environment = Environment::new();
        environment.set_undefined_behavior(UndefinedBehavior::Strict);
        environment.set_fuel(Some(1_000_000));
        environment.set_syntax(
            SyntaxConfig::builder()
                .keep_trailing_newline(true)
                .trim_blocks(true)
                .lstrip_blocks(true)
                .build()
                .expect("default Prompt template syntax"),
        );
        environment.set_loader(move |name| {
            Ok(catalog
                .template_asset(name)
                .or_else(|| BUILTIN.template_asset(name))
                .map(|asset| template_source(name, &asset.text)))
        });
        Self(Arc::new(environment))
    }

    pub(crate) fn render(&self, name: &str, facts: Value) -> Result<String> {
        let error = |err: minijinja::Error| Error::PromptRender(format!("{name}: {err}"));
        let template = self.0.get_template(name).map_err(error)?;
        let mut output = PromptOutput::default();
        template
            .render_captured_to(facts, &mut output)
            .map_err(error)?;
        String::from_utf8(output.0).map_err(|err| Error::PromptRender(format!("{name}: {err}")))
    }
}

#[derive(Default)]
struct PromptOutput(Vec<u8>);

impl Write for PromptOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.0.len().saturating_add(bytes.len())
            > crate::session::RuntimeLimits::default().context_replay_bytes()
        {
            return Err(io::Error::other(
                "rendered Prompt exceeds the context replay byte limit",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn template_source(name: &str, text: &str) -> String {
    match name {
        "blocks/tool-guidance.md" => {
            let mut source = String::new();
            for line in text.split_inclusive('\n') {
                if let Some(rest) = line.strip_prefix("<!-- tools: ") {
                    let Some((names, body)) = rest.split_once(" -->") else {
                        continue;
                    };
                    let condition = names
                        .split_whitespace()
                        .map(|name| format!("{} in tools", serde_json::json!(name)))
                        .collect::<Vec<_>>()
                        .join(" and ");
                    if condition.is_empty() {
                        source.push_str(body);
                    } else {
                        source.push_str(&format!("{{% if {condition} %}}{body}{{% endif %}}"));
                    }
                } else {
                    source.push_str(line);
                }
            }
            source
        }
        "blocks/delegation.md" if !text.contains("{%") => format!(
            "{{% if \"create_agent\" in tools and \"send_message\" in tools and \"list_agents\" in tools %}}{text}{{% endif %}}"
        ),
        "blocks/project-policy.md" if text.contains("{{model_tiers_rendered}}") => {
            model_tiers_section()
                .replace_all(text, |captures: &regex::Captures<'_>| {
                    format!(
                        "{{% if models is not none %}}\n{}{{% endif %}}",
                        &captures[0]
                    )
                })
                .into_owned()
        }
        _ => text.to_owned(),
    }
}

pub(crate) fn render_runtime_date(catalog: &PromptCatalog, current_date: &str) -> Result<String> {
    catalog.renderer().render(
        "runtime/current-date.md",
        minijinja::context! { current_date },
    )
}

pub(crate) fn project_policy_has_date(text: &str, template: &str, current_date: &str) -> bool {
    if !template.contains("{{current_date}}") {
        return false;
    }
    if matches_project_policy_date(text, template, current_date) {
        return true;
    }
    if let Cow::Owned(template) = without_model_tiers(template) {
        return matches_project_policy_date(text, &template, current_date);
    }
    false
}

fn model_tiers_section() -> &'static Regex {
    static MODEL_TIERS_SECTION: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\r?\n<kuku_models>\r?\n\{\{model_tiers_rendered\}\}\r?\n</kuku_models>\r?\n")
            .expect("model tiers section pattern is valid")
    });
    &MODEL_TIERS_SECTION
}

fn without_model_tiers(template: &str) -> Cow<'_, str> {
    model_tiers_section().replace_all(template, "")
}

fn matches_project_policy_date(text: &str, template: &str, current_date: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn render_project(template: &str, facts: Value) -> Result<String> {
        let mut catalog = crate::prompt::builtin_prompt_catalog();
        catalog.blocks.get_mut("project-policy").unwrap().text = template.to_owned();
        catalog.renderer().render("blocks/project-policy.md", facts)
    }

    #[test]
    fn conditional_guidance_follows_the_selected_tools_and_permission_mode() {
        let renderer = crate::prompt::builtin_prompt_catalog().renderer();
        let messaging = renderer
            .render(
                "blocks/delegation.md",
                minijinja::context! {
                    tools => vec!["send_message", "list_agents"],
                },
            )
            .unwrap();
        assert!(messaging.contains("sharing a saved value"));
        assert!(messaging.contains("list_agents"));
        assert!(!messaging.contains("Create an idle Agent"));

        let permission = |mode| {
            renderer
                .render(
                    "runtime/permission-mode.md",
                    minijinja::context! {
                        step => minijinja::context! { permission_mode => mode },
                        mode => if mode == "ask" { "Ask" } else { "Full" },
                        tools => vec!["run_js"],
                    },
                )
                .unwrap()
        };
        let ask = permission("ask");
        let full = permission("full");
        assert!(ask.contains("may wait for approval at its"));
        assert!(full.contains("Current workspace permission mode: Full."));
        assert!(!full.contains("may wait for approval at its"));

        for (tools, can_query) in [(vec!["query_session"], true), (vec![], false)] {
            let handoff = renderer
                .render(
                    "runtime/handoff-context.md",
                    minijinja::context! {
                        handoff_summary => "Earlier user text: {{ literal }}",
                        tools,
                    },
                )
                .unwrap();
            assert_eq!(handoff.contains("Use the `query_session` tool"), can_query);
            assert!(handoff.contains("{{ literal }}"));
        }
    }

    #[test]
    fn catalog_overrides_can_include_other_registered_templates() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("blocks")).unwrap();
        std::fs::write(
            dir.path().join("system.md"),
            "{% include \"blocks/extra.md\" %}",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("blocks/extra.md"),
            "{% if enabled %}{{ text }}{% endif %}",
        )
        .unwrap();
        let renderer = PromptCatalog::load_from_dir(dir.path()).unwrap().renderer();
        let render = |enabled| {
            renderer
                .render(
                    "system.md",
                    minijinja::context! {
                        enabled,
                        text => "{{ literal }}",
                    },
                )
                .unwrap()
        };
        assert_eq!(render(true), "{{ literal }}");
        assert_eq!(render(false), "");
    }

    #[test]
    fn renders_project_context_placeholders_without_interpreting_values() {
        let rendered = render_project(
            "Workspace: {{ workspace_root }}\nPlatform: {{ platform }}",
            minijinja::context! {
                workspace_root => "/code/{{untrusted}}",
                platform => "linux",
            },
        )
        .unwrap();
        assert_eq!(rendered, "Workspace: /code/{{untrusted}}\nPlatform: linux");
    }

    #[test]
    fn reports_unknown_template_variables() {
        let error = render_project("{{missing_key}}", minijinja::context! {}).unwrap_err();
        assert!(error.to_string().contains("missing_key"));
        assert!(error.to_string().contains("blocks/project-policy.md"));
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
    fn project_date_matches_policy_with_model_tiers_removed() {
        let template = "Project policy\r\n- Date: {{current_date}}\r\n\r\n<kuku_models>\r\n{{model_tiers_rendered}}\r\n</kuku_models>\r\nEnd of policy";
        let without_tiers = render_project(
            template,
            minijinja::context! { current_date => "2026-05-19", models => Option::<Vec<String>>::None },
        ).unwrap();
        assert_eq!(
            without_tiers,
            "Project policy\r\n- Date: 2026-05-19\r\nEnd of policy"
        );
        let with_tiers = render_project(
            template,
            minijinja::context! { current_date => "2026-05-19", models => vec!["balanced"], model_tiers_rendered => "balanced" },
        ).unwrap();
        assert!(with_tiers.contains("<kuku_models>\r\nbalanced\r\n</kuku_models>"));
        for text in [without_tiers, with_tiers] {
            assert!(project_policy_has_date(&text, template, "2026-05-19"));
            assert!(!project_policy_has_date(&text, template, "2026-05-20"));
        }
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
