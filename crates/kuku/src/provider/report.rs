use serde_json::{Map, Value};

use crate::config::ProviderFormat;

use super::chunk::ProviderChunk;
use super::types::{ProviderFailure, ProviderReport, ProviderUsage};

#[derive(Default)]
pub(crate) struct ReportAccumulator {
    report: ProviderReport,
}

impl ReportAccumulator {
    pub(crate) fn observe(
        &mut self,
        response: &Value,
    ) -> Result<Option<ProviderReport>, ProviderFailure> {
        let previous = self.report.clone();
        update_string(&mut self.report.model, response.get("model"));
        update_string(&mut self.report.provider, response.get("provider"));
        update_string(&mut self.report.service_tier, response.get("service_tier"));
        if let Some(routing) = response.get("openrouter_metadata") {
            if let Some(is_byok) = routing.get("is_byok").and_then(Value::as_bool) {
                self.report.is_byok = Some(is_byok);
            }
            if let Some(endpoints) = routing
                .pointer("/endpoints/available")
                .and_then(Value::as_array)
            {
                let mut selected = endpoints
                    .iter()
                    .filter(|endpoint| endpoint["selected"] == true);
                if let Some(endpoint) = selected.next().filter(|_| selected.next().is_none()) {
                    update_string(&mut self.report.provider, endpoint.get("provider"));
                }
            }
        }
        update_string(
            &mut self.report.reasoning_effort,
            response
                .get("reasoning_effort")
                .or_else(|| {
                    response
                        .get("reasoning")
                        .and_then(|value| value.get("effort"))
                })
                .or_else(|| {
                    response
                        .get("output_config")
                        .and_then(|value| value.get("effort"))
                }),
        );
        if let Some(usage) = response.get("usage").and_then(Value::as_object) {
            let existing = self
                .report
                .raw_usage
                .get_or_insert_with(|| Value::Object(Map::new()));
            merge_usage(existing, &Value::Object(usage.clone()));
            super::replay::validate_replay_size(existing)?;
            update_string(&mut self.report.service_tier, usage.get("service_tier"));
            if let Some(cost) = usage
                .get("cost_usd")
                .and_then(Value::as_f64)
                .or_else(|| usage.get("cost").and_then(Value::as_f64))
                .filter(|cost| cost.is_finite() && *cost >= 0.0)
            {
                self.report.cost_usd = Some(cost);
            }
        }
        Ok((self.report != previous).then(|| self.report.clone()))
    }

    pub(crate) fn usage(
        &self,
        protocol: ProviderFormat,
        output_reported: bool,
    ) -> Option<ProviderChunk> {
        let raw = self.report.raw_usage.as_ref()?;
        let usage = normalize_usage(protocol, raw);
        Some(ProviderChunk::StreamUsage {
            input_tokens: usage.input_tokens,
            output_tokens: output_reported.then_some(usage.output_tokens).flatten(),
            cache_read_input_tokens: usage.cache_read_input_tokens,
            cache_creation_input_tokens: usage.cache_creation_input_tokens,
        })
    }
}

pub(super) fn normalize_usage(protocol: ProviderFormat, usage: &Value) -> ProviderUsage {
    let (input, output, read, write) = match protocol {
        ProviderFormat::OpenAiChat => {
            let cache_hit = usage.get("prompt_cache_hit_tokens").and_then(Value::as_u64);
            let prompt = usage.get("prompt_tokens").and_then(Value::as_u64);
            let deepseek_total =
                || cache_hit?.checked_add(usage.get("prompt_cache_miss_tokens")?.as_u64()?);
            let derived = prompt.is_none().then(deepseek_total).flatten();
            let read = if derived.is_some() {
                cache_hit
            } else {
                usage
                    .pointer("/prompt_tokens_details/cached_tokens")
                    .and_then(Value::as_u64)
                    .or(cache_hit)
            };
            (
                prompt.or(derived),
                usage.get("completion_tokens").and_then(Value::as_u64),
                read,
                usage
                    .pointer("/prompt_tokens_details/cache_write_tokens")
                    .and_then(Value::as_u64),
            )
        }
        ProviderFormat::OpenAiResponses => (
            usage.get("input_tokens").and_then(Value::as_u64),
            usage.get("output_tokens").and_then(Value::as_u64),
            usage
                .pointer("/input_tokens_details/cached_tokens")
                .and_then(Value::as_u64),
            usage
                .pointer("/input_tokens_details/cache_write_tokens")
                .and_then(Value::as_u64),
        ),
        ProviderFormat::Anthropic => (
            usage.get("input_tokens").and_then(Value::as_u64),
            usage.get("output_tokens").and_then(Value::as_u64),
            usage.get("cache_read_input_tokens").and_then(Value::as_u64),
            usage
                .get("cache_creation_input_tokens")
                .and_then(Value::as_u64),
        ),
    };
    ProviderUsage {
        input_tokens: if protocol == ProviderFormat::Anthropic {
            input
                .and_then(|input| input.checked_add(read.unwrap_or(0)))
                .and_then(|input| input.checked_add(write.unwrap_or(0)))
        } else {
            input
        },
        output_tokens: output,
        cache_read_input_tokens: read,
        cache_creation_input_tokens: write,
    }
}

