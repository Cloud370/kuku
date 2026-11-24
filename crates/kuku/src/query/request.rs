use serde_json::Value;

use crate::context::{CanonicalMessage, ContextAssembly, MessageBlock, Role};
use crate::prompt::{PromptAsset, PromptCatalog};
use crate::provider::types::{CanonicalPromptInput, ProviderRequest};

#[derive(Debug, Clone)]
pub(super) struct OwnedRequestBase {
    assembly: ContextAssembly,
    catalog: PromptCatalog,
    current_input: CanonicalPromptInput,
    model: String,
    max_output_tokens: Option<u32>,
    temperature: Option<f32>,
    think_level: crate::config::ThinkLevel,
    thinking: crate::config::ResolvedThinking,
    current_user_index: usize,
    recovery: PromptAsset,
    recovery_notice: String,
}

impl OwnedRequestBase {
    #[allow(
        clippy::too_many_arguments,
        reason = "request construction keeps the resolved assembly, identity, and provider inputs explicit"
    )]
    pub(super) fn new(
        assembly: ContextAssembly,
        catalog: PromptCatalog,
        current_input: CanonicalPromptInput,
        model: String,
        max_output_tokens: Option<u32>,
        temperature: Option<f32>,
        think_level: crate::config::ThinkLevel,
        thinking: crate::config::ResolvedThinking,
        current_user_index: usize,
    ) -> crate::error::Result<Self> {
        let recovery = catalog.runtime.get("recovery").cloned().ok_or_else(|| {
            crate::error::Error::PromptRender("missing runtime/recovery template".to_string())
        })?;
        let tools: Vec<_> = assembly
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        let recovery_notice = catalog.render(
            "runtime/recovery.md",
            minijinja::context! {
                step => minijinja::context! { stop_reason => "length" },
                tools => minijinja::value::Serde(&tools),
            },
        )?;
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
            recovery_notice,
        })
    }

    pub(super) fn request(&self) -> ProviderRequest<'_> {
        ProviderRequest {
            assembly: self.assembly.clone(),
            catalog: &self.catalog,
            current_input: self.current_input.clone(),
            model: self.model.clone(),
            max_output_tokens: self.max_output_tokens,
            temperature: self.temperature,
            stream: true,
            think_level: self.think_level,
            thinking: self.thinking.clone(),
        }
    }

    pub(super) fn replace_checkpoint_history(
        &mut self,
        history: Vec<crate::context::CanonicalMessage>,
        current_user_index: usize,
    ) {
        self.assembly.history = history;
        self.current_user_index = current_user_index;
        self.current_input.parts = vec![self.assembly.history[current_user_index].clone()];
    }

    pub(super) fn checkpoint_request(
        &self,
        mut history: Vec<crate::context::CanonicalMessage>,
        instruction: String,
        reuse_prefix: bool,
    ) -> Self {
        let mut request = self.clone();
        if !reuse_prefix {
            request.assembly.tools.clear();
            request.assembly.prelude_messages.clear();
            request.assembly.system_prompt.clear();
        }
        request.current_user_index = history.len();
        let instruction = crate::context::CanonicalMessage::user_text(instruction);
        history.push(instruction.clone());
        request.assembly.history = history;
        request.current_input.parts = vec![instruction];
        request
    }

    pub(super) fn estimated_input_tokens(&self) -> u32 {
        let prelude = estimate_text(&self.assembly.system_prompt).saturating_add(
            estimated_message_tokens(self.assembly.prelude_messages.iter()),
        );
        let tools = self.assembly.tools.iter().fold(0u32, |total, tool| {
            total
                .saturating_add(estimate_text(&tool.name))
                .saturating_add(estimate_text(&tool.description))
                .saturating_add(estimate_json(&tool.input_schema))
        });
        prelude
            .saturating_add(tools)
            .saturating_add(estimated_message_tokens(self.assembly.history.iter()))
    }

    pub(super) fn limit_checkpoint_output(&mut self, available: u32) {
        self.max_output_tokens = self.max_output_tokens.map(|limit| limit.min(available));
    }

    pub(super) fn checkpoint_instruction(&self) -> crate::Result<Option<String>> {
        self.catalog
            .runtime
            .get("compaction-instruction")
            .map(|_| {
                self.catalog
                    .render("runtime/compaction-instruction.md", minijinja::context! {})
            })
            .transpose()
    }

    pub(super) fn render_checkpoint(&self, checkpoint: &str) -> crate::Result<String> {
        self.catalog.render(
            "runtime/compaction-context.md",
            minijinja::context! { checkpoint },
        )
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
            .push(MessageBlock::Text(self.recovery_notice.clone()));

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
            .push(MessageBlock::Text(self.recovery_notice.clone()));

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

    pub(super) fn recovery_notice(&self) -> &str {
        &self.recovery_notice
    }

    pub(super) fn recovery_asset(&self) -> &PromptAsset {
        &self.recovery
    }

    pub(super) fn max_output_tokens(&self) -> Option<u32> {
        self.max_output_tokens
    }

    pub(super) fn context_limit(&self, resolved: &crate::provider::types::ResolvedProvider) -> u32 {
        resolved
            .max_context_tokens
            .saturating_sub(self.think_level.overhead_tokens())
    }

    pub(super) fn context_headroom(
        &self,
        resolved: &crate::provider::types::ResolvedProvider,
    ) -> crate::notice::ContextHeadroom {
        crate::notice::compute_context_headroom(
            self.context_limit(resolved),
            self.max_output_tokens,
            None,
        )
    }
}

