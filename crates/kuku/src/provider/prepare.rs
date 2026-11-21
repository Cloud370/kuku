use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::config::{PromptCacheScope, ProviderAuth, ProviderFormat, ProviderProfile};
use crate::context::MessageBlock;

use super::types::{
    PreparedProviderRequest, ProviderCorrelation, ProviderFailure, ProviderFailureKind,
    ProviderKind, ProviderRequest, ResolvedProvider,
};

pub(crate) fn prepare_provider_request(
    config: &ResolvedProvider,
    request: &ProviderRequest<'_>,
    correlation: Option<&ProviderCorrelation>,
) -> Result<PreparedProviderRequest, ProviderFailure> {
    if request.model != config.model {
        return Err(invalid_request(
            "request model differs from the resolved connection",
        ));
    }
    if correlation.is_some_and(|value| value.connection_name != config.connection_name) {
        return Err(invalid_request(
            "transport correlation belongs to another connection",
        ));
    }
    if config.output == crate::config::RequestedOutput::Image {
        return Err(invalid_request(
            "This Agent requests image output. The built-in runtime supports text output only.",
        ));
    }
    if config.capabilities.tool_calls == Some(false) && !request.assembly.tools.is_empty() {
        return Err(invalid_request(
            "This OpenAI model requires Responses for tool calls. Switch the model route to Responses.",
        ));
    }
    validate_replay(config, request)?;
    let url = request_url(config)?;
    let mut body = match config.kind {
        ProviderKind::Anthropic => super::anthropic::render_body(request),
        ProviderKind::OpenAiCompatible => super::openai_compat::render_body(request),
        ProviderKind::OpenAiResponses => super::openai_responses::render_body(request),
    };
    body["stream"] = json!(true);
    apply_settings(config, &mut body);
    let correlation_id = correlation.map(correlation_id);
    if let Some(cache_key) = prompt_cache_key(config, correlation, &url) {
        body["prompt_cache_key"] = json!(cache_key);
    }
    if config.capabilities.profile == ProviderProfile::OpenRouter {
        if let Some(value) = &correlation_id {
            body["session_id"] = json!(value);
        }
        body["provider"] = serde_json::to_value(config.options.routing.clone().unwrap_or_default())
            .map_err(|_| invalid_request("could not encode OpenRouter routing options"))?;
    }
    Ok(PreparedProviderRequest {
        kind: config.kind,
        connection_name: config.connection_name.clone(),
        model: config.model.clone(),
        url,
        body,
        settings: config.settings.clone(),
        correlation_id,
    })
}

pub(crate) fn request_url(config: &ResolvedProvider) -> Result<String, ProviderFailure> {
    crate::config::resolve_request_url(
        config.kind.format(),
        &config.base_url,
        config.options.endpoint_url.as_deref(),
    )
    .map_err(|error| invalid_request(&error.to_string()))
}

pub(super) fn is_official_openrouter(url: &url::Url) -> bool {
    url.scheme() == "https"
        && matches!(
            url.host_str(),
            Some("openrouter.ai" | "us.openrouter.ai" | "eu.openrouter.ai")
        )
        && url.port_or_known_default() == Some(443)
}

pub(crate) fn authentication(config: &ResolvedProvider) -> ProviderAuth {
    authentication_for(
        config.kind.format(),
        config.capabilities.profile,
        &config.options,
    )
}

pub(crate) fn authentication_for(
    format: ProviderFormat,
    profile: ProviderProfile,
    options: &crate::config::ProviderOptions,
) -> ProviderAuth {
    options.auth.unwrap_or_else(|| {
        if profile == ProviderProfile::OpenRouter {
            ProviderAuth::Bearer
        } else if format == ProviderFormat::Anthropic {
            ProviderAuth::ApiKey
        } else {
            ProviderAuth::Bearer
        }
    })
}

pub(crate) fn invalid_request(message: &str) -> ProviderFailure {
    ProviderFailure {
        kind: ProviderFailureKind::InvalidRequest,
        message: message.into(),
        status: None,
        provider_request_id: None,
        retry_after: None,
        retryable: false,
    }
}

