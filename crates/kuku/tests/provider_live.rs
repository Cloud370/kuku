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

    std::fs::write(
        workspace.path().join("README.md"),
        "# LIVE_PROMPT_CONTEXT_OK\n",
    )
    .unwrap();

    let output = query("Read README.md with tools and reply with exactly: LIVE_PROMPT_CONTEXT_OK")
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
        .expect("live Anthropic call should succeed");

    let events = EventStore::replay(
        session_events_path(
            home.path(),
            &std::fs::canonicalize(workspace.path()).unwrap(),
            &output.session_id,
        )
        .unwrap(),
    )
    .unwrap();

    assert!(output.text.contains("LIVE_PROMPT_CONTEXT_OK"));
    assert!(events.iter().any(|event| matches!(
        event.payload,
        EventPayload::ToolCall { ref tool, .. } if tool == "read_file"
    )));

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
}
