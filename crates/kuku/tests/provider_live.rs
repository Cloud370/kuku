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
    use std::io;
    use std::path::Path;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll};
    use std::time::{Duration, Instant};
    use time::format_description::well_known::Rfc3339;
    use time::OffsetDateTime;
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::oneshot;
    use tokio::task::{JoinHandle, JoinSet};

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

    #[derive(Default, Clone)]
    struct ProxyStats {
        allowed_destination: bool,
        upstream_tcp_connections: u64,
        closed_tunnels: u64,
        forwarded_tunnels: u64,
        blocked_local_retries: u64,
        rejected_destinations: u64,
        first_close_after_start_ms: Option<u64>,
    }

    #[derive(Clone)]
    struct ProxyState {
        host: String,
        restore: Arc<AtomicBool>,
        stats: Arc<Mutex<ProxyStats>>,
        client_to_upstream_bytes: Arc<AtomicU64>,
        upstream_to_client_bytes: Arc<AtomicU64>,
        started: Instant,
    }

    struct ConnectProxy {
        url: String,
        state: ProxyState,
        shutdown: Option<oneshot::Sender<()>>,
        task: Option<JoinHandle<()>>,
    }

    struct ProxyEnvironment {
        old_https_proxy: Option<std::ffi::OsString>,
        old_all_proxy: Option<std::ffi::OsString>,
        old_no_proxy: Option<std::ffi::OsString>,
    }

    struct CountedRead<R> {
        inner: R,
        bytes: Arc<AtomicU64>,
    }

    struct EventCapture<'a> {
        started: Instant,
        api_key: &'a str,
        restore_after_waiting: Option<&'a AtomicBool>,
        retry_events: Vec<Value>,
        prepared_steps: u64,
        saw_model_response: bool,
        lagged: bool,
    }

    impl ConnectProxy {
        async fn start(host: &str) -> io::Result<Self> {
            let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
            let address = listener.local_addr()?;
            let state = ProxyState {
                host: host.to_owned(),
                restore: Arc::new(AtomicBool::new(false)),
                stats: Arc::new(Mutex::new(ProxyStats::default())),
                client_to_upstream_bytes: Arc::new(AtomicU64::new(0)),
                upstream_to_client_bytes: Arc::new(AtomicU64::new(0)),
                started: Instant::now(),
            };
            let server_state = state.clone();
            let (shutdown, mut stop) = oneshot::channel();
            let task = tokio::spawn(async move {
                let mut handlers = JoinSet::new();
                loop {
                    tokio::select! {
                        _ = &mut stop => break,
                        result = listener.accept() => match result {
                            Ok((client, _)) => {
                                let state = server_state.clone();
                                handlers.spawn(async move { serve_connect(client, state).await; });
                            }
                            Err(_) => break,
                        }
                    }
                }
                handlers.abort_all();
                while handlers.join_next().await.is_some() {}
            });
            Ok(Self {
                url: format!("http://{address}"),
                state,
                shutdown: Some(shutdown),
                task: Some(task),
            })
        }

        fn report(&self) -> Value {
            let stats = self.state.stats.lock().unwrap().clone();
            json!({
                "proxy": "local HTTP CONNECT tunnel; TLS is not intercepted",
                "allowed_destination": format!("{}:443", self.state.host),
                "upstream_tcp_connections": stats.upstream_tcp_connections,
                "closed_tunnels": stats.closed_tunnels,
                "forwarded_tunnels": stats.forwarded_tunnels,
                "blocked_local_retries": stats.blocked_local_retries,
                "rejected_destinations": stats.rejected_destinations,
                "first_close_after_start_ms": stats.first_close_after_start_ms,
                "restored_after_waiting_event": self.state.restore.load(Ordering::Acquire),
                "forwarded_tls_bytes_client_to_upstream": self.state.client_to_upstream_bytes.load(Ordering::Relaxed),
                "forwarded_tls_bytes_upstream_to_client": self.state.upstream_to_client_bytes.load(Ordering::Relaxed),
            })
        }

        async fn stop(&mut self) {
            if let Some(shutdown) = self.shutdown.take() {
                let _ = shutdown.send(());
            }
            if let Some(task) = self.task.take() {
                let _ = tokio::time::timeout(Duration::from_secs(3), task).await;
            }
        }
    }

    impl ProxyEnvironment {
        fn set(proxy_url: &str) -> Self {
            let old_https_proxy = env::var_os("HTTPS_PROXY");
            let old_all_proxy = env::var_os("ALL_PROXY");
            let old_no_proxy = env::var_os("NO_PROXY");
            env::set_var("HTTPS_PROXY", proxy_url);
            env::set_var("ALL_PROXY", proxy_url);
            env::set_var("NO_PROXY", "");
            Self {
                old_https_proxy,
                old_all_proxy,
                old_no_proxy,
            }
        }
    }

    impl Drop for ProxyEnvironment {
        fn drop(&mut self) {
            restore_env("HTTPS_PROXY", self.old_https_proxy.take());
            restore_env("ALL_PROXY", self.old_all_proxy.take());
            restore_env("NO_PROXY", self.old_no_proxy.take());
        }
    }

    fn restore_env(name: &str, value: Option<std::ffi::OsString>) {
        if let Some(value) = value {
            env::set_var(name, value);
        } else {
            env::remove_var(name);
        }
    }

    impl<R: AsyncRead + Unpin> AsyncRead for CountedRead<R> {
        fn poll_read(
            self: Pin<&mut Self>,
            context: &mut Context<'_>,
            buffer: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            let this = self.get_mut();
            let before = buffer.filled().len();
            match Pin::new(&mut this.inner).poll_read(context, buffer) {
                Poll::Ready(Ok(())) => {
                    let read = buffer.filled().len().saturating_sub(before) as u64;
                    this.bytes.fetch_add(read, Ordering::Relaxed);
                    Poll::Ready(Ok(()))
                }
                other => other,
            }
        }
    }

    impl<R: AsyncWrite + Unpin> AsyncWrite for CountedRead<R> {
        fn poll_write(
            self: Pin<&mut Self>,
            context: &mut Context<'_>,
            buffer: &[u8],
        ) -> Poll<io::Result<usize>> {
            Pin::new(&mut self.get_mut().inner).poll_write(context, buffer)
        }

        fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.get_mut().inner).poll_flush(context)
        }

        fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.get_mut().inner).poll_shutdown(context)
        }
    }

    async fn read_connect_target(client: &mut TcpStream) -> io::Result<String> {
        let mut request = Vec::new();
        let mut byte = [0_u8; 1];
        while request.len() < 8192 {
            if client.read(&mut byte).await? == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "CONNECT closed",
                ));
            }
            request.push(byte[0]);
            if request.ends_with(b"\r\n\r\n") {
                let first_line = String::from_utf8_lossy(&request)
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                return Ok(first_line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_owned());
            }
        }
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "CONNECT too large",
        ))
    }

    async fn serve_connect(mut client: TcpStream, state: ProxyState) {
        let target = match read_connect_target(&mut client).await {
            Ok(target) => target,
            Err(_) => return,
        };
        if target != format!("{}:443", state.host) {
            state.stats.lock().unwrap().rejected_destinations += 1;
            return;
        }
        state.stats.lock().unwrap().allowed_destination = true;

        if !state.restore.load(Ordering::Acquire) {
            let first = state.stats.lock().unwrap().closed_tunnels == 0;
            if !first {
                state.stats.lock().unwrap().blocked_local_retries += 1;
                return;
            }
            let upstream = match TcpStream::connect((state.host.as_str(), 443)).await {
                Ok(upstream) => upstream,
                Err(_) => return,
            };
            state.stats.lock().unwrap().upstream_tcp_connections += 1;
            if client
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await
                .is_ok()
            {
                let mut stats = state.stats.lock().unwrap();
                stats.closed_tunnels += 1;
                stats.first_close_after_start_ms =
                    Some(u64::try_from(state.started.elapsed().as_millis()).unwrap_or(u64::MAX));
            }
            drop(upstream);
            return;
        }

        let upstream = match TcpStream::connect((state.host.as_str(), 443)).await {
            Ok(upstream) => upstream,
            Err(_) => return,
        };
        state.stats.lock().unwrap().upstream_tcp_connections += 1;
        if client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await
            .is_err()
        {
            return;
        }
        state.stats.lock().unwrap().forwarded_tunnels += 1;
        let mut client_read = CountedRead {
            inner: client,
            bytes: Arc::clone(&state.client_to_upstream_bytes),
        };
        let mut upstream_read = CountedRead {
            inner: upstream,
            bytes: Arc::clone(&state.upstream_to_client_bytes),
        };
        let _ = tokio::io::copy_bidirectional(&mut client_read, &mut upstream_read).await;
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

    fn record_observation(observation: Observation, capture: &mut EventCapture<'_>) {
        let event = match observation {
            Observation::Committed(event) => event,
            Observation::Lagged { .. } => {
                capture.lagged = true;
                return;
            }
            _ => return,
        };
        match event.payload {
            EventPayload::ModelResponse { .. } => {
                capture.saw_model_response = true;
            }
            EventPayload::Unknown(value) => match value.get("kind").and_then(Value::as_str) {
                Some("runtime.step_prepared") => capture.prepared_steps += 1,
                Some("runtime.model_retry") => {
                    if value["phase"] == "waiting" {
                        if let Some(restore) = capture.restore_after_waiting {
                            restore.store(true, Ordering::Release);
                        }
                    }
                    let mut summary = Map::new();
                    for field in [
                        "kind",
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
                            Value::String(bounded.replace(capture.api_key, "[REDACTED]")),
                        );
                    }
                    summary.insert("event_id".into(), json!(event.id));
                    summary.insert(
                        "observed_elapsed_ms".into(),
                        json!(u64::try_from(capture.started.elapsed().as_millis())
                            .unwrap_or(u64::MAX)),
                    );
                    capture.retry_events.push(Value::Object(summary));
                }
                Some("model.response") => capture.saw_model_response = true,
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
        if !matches!(
            scenario.as_str(),
            "normal" | "invalid_auth" | "transport_restore"
        ) {
            return Err("scenario must be normal, invalid_auth, or transport_restore".into());
        }
        if scenario == "transport_restore" && provider_name != "deepseek" {
            return Err("transport_restore is restricted to the DeepSeek route".into());
        }

        let mut proxy = if scenario == "transport_restore" {
            Some(ConnectProxy::start("api.deepseek.com").await?)
        } else {
            None
        };
        let _proxy_environment = proxy
            .as_ref()
            .map(|proxy| ProxyEnvironment::set(&proxy.url));
        let restore_after_waiting = proxy.as_ref().map(|proxy| Arc::clone(&proxy.state.restore));

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

        let mut capture = EventCapture {
            started,
            api_key: &api_key,
            restore_after_waiting: restore_after_waiting.as_deref(),
            retry_events: Vec::new(),
            prepared_steps: 0,
            saw_model_response: false,
            lagged: false,
        };
        let settled = tokio::time::timeout(RUN_TIMEOUT, async {
            loop {
                loop {
                    match observer.try_next() {
                        Ok(Some(observation)) => record_observation(observation, &mut capture),
                        Ok(None) => break,
                        Err(_) => return Err("session observer failed"),
                    }
                }
                let snapshot = session
                    .agent("main")
                    .map_err(|_| "could not read final agent state")?;
                if snapshot.status != AgentStatus::Running
                    && (snapshot.status != AgentStatus::Idle || capture.saw_model_response)
                {
                    return Ok(snapshot);
                }
                match observer.next().await {
                    Ok(Some(observation)) => record_observation(observation, &mut capture),
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
            record_observation(observation, &mut capture);
        }
        let close_succeeded = session.close().await.is_ok();
        if let Some(proxy) = proxy.as_mut() {
            proxy.stop().await;
        }
        let proxy_report = proxy.as_ref().map(ConnectProxy::report);
        let retry_events = &capture.retry_events;
        let prepared_steps = capture.prepared_steps;
        let observer_lagged = capture.lagged;

        let normal_success = snapshot.status == AgentStatus::Idle
            && snapshot
                .last_output
                .as_deref()
                .is_some_and(|output| output.contains("LIVE_OK"))
            && snapshot.usage.input_tokens.is_some()
            && snapshot.usage.output_tokens.is_some();
        let auth_failure = retry_is_auth_failure(retry_events)
            && snapshot.status != AgentStatus::Idle
            && snapshot.usage.model_requests == 1
            && !retry_events
                .iter()
                .any(|event| matches!(event["phase"].as_str(), Some("waiting" | "started")));
        let attempts_from_events = event_attempt_count(prepared_steps, retry_events);
        let retry_waits = wait_intervals(retry_events);
        let observed_wait_ms = retry_waits
            .first()
            .and_then(|wait| wait["observed_wait_ms"].as_u64());
        let configured_delay_ms = retry_waits
            .first()
            .and_then(|wait| wait["configured_delay_ms"].as_u64());
        let transport_restore_success = scenario == "transport_restore"
            && normal_success
            && snapshot.usage.model_requests == 2
            && attempts_from_events == 2
            && retry_events.len() == 2
            && retry_events[0]["kind"] == "runtime.model_retry"
            && retry_events[0]["phase"] == "waiting"
            && retry_events[0]["failed_kind"] == "transport"
            && retry_events[1]["kind"] == "runtime.model_retry"
            && retry_events[1]["phase"] == "started"
            && configured_delay_ms.is_some_and(|delay| (2_000..=2_500).contains(&delay))
            && observed_wait_ms.is_some_and(|wait| (1_800..=3_500).contains(&wait))
            && proxy_report.as_ref().is_some_and(|proxy| {
                proxy["allowed_destination"] == "api.deepseek.com:443"
                    && proxy["upstream_tcp_connections"] == 2
                    && proxy["closed_tunnels"] == 1
                    && proxy["forwarded_tunnels"] == 1
                    && proxy["rejected_destinations"] == 0
                    && proxy["restored_after_waiting_event"] == true
                    && proxy["forwarded_tls_bytes_client_to_upstream"]
                        .as_u64()
                        .unwrap_or(0)
                        > 0
                    && proxy["forwarded_tls_bytes_upstream_to_client"]
                        .as_u64()
                        .unwrap_or(0)
                        > 0
            });
        let final_outcome = match scenario.as_str() {
            "normal" if normal_success => "success",
            "invalid_auth" if auth_failure => "permanent_auth_failure",
            "transport_restore" if transport_restore_success => "transport_retry_restored",
            _ if run_error == Some("session timeout") => "run_timeout",
            _ if snapshot.status == AgentStatus::Idle => "unexpected_response",
            _ => "provider_error",
        };
        let generated_at = OffsetDateTime::now_utc().format(&Rfc3339)?;
        let evidence_json = env::var("KUKU_LIVE_EVIDENCE_JSON")?;
        let evidence_markdown = env::var("KUKU_LIVE_EVIDENCE_MD")?;
        let commit = env::var("KUKU_LIVE_COMMIT")?;
        let timeout_note = if scenario == "transport_restore" {
            "no request timeout knob; CONNECT tunnel close induced a transport error"
        } else {
            "no public request timeout knob; no timeout scenario attempted"
        };
        let report = json!({
            "schema_version": 1,
            "generated_at_utc": generated_at,
            "commit": commit,
            "provider": route.provider,
            "model": route.model,
            "base_url": route.base_url,
            "profile": route.profile,
            "wire_format": route.format,
            "configured_upstream": route.upstream,
            "observed_upstream": null,
            "scenario": scenario,
            "limits": {
                "max_requests_per_open": MAX_REQUESTS,
                "max_output_tokens": MAX_OUTPUT_TOKENS,
                "max_context_tokens": MAX_CONTEXT_TOKENS,
                "max_request_bytes": MAX_REQUEST_BYTES,
                "run_timeout_seconds": RUN_TIMEOUT.as_secs(),
            },
            "provider_request_timeout_configurable": false,
            "transport_fault": proxy_report,
            "failed_tunnel_closed_before_tls_application_data": proxy_report
                .as_ref()
                .is_some_and(|proxy| proxy["closed_tunnels"] == 1),
            "final_status": status_name(snapshot.status),
            "final_outcome": final_outcome,
            "response_contains_expected_marker": normal_success,
            "prepared_steps": prepared_steps,
            "provider_attempts": snapshot.usage.model_requests,
            "attempts_from_events": attempts_from_events,
            "usage": snapshot.usage,
            "retry_events": retry_events,
            "actual_retry_waits": retry_waits,
            "observer_lagged": observer_lagged,
            "session_closed": close_succeeded,
        });
        let json_text = serde_json::to_string_pretty(&report)?;
        if json_text.contains(&api_key) {
            return Err("refusing to write evidence containing the active credential".into());
        }
        let markdown = format!(
            "# Sanitized live provider evidence\n\n- Commit: `{}`\n- UTC: `{}`\n- Provider/model: `{}` / `{}`\n- Configured/pinned upstream: `{}`\n- Independently observed upstream: not captured\n- Scenario: `{}`\n- Outcome: `{}` (`{}`)\n- Prepared steps / provider attempts: {} / {}\n- Usage: `{}`\n- Request timeout config: {timeout_note}\n\n## Retry events and observed waits\n\n```json\n{}\n```\n",
            report["commit"].as_str().unwrap_or(""),
            report["generated_at_utc"].as_str().unwrap_or(""),
            report["provider"].as_str().unwrap_or(""),
            report["model"].as_str().unwrap_or(""),
            report["configured_upstream"].as_str().unwrap_or(""),
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
            "transport_restore" => transport_restore_success && !observer_lagged && close_succeeded,
            _ => false,
        };
        assert!(
            scenario_passed,
            "live scenario did not meet its expected outcome; see sanitized evidence"
        );
        Ok(())
    }
}