fn update_string(target: &mut Option<String>, value: Option<&Value>) {
    if let Some(value) = value.and_then(Value::as_str) {
        *target = Some(value.into());
    }
}

fn merge_usage(target: &mut Value, update: &Value) {
    match (target, update) {
        (Value::Object(target), Value::Object(update)) => {
            for (key, value) in update {
                if !value.is_null() {
                    merge_usage(target.entry(key.clone()).or_insert(Value::Null), value);
                }
            }
        }
        (target, update) => *target = update.clone(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{ProviderFormat, ReportAccumulator, normalize_usage};

    #[test]
    fn reads_the_selected_openrouter_provider_from_responses_metadata() {
        let mut reports = ReportAccumulator::default();
        let report = reports
            .observe(&json!({
                "model": "openai/gpt-6-luna",
                "service_tier": "default",
                "openrouter_metadata": {
                    "is_byok": false,
                    "endpoints": { "available": [
                        { "provider": "OpenAI", "selected": false },
                        { "provider": "Azure", "selected": true }
                    ]}
                },
                "usage": { "input_tokens": 100, "output_tokens": 10 }
            }))
            .unwrap()
            .unwrap();
        assert_eq!(Some("Azure"), report.provider.as_deref());
        assert_eq!(Some("default"), report.service_tier.as_deref());
        assert_eq!(Some(false), report.is_byok);
    }

    #[test]
    fn total_input_has_the_same_meaning_in_all_protocols() {
        for (protocol, value) in [
            (
                ProviderFormat::OpenAiChat,
                json!({"prompt_tokens":100,"prompt_tokens_details":{"cached_tokens":60,"cache_write_tokens":10}}),
            ),
            (
                ProviderFormat::OpenAiResponses,
                json!({"input_tokens":100,"input_tokens_details":{"cached_tokens":60,"cache_write_tokens":10}}),
            ),
            (
                ProviderFormat::Anthropic,
                json!({"input_tokens":30,"cache_read_input_tokens":60,"cache_creation_input_tokens":10}),
            ),
        ] {
            let usage = normalize_usage(protocol, &value);
            assert_eq!(Some(100), usage.input_tokens);
            assert_eq!(Some(60), usage.cache_read_input_tokens);
            assert_eq!(Some(10), usage.cache_creation_input_tokens);
            assert_eq!(Some((60, 100)), usage.cache_input_counts());
            assert_eq!(None, usage.output_tokens);
        }
    }

    #[test]
    fn deepseek_chat_can_reconstruct_the_total_from_explicit_hit_and_miss_counts() {
        let protocol = ProviderFormat::OpenAiChat;
        let both = normalize_usage(
            protocol,
            &json!({"prompt_cache_hit_tokens":60,"prompt_cache_miss_tokens":40}),
        );
        assert_eq!(Some((60, 100)), both.cache_input_counts());
        let hit_only = normalize_usage(protocol, &json!({"prompt_cache_hit_tokens":60}));
        assert_eq!(Some(60), hit_only.cache_read_input_tokens);
        assert_eq!(None, hit_only.cache_input_counts());
        let empty_cache = normalize_usage(
            protocol,
            &json!({"prompt_tokens":100,"prompt_tokens_details":{"cached_tokens":0}}),
        );
        assert_eq!(Some((0, 100)), empty_cache.cache_input_counts());
    }

    #[test]
    fn messages_initial_and_terminal_usage_merge_without_losing_unknowns() {
        let mut reports = ReportAccumulator::default();
        reports.observe(&json!({"model":"deepseek-flash","usage":{"input_tokens":30,"cache_read_input_tokens":60,"output_tokens":0}})).unwrap();
        let initial = reports.usage(ProviderFormat::Anthropic, false).unwrap();
        let final_report = reports.observe(&json!({"usage":{"output_tokens":12,"cost":0.0001,"output_tokens_details":{"thinking_tokens":2}}})).unwrap().unwrap();
        assert_eq!(Some(0.0001), final_report.cost_usd);
        assert_eq!(None, final_report.provider);
        assert_eq!(None, final_report.service_tier);
        assert_eq!(30, final_report.raw_usage.as_ref().unwrap()["input_tokens"]);
        assert_eq!(
            2,
            final_report.raw_usage.as_ref().unwrap()["output_tokens_details"]["thinking_tokens"]
        );
        let terminal = reports.usage(ProviderFormat::Anthropic, true).unwrap();
        assert!(matches!(
            initial,
            super::ProviderChunk::StreamUsage {
                input_tokens: Some(90),
                output_tokens: None,
                cache_read_input_tokens: Some(60),
                ..
            }
        ));
        assert!(matches!(
            terminal,
            super::ProviderChunk::StreamUsage {
                input_tokens: Some(90),
                output_tokens: Some(12),
                cache_read_input_tokens: Some(60),
                ..
            }
        ));
        assert!(reports.observe(&json!({"usage":null})).unwrap().is_none());
    }
}
