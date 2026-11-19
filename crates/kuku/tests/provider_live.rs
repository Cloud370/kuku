use kuku::event::{EventPayload, EventStore};
use kuku::session::session_events_path;
use kuku::{query, Provider};

fn require_live_tests() {
    assert_eq!(
        std::env::var("KUKU_LIVE_PROVIDER_TESTS").as_deref(),
        Ok("1"),
        "set KUKU_LIVE_PROVIDER_TESTS=1 to run live provider tests"
    );
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires KUKU_LIVE_PROVIDER_TESTS=1 and Anthropic provider env vars"]
async fn anthropic_live_smoke_reads_a_real_file_via_tool_loop() {
    require_live_tests();

    let previous_home = std::env::var_os("KUKU_HOME");
    let previous_cwd = std::env::current_dir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    std::env::set_var("KUKU_HOME", home.path());
    std::env::set_current_dir(workspace.path()).unwrap();

    std::fs::write(workspace.path().join("AGENTS.md"), "version one\n").unwrap();
    std::fs::write(
        workspace.path().join("README.md"),
        "# LIVE_PROMPT_CONTEXT_OK\n",
    )
    .unwrap();

    let session_id =
        query("Read README.md with tools and reply with exactly: LIVE_PROMPT_CONTEXT_OK")
            .provider(Provider::Anthropic)
            .model(std::env::var("KUKU_ANTHROPIC_MODEL").expect("KUKU_ANTHROPIC_MODEL required"))
            .base_url(
                std::env::var("KUKU_ANTHROPIC_BASE_URL").expect("KUKU_ANTHROPIC_BASE_URL required"),
            )
            .api_key(
                std::env::var("KUKU_ANTHROPIC_API_KEY").expect("KUKU_ANTHROPIC_API_KEY required"),
            )
            .max_output_tokens(256)
            .temperature(0.0)
            .run()
            .await
            .expect("live Anthropic call should succeed")
            .session_id;

    std::fs::write(workspace.path().join("AGENTS.md"), "version two\n").unwrap();

    let drift_output = query(
        "If you receive a <kuku_system_notice> about changed context, reply with exactly: DRIFT_NOTICED. Otherwise reply with exactly: NO_NOTICE.",
    )
    .session(&session_id)
    .provider(Provider::Anthropic)
    .model(std::env::var("KUKU_ANTHROPIC_MODEL").expect("KUKU_ANTHROPIC_MODEL required"))
    .base_url(
        std::env::var("KUKU_ANTHROPIC_BASE_URL").expect("KUKU_ANTHROPIC_BASE_URL required"),
    )
    .api_key(std::env::var("KUKU_ANTHROPIC_API_KEY").expect("KUKU_ANTHROPIC_API_KEY required"))
    .max_output_tokens(256)
    .temperature(0.0)
    .run()
    .await
    .expect("live Anthropic drift turn should succeed");

    let events = EventStore::replay(
        session_events_path(
            home.path(),
            &std::fs::canonicalize(workspace.path()).unwrap(),
            &session_id,
        )
        .unwrap(),
    )
    .unwrap();

    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ToolCall { ref tool, .. } if tool == "read_file"
    )));
    assert!(drift_output.text.contains("DRIFT_NOTICED"));

    std::env::set_current_dir(previous_cwd).unwrap();
    match previous_home {
        Some(value) => std::env::set_var("KUKU_HOME", value),
        None => std::env::remove_var("KUKU_HOME"),
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires KUKU_LIVE_PROVIDER_TESTS=1 and OpenAI-compatible provider env vars"]
async fn openai_compatible_live_smoke_returns_text() {
    require_live_tests();
    let output = query("Reply with exactly: ok")
        .provider(Provider::OpenAiCompatible)
        .model(std::env::var("KUKU_OPENAI_MODEL").expect("KUKU_OPENAI_MODEL required"))
        .base_url(std::env::var("KUKU_OPENAI_BASE_URL").expect("KUKU_OPENAI_BASE_URL required"))
        .api_key(std::env::var("KUKU_OPENAI_API_KEY").expect("KUKU_OPENAI_API_KEY required"))
        .max_output_tokens(128)
        .temperature(0.0)
        .run()
        .await
        .expect("live OpenAI-compatible call should succeed");

    assert!(!output.text.trim().is_empty());
    let usage = output.usage.expect("live response should include usage");
    assert!(usage.input_tokens.unwrap_or(0) > 0);
    assert!(usage.output_tokens.unwrap_or(0) > 0);
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires KUKU_LIVE_PROVIDER_TESTS=1 and OpenAI-compatible provider env vars"]
async fn openai_responses_live_smoke_returns_text_and_usage() {
    require_live_tests();
    let output = query("Reply with exactly: ok")
        .provider(Provider::OpenAiResponses)
        .model(std::env::var("KUKU_OPENAI_MODEL").expect("KUKU_OPENAI_MODEL required"))
        .base_url(std::env::var("KUKU_OPENAI_BASE_URL").expect("KUKU_OPENAI_BASE_URL required"))
        .api_key(std::env::var("KUKU_OPENAI_API_KEY").expect("KUKU_OPENAI_API_KEY required"))
        .max_output_tokens(128)
        .temperature(0.0)
        .run()
        .await
        .expect("live OpenAI Responses call should succeed");

    assert!(!output.text.trim().is_empty());
    let usage = output.usage.expect("live response should include usage");
    assert!(usage.input_tokens.unwrap_or(0) > 0);
    assert!(usage.output_tokens.unwrap_or(0) > 0);
}

#[cfg(test)]
mod session_live_evidence {
    use super::require_live_tests;
    use kuku::config::load_config;
    use kuku::event::EventPayload;
    use kuku::{AgentConfig, AgentStatus, Input, Observation, Session, SessionOptions};
    use serde_json::{json, Map, Value};
    use std::env;
    use std::error::Error;
    use std::fs;
    use std::path::Path;
    use std::time::{Duration, Instant};
    use time::format_description::well_known::Rfc3339;
    use time::OffsetDateTime;

    const MAX_REQUESTS: u64 = 2;
    const MAX_OUTPUT_TOKENS: u32 = 128;
    const MAX_CONTEXT_TOKENS: u32 = 8_192;
    const MAX_REQUEST_BYTES: usize = 65_536;
    const RUN_TIMEOUT: Duration = Duration::from_secs(90);

    struct Route {
        provider: &'static str,
        model: &'static str,
        base_url: &'static str,
        format: &'static str,
        profile: &'static str,
        upstream: &'static str,
    }

    fn route(name: &str) -> Option<Route> {
        match name {
            "deepseek" => Some(Route {
                provider: "DeepSeek",
                model: "deepseek-flash",
                base_url: "https://api.deepseek.com",
                format: "openai-chat",
                profile: "deepseek",
                upstream: "DeepSeek official",
            }),
            "openrouter" => Some(Route {
                provider: "OpenRouter",
                model: "anthropic/claude-sonnet-5.5",
                base_url: "https://openrouter.ai/api/v1",
                format: "openai-responses",
                profile: "openrouter",
                upstream: "anthropic",
            }),
            _ => None,
        }
    }

    fn config(route: &Route) -> String {
        let reasoning_efforts = if route.profile == "openrouter" {
            "reasoning_efforts = [\"low\"]\n"
        } else {
            ""
        };
        let model = route.model;
        let mut raw = format!(
            r#"default_model = "balanced"

[model.balanced]
provider = "live"
model = "{model}"
think = "low"
reasoning_effort = "low"
context_window = {MAX_CONTEXT_TOKENS}
max_output_tokens = {MAX_OUTPUT_TOKENS}

[model.strong]
provider = "live"
model = "{model}"
think = "low"
reasoning_effort = "low"
context_window = {MAX_CONTEXT_TOKENS}
max_output_tokens = {MAX_OUTPUT_TOKENS}

[model.light]
provider = "live"
model = "{model}"
think = "low"
reasoning_effort = "low"
context_window = {MAX_CONTEXT_TOKENS}
max_output_tokens = {MAX_OUTPUT_TOKENS}

[provider.live]
format = "{}"
profile = "{}"
base_url = "{}"
api_key = "$KUKU_LIVE_API_KEY"
{reasoning_efforts}

[discovery]
auto_discover = false

[handoff]
enabled = false

[plugin]
enabled = false
"#,
            route.format, route.profile, route.base_url
        );
        if route.profile == "openrouter" {
            raw.push_str(
                r#"
[provider.live.routing]
order = ["anthropic"]
only = ["anthropic"]
allow_fallbacks = false
require_parameters = true

[provider.live.routing.max_price]
prompt = 2.0
completion = 10.0
"#,
            );
        }
        raw
    }

    fn record_observation(
        observation: Observation,
        started: Instant,
        api_key: &str,
        retry_events: &mut Vec<Value>,
        prepared_steps: &mut u64,
        saw_model_response: &mut bool,
        lagged: &mut bool,
    ) {
        let event = match observation {
            Observation::Committed(event) => event,
            Observation::Lagged { .. } => {
                *lagged = true;
                return;
            }
            _ => return,
        };
        match event.payload {
            EventPayload::ModelResponse { .. } => *saw_model_response = true,
            EventPayload::Unknown(value) => match value.get("kind").and_then(Value::as_str) {
                Some("runtime.step_prepared") => *prepared_steps += 1,
                Some("runtime.model_retry") => {
                    let mut summary = Map::new();
                    for field in [
                        "agent",
                        "turn",
                        "request_id",
                        "phase",
                        "attempt",
                        "failed_attempt",
                        "max_attempts",
                        "will_retry",
                        "failed_kind",
                        "status",
                        "stop_reason",
                        "delay_ms",
                        "ts",
                    ] {
                        if let Some(value) = value.get(field) {
                            summary.insert(field.to_owned(), value.clone());
                        }
                    }
                    if let Some(message) = value.get("message").and_then(Value::as_str) {
                        let bounded: String = message.chars().take(256).collect();
                        summary.insert(
                            "message".into(),
                            Value::String(bounded.replace(api_key, "[REDACTED]")),
                        );
                    }
                    summary.insert("event_id".into(), json!(event.id));
                    summary.insert(
                        "observed_elapsed_ms".into(),
                        json!(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)),
                    );
                    retry_events.push(Value::Object(summary));
                }
                Some("model.response") => *saw_model_response = true,
                _ => {}
            },
            _ => {}
        }
    }

    fn wait_intervals(events: &[Value]) -> Vec<Value> {
        let mut intervals = Vec::new();
        for waiting in events.iter().filter(|event| event["phase"] == "waiting") {
            let started = events.iter().find(|event| {
                event["phase"] == "started"
                    && event["request_id"] == waiting["request_id"]
                    && event["attempt"] == waiting["attempt"]
            });
            if let Some(started) = started {
                let waiting_ms = waiting["observed_elapsed_ms"].as_u64().unwrap_or(0);
                let started_ms = started["observed_elapsed_ms"]
                    .as_u64()
                    .unwrap_or(waiting_ms);
                intervals.push(json!({
                    "attempt": waiting["attempt"],
                    "configured_delay_ms": waiting["delay_ms"],
                    "observed_wait_ms": started_ms.saturating_sub(waiting_ms),
                }));
            }
        }
        intervals
    }

    fn status_name(status: AgentStatus) -> &'static str {
        match status {
            AgentStatus::Idle => "idle",
            AgentStatus::Running => "running",
            AgentStatus::Waiting => "waiting",
            AgentStatus::Stopped => "stopped",
            AgentStatus::Interrupted => "interrupted",
        }
    }

    fn retry_is_auth_failure(events: &[Value]) -> bool {
        events.iter().any(|event| {
            event["phase"] == "stopped"
                && event["stop_reason"] == "non_retryable"
                && matches!(
                    event["failed_kind"].as_str(),
                    Some("authentication" | "permission_denied")
                )
                && matches!(event["status"].as_u64(), Some(401 | 403))
        })
    }

    fn event_attempt_count(prepared_steps: u64, events: &[Value]) -> u64 {
        prepared_steps
            + events
                .iter()
                .filter(|event| event["phase"] == "started")
                .count() as u64
    }

    fn write_evidence(path: &Path, content: &str) -> Result<(), Box<dyn Error>> {
        fs::write(path, content)?;
        Ok(())
    }

    #[test]
    fn live_provider_configs_parse_without_resolving_credentials() -> Result<(), Box<dyn Error>> {
        let scratch = tempfile::tempdir()?;
        for name in ["deepseek", "openrouter"] {
            let route = route(name).unwrap();
            let path = scratch.path().join(format!("{name}.toml"));
            fs::write(&path, config(&route))?;
            let resolved = load_config(&path)?.resolve()?;
            assert_eq!(route.model, resolved.tiers["balanced"].model);
            assert_eq!(route.base_url, resolved.providers["live"].base_url);
        }
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    #[ignore = "requires authorized live provider env and writes sanitized evidence outside the repo"]
    async fn session_live_retry_evidence_is_bounded() -> Result<(), Box<dyn Error>> {
        require_live_tests();
        let provider_name = env::var("KUKU_LIVE_PROVIDER")?;
        let scenario = env::var("KUKU_LIVE_SCENARIO")?;
        let api_key = env::var("KUKU_LIVE_API_KEY")?;
        let route = route(&provider_name).ok_or("unknown KUKU_LIVE_PROVIDER")?;
        if !matches!(scenario.as_str(), "normal" | "invalid_auth") {
            return Err("scenario must be normal or invalid_auth".into());
        }

        let scratch = tempfile::tempdir()?;
        let workspace = scratch.path().join("workspace");
        let data = scratch.path().join("session-data");
        fs::create_dir(&workspace)?;
        let config_path = scratch.path().join("config.toml");
        fs::write(&config_path, config(&route))?;
        let config = load_config(&config_path)?.resolve()?;
        let mut options = SessionOptions::new(&workspace, &data, config);
        options.limits.max_requests_per_open = Some(MAX_REQUESTS);
        options.limits.max_output_tokens_per_request = Some(MAX_OUTPUT_TOKENS);
        options.limits.max_request_bytes = MAX_REQUEST_BYTES;
        options.limits.max_model_requests = 1;
        options.limits.retry_transient_provider_errors = true;

        let session = Session::open(options).await?;
        let mut observer = session.observe(0);
        session.create_agent(
            "create-main",
            "main",
            AgentConfig {
                connection: Some("live".into()),
                model: Some(route.model.into()),
                reasoning_effort: Some("low".into()),
                max_output_tokens: Some(MAX_OUTPUT_TOKENS),
                instructions: Some("Return exactly LIVE_OK. Do not call tools.".into()),
                no_agents: true,
                no_skills: true,
                tools: Some(Vec::new()),
                ..Default::default()
            },
        )?;

        let started = Instant::now();
        session.send(Input::new(
            "live-request",
            "main",
            "Reply with exactly LIVE_OK. Do not call tools.",
        ))?;
        session.advance("main")?;

        let mut retry_events = Vec::new();
        let mut prepared_steps = 0;
        let mut saw_model_response = false;
        let mut observer_lagged = false;
        let settled = tokio::time::timeout(RUN_TIMEOUT, async {
            loop {
                loop {
                    match observer.try_next() {
                        Ok(Some(observation)) => record_observation(
                            observation,
                            started,
                            &api_key,
                            &mut retry_events,
                            &mut prepared_steps,
                            &mut saw_model_response,
                            &mut observer_lagged,
                        ),
                        Ok(None) => break,
                        Err(_) => return Err("session observer failed"),
                    }
                }
                let snapshot = session
                    .agent("main")
                    .map_err(|_| "could not read final agent state")?;
                if snapshot.status != AgentStatus::Running
                    && (snapshot.status != AgentStatus::Idle || saw_model_response)
                {
                    return Ok(snapshot);
                }
                match observer.next().await {
                    Ok(Some(observation)) => record_observation(
                        observation,
                        started,
                        &api_key,
                        &mut retry_events,
                        &mut prepared_steps,
                        &mut saw_model_response,
                        &mut observer_lagged,
                    ),
                    Ok(None) => return Err("session observer closed before settlement"),
                    Err(_) => return Err("session observer failed"),
                }
            }
        })
        .await;

        let (snapshot, run_error) = match settled {
            Ok(Ok(snapshot)) => (snapshot, None),
            Ok(Err(error)) => (session.agent("main")?, Some(error)),
            Err(_) => {
                let _ = tokio::time::timeout(Duration::from_secs(3), session.cancel("main")).await;
                (session.agent("main")?, Some("session timeout"))
            }
        };
        while let Ok(Some(observation)) = observer.try_next() {
            record_observation(
                observation,
                started,
                &api_key,
                &mut retry_events,
                &mut prepared_steps,
                &mut saw_model_response,
                &mut observer_lagged,
            );
        }
        let close_succeeded = session.close().await.is_ok();

        let normal_success = snapshot.status == AgentStatus::Idle
            && snapshot
                .last_output
                .as_deref()
                .is_some_and(|output| output.contains("LIVE_OK"))
            && snapshot.usage.input_tokens.is_some()
            && snapshot.usage.output_tokens.is_some();
        let auth_failure = retry_is_auth_failure(&retry_events)
            && snapshot.status != AgentStatus::Idle
            && snapshot.usage.model_requests == 1
            && !retry_events
                .iter()
                .any(|event| matches!(event["phase"].as_str(), Some("waiting" | "started")));
        let attempts_from_events = event_attempt_count(prepared_steps, &retry_events);
        let final_outcome = match scenario.as_str() {
            "normal" if normal_success => "success",
            "invalid_auth" if auth_failure => "permanent_auth_failure",
            _ if run_error == Some("session timeout") => "run_timeout",
            _ if snapshot.status == AgentStatus::Idle => "unexpected_response",
            _ => "provider_error",
        };
        let generated_at = OffsetDateTime::now_utc().format(&Rfc3339)?;
        let evidence_json = env::var("KUKU_LIVE_EVIDENCE_JSON")?;
        let evidence_markdown = env::var("KUKU_LIVE_EVIDENCE_MD")?;
        let commit = env::var("KUKU_LIVE_COMMIT")?;
        let report = json!({
            "schema_version": 1,
            "generated_at_utc": generated_at,
            "commit": commit,
            "provider": route.provider,
            "model": route.model,
            "base_url": route.base_url,
            "profile": route.profile,
            "wire_format": route.format,
            "upstream": route.upstream,
            "scenario": scenario,
            "limits": {
                "max_requests_per_open": MAX_REQUESTS,
                "max_output_tokens": MAX_OUTPUT_TOKENS,
                "max_context_tokens": MAX_CONTEXT_TOKENS,
                "max_request_bytes": MAX_REQUEST_BYTES,
                "run_timeout_seconds": RUN_TIMEOUT.as_secs(),
            },
            "provider_request_timeout_configurable": false,
            "final_status": status_name(snapshot.status),
            "final_outcome": final_outcome,
            "response_contains_expected_marker": normal_success,
            "prepared_steps": prepared_steps,
            "provider_attempts": snapshot.usage.model_requests,
            "attempts_from_events": attempts_from_events,
            "usage": snapshot.usage,
            "retry_events": retry_events,
            "actual_retry_waits": wait_intervals(&retry_events),
            "observer_lagged": observer_lagged,
            "session_closed": close_succeeded,
        });
        let json_text = serde_json::to_string_pretty(&report)?;
        if json_text.contains(&api_key) {
            return Err("refusing to write evidence containing the active credential".into());
        }
        let markdown = format!(
            "# Sanitized live provider evidence\n\n- Commit: `{}`\n- UTC: `{}`\n- Provider/model: `{}` / `{}`\n- Upstream: `{}`\n- Scenario: `{}`\n- Outcome: `{}` (`{}`)\n- Prepared steps / provider attempts: {} / {}\n- Usage: `{}`\n- Request timeout config: unavailable; no induced timeout attempted\n\n## Retry events and observed waits\n\n```json\n{}\n```\n",
            report["commit"].as_str().unwrap_or(""),
            report["generated_at_utc"].as_str().unwrap_or(""),
            report["provider"].as_str().unwrap_or(""),
            report["model"].as_str().unwrap_or(""),
            report["upstream"].as_str().unwrap_or(""),
            report["scenario"].as_str().unwrap_or(""),
            report["final_outcome"].as_str().unwrap_or(""),
            report["final_status"].as_str().unwrap_or(""),
            prepared_steps,
            snapshot.usage.model_requests,
            serde_json::to_string(&report["usage"])? ,
            serde_json::to_string_pretty(&report["retry_events"])?
        );
        write_evidence(Path::new(&evidence_json), &json_text)?;
        write_evidence(Path::new(&evidence_markdown), &markdown)?;
        println!(
            "live evidence: provider={} scenario={} outcome={} attempts={} retry_events={} json={}",
            route.provider,
            report["scenario"].as_str().unwrap_or(""),
            final_outcome,
            snapshot.usage.model_requests,
            retry_events.len(),
            evidence_json
        );

        let scenario_passed = match scenario.as_str() {
            "normal" => normal_success && !observer_lagged && close_succeeded,
            "invalid_auth" => auth_failure && !observer_lagged && close_succeeded,
            _ => false,
        };
        assert!(
            scenario_passed,
            "live scenario did not meet its expected outcome; see sanitized evidence"
        );
        Ok(())
    }
}