pub(crate) fn invalid_response(message: &str) -> ProviderFailure {
    ProviderFailure {
        kind: ProviderFailureKind::InvalidResponse,
        message: message.into(),
        status: None,
        provider_request_id: None,
        retry_after: None,
        retryable: false,
    }
}

pub(super) fn apply_settings(config: &ResolvedProvider, body: &mut Value) {
    let object = body
        .as_object_mut()
        .expect("protocol renderer returns an object");
    for key in [
        "thinking",
        "reasoning",
        "reasoning_effort",
        "output_config",
        "service_tier",
    ] {
        object.remove(key);
    }
    if config.capabilities.profile == ProviderProfile::DeepSeek {
        object.remove("cache_control");
    } else if config.capabilities.prompt_cache_control {
        body["cache_control"] = json!({"type": "ephemeral"});
    }
    if let Some(effort) = config.settings.reasoning_effort.as_deref() {
        match config.kind {
            ProviderKind::Anthropic => {
                body["thinking"] = if effort == "none" {
                    json!({"type": "disabled"})
                } else if config.capabilities.profile == ProviderProfile::DeepSeek {
                    json!({"type": "enabled"})
                } else if config.capabilities.profile == ProviderProfile::Anthropic {
                    json!({"type": "adaptive", "display": "summarized"})
                } else {
                    json!({"type": "adaptive"})
                };
                if effort != "none" {
                    body["output_config"] = json!({"effort": effort});
                }
            }
            ProviderKind::OpenAiCompatible => {
                if config.capabilities.profile == ProviderProfile::DeepSeek {
                    body["thinking"] =
                        json!({"type": if effort == "none" {"disabled"} else {"enabled"}});
                }
                if config.capabilities.profile == ProviderProfile::OpenRouter {
                    body["reasoning"] = json!({"effort": effort});
                } else if effort != "none"
                    || config.capabilities.profile != ProviderProfile::DeepSeek
                {
                    body["reasoning_effort"] = json!(effort);
                }
            }
            ProviderKind::OpenAiResponses => {
                body["reasoning"] = json!({"effort": effort});
                if effort != "none"
                    && matches!(
                        config.capabilities.profile,
                        ProviderProfile::OpenAi | ProviderProfile::OpenRouter
                    )
                {
                    body["reasoning"]["summary"] = json!("auto");
                }
            }
        }
    }
    if let Some(service) = &config.settings.service_tier {
        body["service_tier"] = json!(service);
    }
    if config.kind == ProviderKind::OpenAiCompatible {
        body["stream_options"] = json!({"include_usage": true});
        if config.capabilities.profile == ProviderProfile::OpenAi {
            if let Some(max_tokens) = body
                .as_object_mut()
                .and_then(|body| body.remove("max_tokens"))
            {
                body["max_completion_tokens"] = max_tokens;
            }
        }
    }
    if config.kind == ProviderKind::OpenAiResponses {
        body["store"] = json!(false);
        if config.capabilities.profile == ProviderProfile::OpenAi {
            body["include"] = json!(["reasoning.encrypted_content"]);
        }
        if matches!(
            config.capabilities.profile,
            ProviderProfile::OpenAi | ProviderProfile::OpenRouter
        ) {
            if let Some(tools) = body.get_mut("tools").and_then(Value::as_array_mut) {
                for tool in tools.iter_mut().filter(|tool| tool["type"] == "function") {
                    // Preserve the runtime's original optional parameters on Responses.
                    tool["strict"] = Value::Bool(false);
                }
            }
        }
    }
}

fn correlation_id(correlation: &ProviderCorrelation) -> String {
    let mut hash = Sha256::new();
    for value in [
        &correlation.session_id,
        &correlation.agent_address,
        &correlation.connection_name,
    ] {
        hash.update((value.len() as u64).to_be_bytes());
        hash.update(value.as_bytes());
    }
    format!("kuku-{:x}", hash.finalize())
}

