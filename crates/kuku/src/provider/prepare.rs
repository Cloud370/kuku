use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::config::{ProviderAuth, ProviderFormat, ProviderProfile};
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
    validate_replay(config, request)?;
    let (url, mut body) = match config.kind {
        ProviderKind::Anthropic => (
            super::anthropic::messages_url(&config.base_url),
            super::anthropic::render_body(request),
        ),
        ProviderKind::OpenAiCompatible => (
            super::openai_compat::chat_completions_url(&config.base_url),
            super::openai_compat::render_body(request),
        ),
        ProviderKind::OpenAiResponses => (
            super::openai_responses::responses_url(&config.base_url),
            super::openai_responses::render_body(request),
        ),
    };
    body["stream"] = json!(true);
    apply_settings(config, &mut body);
    let correlation_id = correlation.map(correlation_id);
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

pub(crate) fn authentication(config: &ResolvedProvider) -> ProviderAuth {
    config.options.auth.unwrap_or_else(|| {
        if config.capabilities.profile == ProviderProfile::OpenRouter {
            ProviderAuth::Bearer
        } else if config.kind == ProviderKind::Anthropic {
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

fn apply_settings(config: &ResolvedProvider, body: &mut Value) {
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
        if config.capabilities.profile == ProviderProfile::OpenAi
            && config.model.starts_with("gpt-5")
        {
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
        };
        let first = prepare_provider_request(&config, &request, Some(&correlation)).unwrap();
        let repeated = prepare_provider_request(&config, &request, Some(&correlation)).unwrap();
        assert_eq!(first, repeated);
        assert_eq!("low", first.body["reasoning"]["effort"]);
        assert_eq!(true, first.body["provider"]["require_parameters"]);
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
