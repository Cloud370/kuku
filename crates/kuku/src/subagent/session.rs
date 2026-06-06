use std::path::Path;
use std::sync::Arc;

use crate::error::Result;
use crate::query::PermissionMode;

use super::definition::SubagentDefinition;

/// Start a child session and return the Run handle without blocking.
/// For TUI/interactive use — the caller polls child events via Run::next().
// Each parameter comes from a different source (config, registry, caller, filesystem);
// bundling them into a struct would obscure which call sites provide which data.
#[allow(clippy::too_many_arguments)]
pub async fn start_child_session(
    _parent_session_dir: &Path,
    child_session_id: &str,
    definition: &SubagentDefinition,
    delegated_prompt: &str,
    workspace: &Path,
    kuku_home: &Path,
    config: Arc<crate::config::Config>,
    prompts_dir: Option<&Path>,
    _parent_mode: PermissionMode,
    _child_session_count: u32,
) -> Result<crate::query::Run> {
    let definition_block = super::catalog::render_agent_definition_block(definition);
    let user_prompt = format!(
        "{definition_block}\n\n<kuku_delegated_prompt>\n{delegated_prompt}\n</kuku_delegated_prompt>"
    );

    let full_registry = crate::tool::builtin_registry(false, false);
    let constrained_registry: Vec<crate::tool::ToolDefinition> = match &definition.tools {
        None => full_registry,
        Some(list) => full_registry
            .into_iter()
            .filter(|t| list.contains(&t.name))
            .collect(),
    };

    let mut query = crate::query::Query::new(user_prompt)
        .workspace(workspace.to_path_buf())
        .tier(definition.tier.clone())
        .config((*config).clone())
        .no_agents();

    query.captured_kuku_home = Some(kuku_home.to_path_buf());
    query.tool_registry_override = Some(constrained_registry);

    if let Some(dir) = prompts_dir {
        query = query.prompts_dir(dir.to_path_buf());
    }

    query.session(child_session_id.to_string()).start().await
}

#[cfg(all(test, feature = "test_support"))]
mod tests {
    use httpmock::prelude::*;

    use crate::event::{EventPayload, EventStore};
    use crate::query::UiEvent;

    use super::*;

    fn test_config(base_url: String) -> crate::config::Config {
        use crate::config::{
            ApiKey, Config, DiscoveryConfig, HandoffConfig, ProviderConfig, ThinkLevel, TierConfig,
        };
        use std::collections::BTreeMap;

        let mut tiers = BTreeMap::new();
        tiers.insert(
            "balanced".to_string(),
            TierConfig {
                provider: "anthropic".to_string(),
                model: "claude-sonnet-4-6".to_string(),
                think: ThinkLevel::Medium,
                context_window: 200_000,
                max_output_tokens: 48_000,
                purpose: "balanced".to_string(),
            },
        );

        let mut providers = BTreeMap::new();
        providers.insert(
            "anthropic".to_string(),
            ProviderConfig {
                format: crate::config::ProviderFormat::Anthropic,
                base_url,
                api_key: ApiKey::Plaintext("test-key".to_string()),
            },
        );

        Config {
            tiers,
            providers,
            default_tier: "balanced".to_string(),
            discovery: DiscoveryConfig::default(),
            handoff: HandoffConfig::default(),
            logs: crate::config::LogsConfig::default(),
            plugin: crate::config::PluginConfig::default(),
            update: crate::config::UpdateConfig::default(),
        }
    }

    fn command_agent() -> SubagentDefinition {
        SubagentDefinition {
            name: "runner".into(),
            description: "run commands".into(),
            instructions: "Run commands when delegated.".into(),
            tier: "balanced".into(),
            tool_profile: crate::subagent::definition::ToolProfile::ReadWrite,
            tools: Some(vec!["run_command".into()]),
            max_turns: 10,
            source: crate::subagent::definition::DefinitionSource::Project,
            hash: String::new(),
            source_path: None,
            metadata: serde_json::Value::Null,
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn child_permission_request_is_persisted_in_child_session_events() {
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        let workspace_alias = home.path().join("workspace-link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(workspace.path(), &workspace_alias).unwrap();
        #[cfg(not(unix))]
        let workspace_alias = workspace.path().to_path_buf();
        let server = MockServer::start();

        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .body_contains("<kuku_delegated_prompt>");
            then.status(200)
                .body(crate::test_support::anthropic_sse_response(serde_json::json!({
                    "id": "msg_child_tool",
                    "type": "message",
                    "role": "assistant",
                    "content": [
                        {"type": "text", "text": "Need child approval."},
                        {"type": "tool_use", "id": "toolu_child_cmd", "name": "run_command", "input": {"command": "cargo test", "timeout": 60, "brief": "run tests"}}
                    ],
                    "stop_reason": "tool_use",
                    "usage": {"input_tokens": 5, "output_tokens": 6}
                })));
        });

        let child_session_id = "child_s_permission_persistence_0";
        let mut run = start_child_session(
            home.path(),
            child_session_id,
            &command_agent(),
            "run the tests",
            &workspace_alias,
            home.path(),
            std::sync::Arc::new(test_config(server.base_url())),
            None,
            PermissionMode::AutoAllow,
            1,
        )
        .await
        .unwrap();

        let request = loop {
            let event = run.next().await.unwrap().expect("event");
            if let UiEvent::PermissionRequested { request } = event {
                break request;
            }
        };

        assert_eq!(request.tool_call_id, "toolu_child_cmd");
        assert_eq!(request.tool, "run_command");

        let events_path =
            crate::session::session_events_path(home.path(), &workspace_alias, child_session_id)
                .unwrap();
        let child_events = EventStore::replay(events_path).unwrap();
        let child_permission = child_events
            .iter()
            .find(|event| {
                matches!(event.payload, EventPayload::PermissionRequested { ref tool_call_id, .. } if tool_call_id == "toolu_child_cmd")
            })
            .expect("child permission.requested event");

        match &child_permission.payload {
            EventPayload::PermissionRequested {
                turn,
                tool_call_id,
                tool,
                source,
                ..
            } => {
                assert_eq!(*turn, 1);
                assert_eq!(tool_call_id, "toolu_child_cmd");
                assert_eq!(tool, "run_command");
                assert_eq!(source, "default_ask");
            }
            other => panic!("expected child permission.requested, got {other:?}"),
        }

        assert!(!child_events.iter().any(|event| {
            let payload = serde_json::to_value(&event.payload).unwrap();
            payload.get("log").is_some() || payload.get("debug").is_some()
        }));
    }
}