fn prompt_cache_key(
    config: &ResolvedProvider,
    correlation: Option<&ProviderCorrelation>,
    url: &str,
) -> Option<String> {
    if config.kind != ProviderKind::OpenAiResponses {
        return None;
    }
    let correlation = correlation?;
    let scope = config.options.prompt_cache_scope;
    if scope == PromptCacheScope::Off {
        return None;
    }

    let mut hash = Sha256::new();
    let scope_value = match scope {
        PromptCacheScope::Workspace => "workspace",
        PromptCacheScope::Session => "session",
        PromptCacheScope::Off => "off",
    };
    for value in [
        "kuku-prompt-cache-v1".as_bytes(),
        scope_value.as_bytes(),
        config.connection_name.as_bytes(),
        url.as_bytes(),
        config.model.as_bytes(),
    ] {
        update_hash_field(&mut hash, value);
    }
    match scope {
        PromptCacheScope::Workspace => {
            let workspace = std::fs::canonicalize(&correlation.workspace).ok()?;
            for component in workspace.components() {
                update_hash_field(&mut hash, component.as_os_str().as_encoded_bytes());
            }
        }
        PromptCacheScope::Session => {
            for value in [
                correlation.session_id.as_bytes(),
                correlation.agent_address.as_bytes(),
                correlation.connection_name.as_bytes(),
            ] {
                update_hash_field(&mut hash, value);
            }
        }
        PromptCacheScope::Off => return None,
    }
    Some(format!("{:x}", hash.finalize()))
}

fn update_hash_field(hash: &mut Sha256, value: &[u8]) {
    hash.update((value.len() as u64).to_be_bytes());
    hash.update(value);
}