pub(super) fn estimated_message_tokens<'a>(
    messages: impl Iterator<Item = &'a CanonicalMessage>,
) -> u32 {
    messages.fold(0u32, |total, message| {
        message
            .blocks
            .iter()
            .fold(total.saturating_add(4), |tokens, block| {
                let addition = match block {
                    MessageBlock::Text(text) | MessageBlock::Thinking(text) => estimate_text(text),
                    MessageBlock::ToolUse(call) => estimate_text(&call.name)
                        .saturating_add(estimate_json(&call.args))
                        .saturating_add(estimate_text(&call.id)),
                    MessageBlock::ToolResult(result) => estimate_text(&result.model_content)
                        .saturating_add(estimate_text(&result.tool_call_id)),
                    MessageBlock::ProviderReplay { value, .. } => estimate_json(value),
                    MessageBlock::InputImage(_) => 1_200,
                };
                tokens.saturating_add(addition)
            })
    })
}

fn estimate_text(text: &str) -> u32 {
    if text.is_ascii() {
        return u32::try_from(text.len().div_ceil(4)).unwrap_or(u32::MAX);
    }
    let (ascii, other) = text.chars().fold((0u32, 0u32), |(ascii, other), ch| {
        if ch.is_ascii() {
            (ascii.saturating_add(1), other)
        } else {
            (ascii, other.saturating_add(1))
        }
    });
    ascii.div_ceil(4).saturating_add(other)
}

fn estimate_json(value: &Value) -> u32 {
    match value {
        Value::String(text) => estimate_text(text).saturating_add(2),
        Value::Array(items) => items.iter().fold(2u32, |total, value| {
            total.saturating_add(estimate_json(value)).saturating_add(1)
        }),
        Value::Object(fields) => fields.iter().fold(2u32, |total, (key, value)| {
            total
                .saturating_add(estimate_text(key))
                .saturating_add(estimate_json(value))
                .saturating_add(3)
        }),
        _ => 4,
    }
}

#[derive(Debug, Clone)]
pub(super) struct RecoveredRequest {
    assembly: ContextAssembly,
    catalog: PromptCatalog,
    current_input: CanonicalPromptInput,
    model: String,
    max_output_tokens: Option<u32>,
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
            max_output_tokens: self.max_output_tokens,
            temperature: self.temperature,
            stream: true,
            think_level: self.think_level,
            thinking: self.thinking.clone(),
        }
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
            Some(1000),
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
            max_output_tokens: Some(1000),
            think_level: crate::config::ThinkLevel::Medium,
            thinking: crate::config::ResolvedThinking::default(),
            ..Default::default()
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
