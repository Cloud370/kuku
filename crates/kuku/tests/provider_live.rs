use kuku::event::{EventPayload, EventStore};
use kuku::session::session_events_path;
use kuku::{Provider, query};

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

    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();

    std::fs::write(workspace.path().join("AGENTS.md"), "version one\n").unwrap();
    std::fs::write(
        workspace.path().join("README.md"),
        "# LIVE_PROMPT_CONTEXT_OK\n",
    )
    .unwrap();

    let session_id =
        query("Read README.md with tools and reply with exactly: LIVE_PROMPT_CONTEXT_OK")
            .kuku_home(home.path())
            .workspace(workspace.path())
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
    .kuku_home(home.path())
        .workspace(workspace.path())
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
