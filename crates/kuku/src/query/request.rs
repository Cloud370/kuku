use crate::context::{ContextAssembly, MessageBlock, Role};
use crate::prompt::{PromptAsset, PromptCatalog};
use crate::provider::types::{CanonicalPromptInput, ProviderRequest};

#[derive(Debug, Clone)]
pub(super) struct OwnedRequestBase {
    assembly: ContextAssembly,
    catalog: PromptCatalog,
    current_input: CanonicalPromptInput,
    model: String,
    max_output_tokens: u32,
    temperature: Option<f32>,
    think_level: crate::config::ThinkLevel,
    thinking: crate::config::ResolvedThinking,
    current_user_index: usize,
    recovery: PromptAsset,
}

impl OwnedRequestBase {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        assembly: ContextAssembly,
        catalog: PromptCatalog,
        current_input: CanonicalPromptInput,
        model: String,
        max_output_tokens: u32,
        temperature: Option<f32>,
        think_level: crate::config::ThinkLevel,
        thinking: crate::config::ResolvedThinking,
        current_user_index: usize,
    ) -> crate::error::Result<Self> {
        let recovery = catalog.runtime.get("recovery").cloned().ok_or_else(|| {
            crate::error::Error::PromptRender("missing runtime/recovery template".to_string())
        })?;
        let history_message = assembly.history.get(current_user_index).ok_or_else(|| {
            crate::error::Error::InvalidEventStream(
                "frozen request has no current history message".to_string(),
            )
        })?;
        let input_message = current_input.parts.last().ok_or_else(|| {
            crate::error::Error::InvalidEventStream(
                "frozen request has no current input message".to_string(),
            )
        })?;
        if history_message != input_message {
            return Err(crate::error::Error::InvalidEventStream(
                "frozen request current input does not match history".to_string(),
            ));
        }
        Ok(Self {
            assembly,
            catalog,
            current_input,
            model,
            max_output_tokens,
            temperature,
            think_level,
            thinking,
            current_user_index,
            recovery,
        })
    }

    pub(super) fn snapshot_assembly(&self) -> &ContextAssembly {
        &self.assembly
    }

    pub(super) fn snapshot_parameters(&self) -> crate::event::ExactRequestParameters {
        crate::event::ExactRequestParameters {
            model: self.model.clone(),
            max_output_tokens: Some(self.max_output_tokens as u64),
            temperature: self
                .temperature
                .and_then(|value| crate::event::Temperature::try_new(value).ok()),
            stream: true,
            thinking: match self.think_level {
                crate::config::ThinkLevel::Off => crate::event::ThinkingConfig::Disabled,
                _ => crate::event::ThinkingConfig::Enabled {
                    budget_tokens: None,
                },
            },
        }
    }

    pub(super) fn snapshot_handoff_template(&self) -> Option<&str> {
        self.catalog
            .runtime
            .get("handoff-context")
            .map(|asset| asset.text.as_str())
    }

    pub(super) fn request(&self) -> ProviderRequest<'_> {
        ProviderRequest {
            assembly: self.assembly.clone(),
            catalog: &self.catalog,
            current_input: self.current_input.clone(),
            model: self.model.clone(),
            max_output_tokens: Some(self.max_output_tokens),
            temperature: self.temperature,
            stream: true,
            think_level: self.think_level,
            thinking: self.thinking.clone(),
        }
    }

    pub(super) fn recovery_request(&self) -> crate::error::Result<RecoveredRequest> {
        let mut assembly = self.assembly.clone();
        let message = assembly
            .history
            .get_mut(self.current_user_index)
            .ok_or_else(|| {
                crate::error::Error::InvalidEventStream(
                    "frozen request no longer has its current user message".to_string(),
                )
            })?;
        if message.role != Role::User {
            return Err(crate::error::Error::InvalidEventStream(
                "frozen request current message is not user input".to_string(),
            ));
        }
        message
            .blocks
            .push(MessageBlock::Text(self.recovery.text.clone()));

        let mut current_input = self.current_input.clone();
        let input = current_input.parts.last_mut().ok_or_else(|| {
            crate::error::Error::InvalidEventStream(
                "frozen request has no current input message".to_string(),
            )
        })?;
        if input.role != Role::User {
            return Err(crate::error::Error::InvalidEventStream(
                "frozen request input is not user input".to_string(),
            ));
        }
        input
            .blocks
            .push(MessageBlock::Text(self.recovery.text.clone()));

        Ok(RecoveredRequest {
            assembly,
            catalog: self.catalog.clone(),
            current_input,
            model: self.model.clone(),
            max_output_tokens: self.max_output_tokens,
            temperature: self.temperature,
            think_level: self.think_level,
            thinking: self.thinking.clone(),
        })
    }

    pub(super) fn recovery_asset(&self) -> &PromptAsset {
        &self.recovery
    }

    pub(super) fn max_output_tokens(&self) -> u32 {
        self.max_output_tokens
    }

    pub(super) fn context_limit(&self, resolved: &crate::provider::types::ResolvedProvider) -> u32 {
        resolved
            .max_context_tokens
            .saturating_sub(self.think_level.overhead_tokens())
    }
}

