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
async fn anthropic_live_smoke_returns_text() {
    require_live_tests();
    let output = query("Reply with exactly: ok")
        .provider(Provider::Anthropic)
        .model(std::env::var("KUKU_ANTHROPIC_MODEL").expect("KUKU_ANTHROPIC_MODEL required"))
        .base_url(
            std::env::var("KUKU_ANTHROPIC_BASE_URL").expect("KUKU_ANTHROPIC_BASE_URL required"),
        )
        .api_key(std::env::var("KUKU_ANTHROPIC_API_KEY").expect("KUKU_ANTHROPIC_API_KEY required"))
        .max_output_tokens(128)
        .temperature(0.0)
        .run()
        .await
        .expect("live Anthropic call should succeed");

    assert!(!output.text.trim().is_empty());
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