fn validate_replay(
    config: &ResolvedProvider,
    request: &ProviderRequest<'_>,
) -> Result<(), ProviderFailure> {
    let protocol = match config.kind {
        ProviderKind::Anthropic => ProviderFormat::Anthropic,
        ProviderKind::OpenAiCompatible => ProviderFormat::OpenAiChat,
        ProviderKind::OpenAiResponses => ProviderFormat::OpenAiResponses,
    };
    for message in request
        .assembly
        .prelude_messages
        .iter()
        .chain(&request.assembly.history)
    {
        for block in &message.blocks {
            if let MessageBlock::ProviderReplay {
                protocol: source, ..
            } = block
            {
                if *source != protocol {
                    return Err(invalid_request("native replay data belongs to another protocol; prepare a successor context segment"));
                }
            }
        }
        if config.capabilities.profile == ProviderProfile::Anthropic {
            let native = super::replay::native_replay(message, protocol);
            let unsigned_native = native
                .and_then(|value| value.get("content"))
                .and_then(Value::as_array)
                .is_some_and(|blocks| {
                    blocks.iter().any(|block| {
                        block.get("type").and_then(Value::as_str) == Some("thinking")
                            && block
                                .get("signature")
                                .and_then(Value::as_str)
                                .is_none_or(str::is_empty)
                    })
                });
            if unsigned_native
                || (native.is_none()
                    && message
                        .blocks
                        .iter()
                        .any(|block| matches!(block, MessageBlock::Thinking(_))))
            {
                return Err(invalid_request(
                    "native Messages thinking replay requires its original signed block",
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{CanonicalMessage, ContextAssembly, ToolSchema};
    use crate::provider::types::{CanonicalPromptInput, ProviderSettings};

    fn assembly() -> ContextAssembly {
        ContextAssembly {
            system_prompt: "fixed system".into(),
            prelude_messages: Vec::new(),
            history: vec![CanonicalMessage::user_text("fixed input")],
            tools: Vec::new(),
            prompt_asset_sources: Vec::new(),
            project_instruction_sources: Vec::new(),
            memory_sources: Vec::new(),
            runtime_context: None,
            handoff_summary: None,
        }
    }

    fn provider_config(
        kind: ProviderKind,
        profile: ProviderProfile,
        model: &str,
    ) -> ResolvedProvider {
        let mut config = ResolvedProvider {
            connection_name: "router".into(),
            kind,
            model: model.into(),
            base_url: "https://openrouter.ai/api/v1".into(),
            ..Default::default()
        };
        config.capabilities.profile = profile;
        config
    }

    fn prepared_request(
        config: &ResolvedProvider,
        correlation: Option<&ProviderCorrelation>,
    ) -> PreparedProviderRequest {
        let catalog = crate::prompt::builtin_prompt_catalog();
        let request = ProviderRequest {
            assembly: assembly(),
            catalog: &catalog,
            current_input: CanonicalPromptInput { parts: Vec::new() },
            model: config.model.clone(),
            max_output_tokens: Some(64),
            temperature: None,
            stream: true,
            think_level: crate::config::ThinkLevel::Off,
            thinking: Default::default(),
        };
        prepare_provider_request(config, &request, correlation).unwrap()
    }

    #[test]
    fn openrouter_claude_protocols_request_automatic_prompt_caching() {
        let workspace = tempfile::tempdir().unwrap();
        let correlation = ProviderCorrelation {
            session_id: "session".into(),
            agent_address: "main".into(),
            connection_name: "router".into(),
            workspace: workspace.path().into(),
        };

        for model in [
            "anthropic/claude-sonnet-5.5",
            "~anthropic/claude-sonnet-latest",
        ] {
            for kind in [
                ProviderKind::OpenAiCompatible,
                ProviderKind::OpenAiResponses,
                ProviderKind::Anthropic,
            ] {
                let mut config = provider_config(kind, ProviderProfile::OpenRouter, model);
                config.capabilities.prompt_cache_control = true;
                let prepared = prepared_request(&config, Some(&correlation));
                assert_eq!(json!({"type": "ephemeral"}), prepared.body["cache_control"]);
                assert_eq!(model, prepared.body["model"]);
                assert_eq!(
                    prepared.correlation_id.as_deref(),
                    prepared.body["session_id"].as_str()
                );
            }
        }
    }

    #[test]
    fn gateway_cache_control_is_scoped_to_supported_routes() {
        let cases = [
            (
                ProviderKind::OpenAiCompatible,
                ProviderProfile::OpenRouter,
                "openai/gpt-5.4-mini",
            ),
            (
                ProviderKind::OpenAiCompatible,
                ProviderProfile::OpenRouter,
                "claude-sonnet-5.5",
            ),
            (
                ProviderKind::OpenAiCompatible,
                ProviderProfile::Compatible,
                "anthropic/claude-sonnet-5.5",
            ),
            (
                ProviderKind::OpenAiCompatible,
                ProviderProfile::OpenAi,
                "anthropic/claude-sonnet-5.5",
            ),
            (
                ProviderKind::OpenAiCompatible,
                ProviderProfile::DeepSeek,
                "anthropic/claude-sonnet-5.5",
            ),
            (
                ProviderKind::OpenAiCompatible,
                ProviderProfile::DeepSeek,
                "deepseek-chat",
            ),
            (
                ProviderKind::OpenAiResponses,
                ProviderProfile::OpenRouter,
                "openai/gpt-6.2-sol",
            ),
        ];

        for (kind, profile, model) in cases {
            let config = provider_config(kind, profile, model);
            let prepared = prepared_request(&config, None);
            assert!(
                prepared.body.get("cache_control").is_none(),
                "unexpected cache_control for {kind:?}, {profile:?}, {model}"
            );
        }
    }

    #[test]
    fn responses_starts_with_the_standard_developer_item() {
        for (profile, model) in [
            (ProviderProfile::OpenAi, "gpt-6.2-sol"),
            (ProviderProfile::OpenAi, "gpt-6.2-astra"),
            (ProviderProfile::Compatible, "fixture"),
        ] {
            let config = provider_config(ProviderKind::OpenAiResponses, profile, model);
            let request = prepared_request(&config, None);
            assert!(request.body.get("instructions").is_none());
            let input = request.body["input"].as_array().unwrap();
            assert_eq!(input.len(), 2);
            assert_eq!(input[0]["role"], "developer");
            assert_eq!(input[0]["content"][0]["text"], "fixed system");
            assert_eq!(input[1]["role"], "user");
        }
    }

    #[test]
    fn anthropic_messages_cache_control_is_retained_and_deepseek_omits_it() {
        let openrouter = provider_config(
            ProviderKind::Anthropic,
            ProviderProfile::OpenRouter,
            "anthropic/claude-sonnet-5.5",
        );
        let prepared = prepared_request(&openrouter, None);
        assert_eq!(json!({"type": "ephemeral"}), prepared.body["cache_control"]);

        for (kind, model) in [
            (ProviderKind::OpenAiCompatible, "deepseek-chat"),
            (ProviderKind::Anthropic, "deepseek-chat"),
        ] {
            let deepseek = provider_config(kind, ProviderProfile::DeepSeek, model);
            let prepared = prepared_request(&deepseek, None);
            assert!(prepared.body.get("cache_control").is_none());
        }
    }

    #[test]
    fn frozen_request_keeps_effective_settings_and_stable_agent_correlation() {
        let catalog = crate::prompt::builtin_prompt_catalog();
        let mut config = ResolvedProvider {
            connection_name: "router".into(),
            model: "deepseek/deepseek-v4.1-flash-1017".into(),
            base_url: "https://openrouter.ai/api/v1".into(),
            settings: ProviderSettings {
                requested_reasoning_effort: Some("low".into()),
                reasoning_effort: Some("low".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        config.capabilities.profile = ProviderProfile::OpenRouter;
        let mut request = ProviderRequest {
            assembly: assembly(),
            catalog: &catalog,
            current_input: CanonicalPromptInput { parts: Vec::new() },
            model: config.model.clone(),
            max_output_tokens: Some(64),
            temperature: None,
            stream: true,
            think_level: crate::config::ThinkLevel::High,
            thinking: Default::default(),
        };
        let correlation = ProviderCorrelation {
            session_id: "session".into(),
            agent_address: "main".into(),
            connection_name: "router".into(),
            workspace: std::path::PathBuf::from("."),
        };
        let first = prepare_provider_request(&config, &request, Some(&correlation)).unwrap();
        let repeated = prepare_provider_request(&config, &request, Some(&correlation)).unwrap();
        assert_eq!(first, repeated);
        assert_eq!("low", first.body["reasoning"]["effort"]);
        assert_eq!(true, first.body["provider"]["require_parameters"]);
        assert_eq!(
            first.correlation_id.as_deref(),
            first.body["session_id"].as_str()
        );
        request.assembly.system_prompt = "later system".into();
        assert_eq!("fixed system", first.body["messages"][0]["content"]);
        let other = prepare_provider_request(
            &config,
            &request,
            Some(&ProviderCorrelation {
                agent_address: "reviewer".into(),
                ..correlation
            }),
        )
        .unwrap();
        assert_ne!(first.correlation_id, other.correlation_id);
        let encoded = serde_json::to_string(&first).unwrap();
        assert!(!encoded.contains("api_key"));
        assert_eq!(first, serde_json::from_str(&encoded).unwrap());
    }

    fn response_cache_key(
        config: &ResolvedProvider,
        correlation: Option<&ProviderCorrelation>,
    ) -> Option<String> {
        let catalog = crate::prompt::builtin_prompt_catalog();
        let request = ProviderRequest {
            assembly: assembly(),
            catalog: &catalog,
            current_input: CanonicalPromptInput { parts: Vec::new() },
            model: config.model.clone(),
            max_output_tokens: Some(64),
            temperature: None,
            stream: true,
            think_level: crate::config::ThinkLevel::Off,
            thinking: Default::default(),
        };
        prepare_provider_request(config, &request, correlation)
            .unwrap()
            .body
            .get("prompt_cache_key")
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    fn response_correlation(
        workspace: &std::path::Path,
        session_id: &str,
        agent_address: &str,
    ) -> ProviderCorrelation {
        ProviderCorrelation {
            session_id: session_id.into(),
            agent_address: agent_address.into(),
            connection_name: "openai".into(),
            workspace: workspace.into(),
        }
    }

    #[test]
    fn workspace_cache_key_is_stable_across_sessions_and_agents_but_scoped_to_route() {
        let workspace = tempfile::tempdir().unwrap();
        let other_workspace = tempfile::tempdir().unwrap();
        let config = ResolvedProvider {
            kind: ProviderKind::OpenAiResponses,
            connection_name: "openai".into(),
            model: "gpt-test".into(),
            base_url: "https://api.openai.com/v1".into(),
            ..Default::default()
        };
        let main = response_correlation(workspace.path(), "session-one", "main");
        let dotted_workspace = workspace.path().join(".");
        let explore = response_correlation(&dotted_workspace, "session-two", "explore");
        let first = response_cache_key(&config, Some(&main)).unwrap();
        assert_eq!(64, first.len());
        assert_eq!(first, response_cache_key(&config, Some(&explore)).unwrap());
        let raw_workspace = workspace.path().to_string_lossy();
        assert!(!first.contains(raw_workspace.as_ref()));

        let other_workspace = response_correlation(other_workspace.path(), "session-one", "main");
        assert_ne!(
            first,
            response_cache_key(&config, Some(&other_workspace)).unwrap()
        );

        let mut other_route = config.clone();
        other_route.connection_name = "gateway".into();
        other_route.base_url = "https://gateway.example/v1".into();
        other_route.model = "routed-model".into();
        let other_route_correlation = ProviderCorrelation {
            connection_name: "gateway".into(),
            ..main.clone()
        };
        assert_ne!(
            first,
            response_cache_key(&other_route, Some(&other_route_correlation)).unwrap()
        );
    }

    #[test]
    fn session_cache_key_is_per_session_and_agent_and_off_omits_it() {
        let workspace = tempfile::tempdir().unwrap();
        let main = response_correlation(workspace.path(), "session-one", "main");
        let config = ResolvedProvider {
            kind: ProviderKind::OpenAiResponses,
            connection_name: "openai".into(),
            model: "gpt-test".into(),
            base_url: "https://api.openai.com/v1".into(),
            options: crate::config::ProviderOptions {
                prompt_cache_scope: PromptCacheScope::Session,
                ..Default::default()
            },
            ..Default::default()
        };
        let first = response_cache_key(&config, Some(&main)).unwrap();
        assert_ne!(
            first,
            response_cache_key(
                &config,
                Some(&response_correlation(
                    workspace.path(),
                    "session-two",
                    "main"
                ))
            )
            .unwrap()
        );
        assert_ne!(
            first,
            response_cache_key(
                &config,
                Some(&response_correlation(
                    workspace.path(),
                    "session-one",
                    "explore"
                ))
            )
            .unwrap()
        );
        let other_workspace = tempfile::tempdir().unwrap();
        assert_eq!(
            first,
            response_cache_key(
                &config,
                Some(&response_correlation(
                    other_workspace.path(),
                    "session-one",
                    "main"
                ))
            )
            .unwrap()
        );
        assert_eq!(first, response_cache_key(&config, Some(&main)).unwrap());

        let mut off = config;
        off.options.prompt_cache_scope = PromptCacheScope::Off;
        assert_eq!(None, response_cache_key(&off, Some(&main)));
        assert_eq!(None, response_cache_key(&off, None));

        let missing_path = workspace.path().join("missing");
        let missing_workspace = response_correlation(&missing_path, "session", "main");
        let mut workspace_scope = off;
        workspace_scope.options.prompt_cache_scope = PromptCacheScope::Workspace;
        assert_eq!(
            None,
            response_cache_key(&workspace_scope, Some(&missing_workspace))
        );
    }

    #[test]
    fn prompt_cache_key_is_responses_only_and_requires_correlation() {
        let workspace = tempfile::tempdir().unwrap();
        let correlation = response_correlation(workspace.path(), "session", "main");
        let mut config = ResolvedProvider {
            kind: ProviderKind::OpenAiCompatible,
            connection_name: "openai".into(),
            model: "gpt-test".into(),
            base_url: "https://api.openai.com/v1".into(),
            ..Default::default()
        };
        assert_eq!(None, response_cache_key(&config, Some(&correlation)));
        config.kind = ProviderKind::Anthropic;
        assert_eq!(None, response_cache_key(&config, Some(&correlation)));
        config.kind = ProviderKind::OpenAiResponses;
        assert_eq!(None, response_cache_key(&config, None));
    }

    #[test]
    fn deepseek_disabled_thinking_and_stateless_responses_are_explicit() {
        let catalog = crate::prompt::builtin_prompt_catalog();
        let mut config = ResolvedProvider {
            model: "deepseek-flash".into(),
            ..Default::default()
        };
        config.capabilities.profile = ProviderProfile::DeepSeek;
        config.settings.reasoning_effort = Some("none".into());
        let request = ProviderRequest {
            assembly: assembly(),
            catalog: &catalog,
            current_input: CanonicalPromptInput { parts: Vec::new() },
            model: config.model.clone(),
            max_output_tokens: Some(64),
            temperature: None,
            stream: true,
            think_level: crate::config::ThinkLevel::Off,
            thinking: Default::default(),
        };
        let chat = prepare_provider_request(&config, &request, None).unwrap();
        assert_eq!("disabled", chat.body["thinking"]["type"]);
        assert!(chat.body.get("reasoning_effort").is_none());
        config.kind = ProviderKind::OpenAiResponses;
        let responses = prepare_provider_request(&config, &request, None).unwrap();
        assert_eq!(false, responses.body["store"]);
        assert_eq!("none", responses.body["reasoning"]["effort"]);
        assert!(responses.body.get("previous_response_id").is_none());
    }

    #[test]
    fn openai_and_routed_responses_preserve_optional_function_parameters() {
        let catalog = crate::prompt::builtin_prompt_catalog();
        let schema = json!({
            "type": "object",
            "properties": {"task": {"type": "string"}, "connection": {"type": "string"}},
            "required": ["task"],
        });
        let mut assembly = assembly();
        assembly.tools.push(ToolSchema {
            name: "create_agent".into(),
            description: "Start an Agent".into(),
            input_schema: schema.clone(),
        });
        let request = ProviderRequest {
            assembly,
            catalog: &catalog,
            current_input: CanonicalPromptInput { parts: Vec::new() },
            model: "fixture".into(),
            max_output_tokens: Some(64),
            temperature: None,
            stream: true,
            think_level: crate::config::ThinkLevel::Off,
            thinking: Default::default(),
        };
        let mut config = ResolvedProvider {
            kind: ProviderKind::OpenAiResponses,
            model: request.model.clone(),
            ..Default::default()
        };
        for (profile, expected_strict) in [
            (ProviderProfile::OpenAi, Some(false)),
            (ProviderProfile::OpenRouter, Some(false)),
            (ProviderProfile::DeepSeek, None),
            (ProviderProfile::Compatible, None),
        ] {
            config.capabilities.profile = profile;
            let prepared = prepare_provider_request(&config, &request, None).unwrap();
            let tool = &prepared.body["tools"][0];
            assert_eq!("function", tool["type"]);
            assert_eq!(expected_strict, tool["strict"].as_bool());
            assert_eq!(schema, tool["parameters"]);
        }
        config.kind = ProviderKind::OpenAiCompatible;
        config.capabilities.profile = ProviderProfile::OpenAi;
        let chat = prepare_provider_request(&config, &request, None).unwrap();
        assert!(chat.body["tools"][0]["function"].get("strict").is_none());
    }
}
