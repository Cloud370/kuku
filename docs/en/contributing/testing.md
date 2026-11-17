# Testing

This page owns verification entrypoints, live-test configuration, and evidence requirements. [Development](development.md) owns the work loop; [Evolution](../architecture/evolution.md#validation-and-measurements) defines rewrite-specific cases and measurements.

## Verification selection

Match checks to the changed behavior. Use focused checks while editing, then run the required Rust integration checks once for a runtime integration batch:

```bash
cargo fmt --all -- --check
cargo clippy -- -D warnings
cargo test -p kuku -p kuku-cli -p kuku-server
```

Format the edited Rust scope after its writers have stopped, before the read-only format check. Use `cargo check` for fast feedback during editing. Do not repeat a passing check solely because commit or push follows; rerun when changed code, configuration, or integration invalidates its evidence. Documentation-only work uses the documentation checks below.

The default test command does not select every suite. The current feature-gated integration suites require:

```bash
cargo test -p kuku --features test_support --test provider_integration --test query_runtime --test cache_runtime_notices
```

Select the affected feature-gated suites explicitly and report which ran. Ignored live tests require both explicit selection and their credential/opt-in configuration. A skipped or unselected test is not coverage. When test ownership changes, update the actual entrypoints, CI selection, and this reference together.

| Evidence | What it establishes |
|---|---|
| Focused contract tests with real downstream components | Deterministic state, authorization, persistence, cancellation, and resource behavior; replace only the nondeterministic boundary |
| Recorded protocol/replay cases | Exact request/context and tool-result behavior without another paid model call; review fixture changes as behavior changes |
| Public-SDK/CLI live task | The assembled user path works against the configured model and its real tools |
| Original benchmark verifier and resource measurements | Task outcome and measured cost/performance under the recorded conditions |
| CI platform matrix | Results for the platforms and configurations actually exercised |

Check changed files, command exit status, durable facts, and resource cleanup independently of the Agent's final wording. A direct HTTP probe checks connectivity or provider behavior; it does not establish SDK integration. Real-model success does not establish rare failure guarantees.

## Test ownership and evidence

Unit tests remain in `#[cfg(test)] mod tests`; integration tests stay under `tests/` by domain, with shared helpers in `tests/common/mod.rs`. Reuse existing callers and fixtures. The rewrite should provide one maintained entrypoint or small case manifest for selecting contract, replay, live, and benchmark checks, without building an evaluation platform.

A run record identifies the code/tree state, case and verifier, command, enabled features, environment, effective model/settings, approved budget, result, and artifact locations. Report selected/passed/failed/skipped coverage and distinguish infrastructure failure, runtime failure, and failure of the Agent to solve the task. Detailed output stays in bounded artifacts; the task or PR carries the concise conclusion and evidence reference. Never print keys or private request bodies.

For routing comparisons, hold the model, reasoning, workload, and budget fixed. Separate cold requests from continued conversations, preserve a stable per-conversation correlation, and compare reported cost, cached/uncached input, output, actual route, time to first output, and completion latency. Provider names alone do not prove endpoint identity or cache retention; a few synthetic turns do not establish a globally optimal route.

## Live protocol acceptance

The user selected direct DeepSeek `deepseek-flash` as the main low-cost benchmark model and OpenRouter's [`deepseek/deepseek-v4.1-flash-1017`](https://openrouter.ai/deepseek/deepseek-v4.1-flash-1017) for the second gateway. The services document the three protocol routes below. These are test connection inputs, not newly frozen SDK configuration names.

| Service | Wire protocol | Base URL | Request suffix | Source |
|---|---|---|---|---|
| DeepSeek | Chat Completions | `https://api.deepseek.com` | `/chat/completions` | [First call](https://api-docs.deepseek.com/) |
| DeepSeek | Responses | `https://api.deepseek.com` | `/responses` | [Responses compatibility](https://api-docs.deepseek.com/guides/responses_api/) |
| DeepSeek | Messages | `https://api.deepseek.com/anthropic` | `/v1/messages` | [Messages compatibility](https://api-docs.deepseek.com/guides/anthropic_api/) |
| OpenRouter | Chat Completions | `https://openrouter.ai/api/v1` | `/chat/completions` | [Quickstart](https://openrouter.ai/docs/quickstart) |
| OpenRouter | Responses | `https://openrouter.ai/api/v1` | `/responses` | [Responses reference](https://openrouter.ai/docs/api/api-reference/responses/create-a-response) |
| OpenRouter | Messages | `https://openrouter.ai/api/v1` | `/messages` | [Messages reference](https://openrouter.ai/docs/api/api-reference/anthropic-messages/create-a-message) |

Start with DeepSeek in stage 1 and exercise the three protocols on both services before declaring stage 2 complete. Each needs streaming output, a real tool-call/result continuation, multi-turn context, and available usage/settings reporting. Keep repeated benchmark runs primarily on DeepSeek; use a small fixed task for OpenRouter compatibility checks rather than rerunning every benchmark on every model/protocol combination. The selected pair covers two access paths within the same model family; it does not establish cross-family model coverage. Additional model families remain optional follow-up validation, not a requirement to buy or select two other models before the initial delivery. Include a delegated task using direct DeepSeek and an OpenRouter-backed Agent with independent contexts.

For protocol-only checks, use the lowest supported enabled reasoning effort. DeepSeek currently documents `low` as that effort in its [thinking guide](https://api-docs.deepseek.com/guides/thinking_mode/); validate the chosen OpenRouter upstream's supported settings and encode them for the selected API rather than assuming all endpoints accept the same fields or values. Benchmark effort is chosen for the case and budget, recorded explicitly, and kept fixed within a comparison. Changes require a separately identified run, not silent escalation after failure. These settings apply to models under test, not to the implementing Codex session's or its development agents' model policies.

For repeatable OpenRouter checks, first select an upstream allowed by the account's routing policies, then pin its exact endpoint through `provider.order`, set `provider.allow_fallbacks = false` and `provider.require_parameters = true`, and avoid automatic model selection or model fallback. Its [routing documentation](https://openrouter.ai/docs/guides/routing/provider-selection) explains that default routing can otherwise fall back or pass unsupported parameters to a provider that ignores them. Record requested model, gateway, selected route, and reported upstream when available; an unavailable confirmation remains unknown. These are test connection options, not global runtime scheduling rules. Manual provider ordering overrides OpenRouter's automatic sticky routing; a fixed endpoint can still reuse its cache.

For ordinary multi-turn use, the recommended OpenRouter configuration is a stable `session_id` or `x-session-id`, supported-parameter filtering, and cost constraints on the eligible providers, leaving automatic sticky routing active. Derive the transport correlation from existing Session/Agent/connection identity and preserve it across Turns, retries, and reopen; separate Agents keep separate correlations. Do not create a new core identity or generate a fresh correlation on each Step. This is provider-adapter behavior, not a new scheduler or shared Agent transcript. Follow the [prompt-caching contract](https://openrouter.ai/docs/guides/best-practices/prompt-caching): explicit session correlation enables affinity from the first successful request; provider failure can change the route. The documented 60-minute inactivity expiry concerns routing affinity, not a guarantee of the upstream cache's lifetime. Stable routing does not replace immutable provider-visible prefixes.

OpenRouter's current default routing weighs effective price, including observed cache usage and input/output mix, alongside performance and reliability. It does not promise the cheapest request. Before choosing a provider allowlist or price caps, compare uncached input, cache-read/write, and output charges for the actual workload. `provider.max_price` limits eligible unit prices, not total session spend; maintain a separate run budget. Optional `provider.only` can restrict candidates without manually ordering them. Check available cached-token usage, final cost, and actual route over multiple turns. Validate these configuration recommendations for the selected endpoints; they do not guarantee cost savings.

Use focused native-protocol fixtures for rare or nondeterministic errors, batching, and provider-specific replay data that a gateway may normalize. OpenRouter coverage is gateway coverage; it is not evidence that the same requests work against official OpenAI or Anthropic endpoints. Direct official routes remain useful additional checks when credentials are available, without making new accounts a prerequisite for starting the rewrite. Record exactly which paths ran, and mark missing live coverage as unverified.

DeepSeek Responses is stateless and does not support `previous_response_id`; its Messages compatibility layer ignores options including `service_tier` and `cache_control`. A successful HTTP response cannot prove those settings took effect. Keep unsupported explicit choices visible and distinguish these endpoint capabilities from the wire protocol itself.

## Credentials and run artifacts

Use the local credential file outside Git at `/code/.secrets/kuku-acceptance.env`, with directory mode `700` and file mode `600`. The user edits secrets locally. `KUKU_TEST_DEEPSEEK_API_KEY` holds the direct DeepSeek key; `KUKU_TEST_OPENROUTER_API_KEY` holds the separate OpenRouter key. Each service's key is reused across its three protocols. These are test-input names, not built-in Codex settings or a new SDK API. The [config reference](../reference/config.md) explains environment-backed credentials.

The caller explicitly loads the file without echoing it or enabling shell tracing; another shell's exports need not survive into a new session. This handoff requires the same filesystem and appropriate access. File permissions do not hide credentials from an Agent running as the owning user; another host/cloud environment needs its own secret provisioning.

Record the user-authorized request/spend scope, model/settings, output limits, timeout, and retry policy before paid tests. Enforce the applicable limits, including retries. Configuration limits are not fresh authorization to repeat a completed test allowance. Do not infer credentials from the coding Agent's model or silently increase reasoning/budget after a failure.

The sanitized local preflight and routing report is `benchmarks/provider-preflight.json` in the existing ignored development-output directory. Inspect its scope and coverage before repeating calls. It is not committed or guaranteed to exist in another checkout, and direct API results are not rewrite-runtime acceptance. Keep run timestamps and per-request details in such artifacts, not architecture or contributor prose.

## Documentation checks

Read back the changed pages and check local links/anchors, bilingual structure and meaning, technical literals, and `git diff --check`. If entrypoints or navigation change, re-check `README.md`, `docs/index.md`, `docs/en/index.md`, and `docs/zh/index.md` together. A passing structural check does not establish that the description is true.

## Zero warnings

`cargo clippy -- -D warnings` remains required for the Rust integration checks. Explain necessary lint allowances at the owning code location. See [Code Style](code-style.md) for code-level rules.