#[derive(Debug, Clone)]
pub(super) struct RecoveredRequest {
    assembly: ContextAssembly,
    catalog: PromptCatalog,
    current_input: CanonicalPromptInput,
    model: String,
    max_output_tokens: u32,
    temperature: Option<f32>,
    think_level: crate::config::ThinkLevel,
    thinking: crate::config::ResolvedThinking,
}

impl RecoveredRequest {
    pub(super) fn request(&self) -> ProviderRequest<'_> {
        ProviderRequest {
            assembly: self.assembly.clone(),
            catalog: &self.catalog,
            current_input: self.current_input.clone(),
            model: self.model.clone(),
            max_output_tokens: Some(self.max_output_tokens),
            temperature: self.temperature,
            stream: true,
            think_level: self.think_level,
            thinking: self.thinking.clone(),
        }
    }

    pub(super) fn snapshot_assembly(&self) -> &ContextAssembly {
        &self.assembly
    }

    pub(super) fn snapshot_parameters(&self) -> crate::event::ExactRequestParameters {
        crate::event::ExactRequestParameters {
            model: self.model.clone(),
            max_output_tokens: Some(self.max_output_tokens as u64),
            temperature: self
                .temperature
                .and_then(|value| crate::event::Temperature::try_new(value).ok()),
            stream: true,
            thinking: match self.think_level {
                crate::config::ThinkLevel::Off => crate::event::ThinkingConfig::Disabled,
                _ => crate::event::ThinkingConfig::Enabled {
                    budget_tokens: None,
                },
            },
        }
    }

    pub(super) fn snapshot_handoff_template(&self) -> Option<&str> {
        self.catalog
            .runtime
            .get("handoff-context")
            .map(|asset| asset.text.as_str())
    }

    pub(super) fn catalog(&self) -> &PromptCatalog {
        &self.catalog
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::CanonicalMessage;
    use crate::provider::types::{ProviderKind, ResolvedProvider, SecretString};

    fn assembly(current: CanonicalMessage) -> ContextAssembly {
        ContextAssembly {
            system_prompt: "system".to_string(),
            prelude_messages: Vec::new(),
            history: vec![current],
            tools: Vec::new(),
            prompt_asset_sources: Vec::new(),
            project_instruction_sources: Vec::new(),
            memory_sources: Vec::new(),
            runtime_context: None,
            handoff_summary: None,
        }
    }

    fn request_base(
        history_message: CanonicalMessage,
        input_message: CanonicalMessage,
        think_level: crate::config::ThinkLevel,
    ) -> crate::error::Result<OwnedRequestBase> {
        OwnedRequestBase::new(
            assembly(history_message),
            crate::prompt::builtin_prompt_catalog(),
            CanonicalPromptInput {
                parts: vec![input_message],
            },
            "test-model".to_string(),
            1000,
            None,
            think_level,
            crate::config::ResolvedThinking::default(),
            0,
        )
    }

    #[test]
    fn effective_context_limit_reserves_thinking_overhead() {
        let current = CanonicalMessage::user_text("task");
        let base =
            request_base(current.clone(), current, crate::config::ThinkLevel::Medium).unwrap();
        let resolved = ResolvedProvider {
            kind: ProviderKind::OpenAiCompatible,
            model: "test-model".to_string(),
            base_url: "https://example.test".to_string(),
            api_key: SecretString::new("test-key"),
            max_context_tokens: 10_000,
            max_output_tokens: 1000,
            think_level: crate::config::ThinkLevel::Medium,
            thinking: crate::config::ResolvedThinking::default(),
        };

        assert_eq!(5904, base.context_limit(&resolved));
    }

    #[test]
    fn frozen_request_rejects_divergent_current_input() {
        let result = request_base(
            CanonicalMessage::user_text("task with hook context"),
            CanonicalMessage::user_text("task"),
            crate::config::ThinkLevel::Off,
        );

        assert!(matches!(
            result,
            Err(crate::error::Error::InvalidEventStream(message))
                if message.contains("current input does not match")
        ));
    }
}
