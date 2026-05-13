use kuku::{query, Provider};

const GATE_VAR: &str = "KUKU_MANUAL_PROVIDER_TESTS";

fn require_gate() {
    match std::env::var(GATE_VAR) {
        Ok(value) if value == "1" => {}
        _ => panic!("set {GATE_VAR}=1 to run manual provider tests"),
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires KUKU_MANUAL_PROVIDER_TESTS=1 and provider env vars"]
async fn anthropic_real_call_returns_text() {
    require_gate();
    let api_key = std::env::var("KUKU_ANTHROPIC_API_KEY").expect("KUKU_ANTHROPIC_API_KEY required");
    let base_url = std::env::var("KUKU_ANTHROPIC_BASE_URL")
        .unwrap_or_else(|_| "https://sub.f2.cm:19443/v1".into());

    let output = query("Say exactly: ok")
        .provider(Provider::Anthropic)
        .model("claude-haiku-4-5-20251001")
        .base_url(base_url)
        .api_key(api_key)
        .max_output_tokens(10)
        .run()
        .await
        .expect("real Anthropic call should succeed");

    assert!(!output.text.is_empty());
    eprintln!("Anthropic response: {}", output.text);
    eprintln!("Session: {}", output.session_id);
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires KUKU_MANUAL_PROVIDER_TESTS=1 and provider env vars"]
async fn openai_real_call_returns_text() {
    require_gate();
    let api_key = std::env::var("KUKU_OPENAI_API_KEY").expect("KUKU_OPENAI_API_KEY required");
    let base_url = std::env::var("KUKU_OPENAI_BASE_URL")
        .unwrap_or_else(|_| "https://sub.f2.cm:19443/v1".into());

    let output = query("Say exactly: ok")
        .provider(Provider::OpenAiCompatible)
        .model("gpt-5.4-mini")
        .base_url(base_url)
        .api_key(api_key)
        .max_output_tokens(10)
        .run()
        .await
        .expect("real OpenAI call should succeed");

    assert!(!output.text.is_empty());
    eprintln!("OpenAI response: {}", output.text);
    eprintln!("Session: {}", output.session_id);
}
