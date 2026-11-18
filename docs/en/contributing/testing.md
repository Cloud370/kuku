# Testing

This page owns verification entrypoints, live-test configuration, and evidence requirements. [Development](development.md) owns the work loop; [Evolution](../architecture/evolution.md#validation-and-measurements) defines rewrite-specific cases and measurements.

## Verification selection

Match checks to the changed behavior. The maintained selector uses Python 3.10+ and writes bounded evidence under the ignored `benchmarks/acceptance/` directory:

```bash
python3 scripts/acceptance/run.py contract
python3 scripts/acceptance/run.py protocol
python3 scripts/acceptance/run.py integration
```

`contract` runs the workspace tests with `kuku/test_support` enabled, including the gated runtime, shared client and TUI suites. `protocol` selects the provider adapter/configuration/error, integration, trace, and cache-notice fixtures without making paid requests. `integration` runs the read-only format check, workspace/all-target Clippy with the test feature, and `contract`. CI runs the same contract and installed terminal selections on Linux, macOS, and Windows and retains bounded evidence. Web CI also runs the installed browser scenario. Live model calls and Linux benchmark containers require explicit local selection.

The selector fixes `CARGO_TARGET_DIR` to this checkout's `target/` without changing the host environment. Give each worktree its own target directory, even for serial builds: shared workspace artifacts can be incorrectly reused from another source tree. A local target may be seeded with third-party dependency cache, but clear workspace outputs once before its first build using `cargo clean --target-dir target -p kuku -p kuku-cli -p kuku-server -p kuku-client -p kuku-tui -p kuku-app`. Record commit/tree, target directory, features, profile and binary hash; a frozen binary built from a shared target is not by itself proof of its source revision.

Use focused checks while editing, then run the required Rust integration checks once for a runtime integration batch. Equivalent direct commands are:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --features kuku/test_support -- -D warnings
cargo test --workspace --features kuku/test_support
```

Format the edited Rust scope after its writers have stopped, before the read-only format check. Use `cargo check` for fast feedback during editing. Do not repeat a passing check solely because commit or push follows; rerun when changed code, configuration, or integration invalidates its evidence. Documentation-only work uses the documentation checks below.

The old default test command without `test_support` does not select every suite. To focus just the gated runtime integrations:

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

## Installed client acceptance

Use the installed-client selection for changes to terminal input, service discovery, the embedded browser or release packaging.

```bash
python3 scripts/acceptance/run.py interactive
python3 scripts/acceptance/run.py interactive --archive PATH_TO_PLATFORM_ARCHIVE --browser
python3 scripts/acceptance/run.py interactive --archive NEW_ARCHIVE --upgrade-from OLD_ARCHIVE
python3 scripts/acceptance/run.py interactive --desktop --archive PATH_TO_LINUX_ARCHIVE
python3 scripts/acceptance/run.py interactive --desktop --archive PATH_TO_WINDOWS_ARCHIVE --windows-terminal PATH_TO_WINDOWS_TERMINAL_EXE
```

The default selection uses the repository's Node version, builds the Web assets and installs the current workspace's `kuku` with `embedded-web-assets`. `--archive` tests the release `.tar.gz` or `.zip` for the current platform. `--binary` copies a previously built or installed executable. The temporary install prefix, fresh data directory and workspaces are outside the checkout. `--scratch-base` selects another external scratch root. The selector records the binary hash, native terminal type, chosen scenarios and results in its usual evidence directory.

To test a package upgrade, provide two platform archives with different executables that both support durable Sessions. The selector uses the platform installer to replace the executable while a tool is running. It checks that restart defers, the original tool finishes once, and the new service reopens the Session.

The terminal scenarios drive an installed `kuku` on Linux and macOS through a Unix PTY, and on Windows through ConPTY. Use `--browser` with an asset-enabled binary to add the Chromium scenario. Install the Playwright Python package and its Chromium browser as configured in the [Web CI job](../../../.github/workflows/web-ci.yml). The scenarios use a controlled local model endpoint and exercise real clients and the real service without provider credentials or paid requests. The [interactive client plan](../architecture/interactive-clients.md#testing-during-development) lists the behaviors to verify at each layer. It drives Ctrl+C and menu completion through the terminal, holds a model request and a tool open to inspect their distinct states, and checks the connection indicator when the service disconnects. A controlled provider reports input, cache reads and output so the installed terminal can be checked against a known context percentage, cumulative usage and cache hit rate, alongside measured throughput and the selected Agent and directory.

For terminal layout comparisons, use the same fixture inputs and grid sizes for both builds. The installed scenario checks `/statusline` preview, saving, reopening and defaults, and exercises navigation among working Agents with unsent drafts. The dense transcript fixture uses at least 20 calls in one Turn, including successful operations, a missing file and a gated command. It captures wide and narrow terminal grids during tool execution and an open code fence, then verifies the completed Chinese report, retained tool facts and the unsent draft. Read the bounded `installed-interactive.log` for the terminal frames and checks. [Terminal scenarios](../../../scripts/acceptance/interactive.py)

Use `--desktop` on Linux with Docker to add the Xfce Terminal and IBus Pinyin scenario. The selector builds the desktop image from Ubuntu packages and runs it in an isolated Xvfb session with only loopback networking. It drives Chinese composition, editable multiline paste, `/copy`, and terminal font zoom through the GUI. On a cold build, Docker needs access to the Ubuntu image and package repositories. The installed executable and the selector's native test helper must run in the image. `desktop/result.json` records package versions, terminal grid sizes, submitted and copied fixture text, and hashes for the bounded screenshots.

The desktop scenario reuses the dense transcript fixture to capture wide and narrow views and verify copying the report's source text. Keep those screenshots with `desktop/result.json`, alongside the action menu, model selector and status line editor captures when collected. [Desktop scenarios](../../../scripts/acceptance/desktop)

For Windows Terminal, use a dedicated active test desktop with its own clipboard, the US keyboard layout and default Windows Terminal shortcuts. Install Pillow in the scenario's Python environment, then pass the existing `WindowsTerminal.exe` with `--windows-terminal`. The scenario drags across part of a reply and checks the Windows clipboard after release. Enable Microsoft Pinyin to exercise candidate composition and commit. If Pinyin is unavailable, the scenario records the results for copy, editable multiline paste and font zoom and reports incomplete GUI coverage. The [Windows CI job](../../../.github/workflows/ci.yml) prepares its own portable Terminal, attempts to configure Pinyin and keeps the preflight report with the scenario evidence. Record macOS hands-on checks with their own platform evidence.

The explicit `--live` selection uses the platform installer, the release archive, Chromium, and a budgeted DeepSeek call through the installed clients. It requires the DeepSeek key in the explicit credential file and an existing [budget ledger](#credentials-and-run-artifacts). `--ledger` selects a different existing ledger when authorized. Use a unique run ID and the authorized ledger.

```bash
python3 scripts/acceptance/run.py --run-id LIVE_RUN_ID interactive --archive PATH_TO_PLATFORM_ARCHIVE --live --secrets PATH_TO_CREDENTIAL_FILE
```

`live-result.json` records the actual route and requests alongside the task result. The scenario checks the real project edit and test output, switching Agents, client replacement during a running tool, and handoff among the terminal, browser and script client. [Live scenario](../../../scripts/acceptance/interactive_live.py)

Record deterministic checks, platform execution and hands-on experience separately. For hands-on release evidence, install the candidate outside the checkout, use a configured provider to read a file, make an edit and run a check, then verify the changed files, diff and tool results. Resume the same Session in the browser and another terminal. Record Linux and Windows Terminal IME, paste, copy and resize checks along with any macOS coverage. Follow the live-call requirements below for provider evidence and mark any unavailable coverage as unverified.

## Test ownership and evidence

Unit tests remain in `#[cfg(test)] mod tests`; integration tests stay under `tests/` by domain, with shared helpers in `tests/common/mod.rs`. Reuse existing callers and fixtures. The rewrite should provide one maintained entrypoint or small case manifest for selecting contract, replay, live, and benchmark checks, without building an evaluation platform.

A selector run produces one `result.json` with the commit, dirty state and tracked-diff hash, selected commands/features, exit statuses, duration, and evidence paths. Each command retains at most 64 KiB of redacted output, its original byte count and hash, and an explicit truncation flag. Timeouts terminate the owned process group on POSIX or request process-tree termination on Windows; incomplete capture is a failure. The selector’s [orphaned-output test](../../../scripts/acceptance/test_run.py) exercises POSIX process groups. It skips Windows. Live SDK checks independently verify the caller's durable facts, settings, usage and cleanup; command success alone cannot pass them.

Report selected/passed/failed/skipped coverage and distinguish infrastructure failure, runtime failure, and failure of the Agent to solve the task. Detailed output stays in bounded artifacts; the task or PR carries the concise conclusion and evidence reference. Never print keys or private request bodies. Test the selector itself without credentials using `python3 -m unittest discover -s scripts/acceptance -p 'test_*.py'`.

For routing comparisons, hold the model, reasoning, workload, and budget fixed. Separate cold requests from continued conversations, preserve a stable per-conversation correlation, and compare reported cost, cached/uncached input, output, actual route, time to first output, and completion latency. Provider names alone do not prove endpoint identity or cache retention; a few synthetic turns do not establish a globally optimal route.

## Original benchmark tasks

[`scripts/acceptance/cases.json`](../../../scripts/acceptance/cases.json) owns the small case selection and full upstream commit. It selects Terminal-Bench 2.0's `cancel-async-tasks` first, followed by `log-summary-date-ranges`, `openssl-selfsigned-cert`, and `regex-log`. The original instruction, Docker build context, solution and verifier remain unchanged. [Harbor's task contract](https://docs.harborframework.com/tasks/overview) permits using these self-contained tasks without installing another evaluation framework.

List the selection, then check one environment before spending model budget:

```bash
python3 scripts/acceptance/run.py list
python3 scripts/acceptance/run.py --run-id cancel-environment benchmark --case cancel-async-tasks --mode preflight
```

The Linux/Docker preflight checks out the fixed source into ignored `benchmarks/sources/`, records task-file/verifier SHA-256 hashes and the built image ID, and creates two fresh containers. The untouched environment must execute collected tests and fail; the original oracle must execute the same original `tests/test.sh` and pass every collected test, without skipped, pending or other outcomes. A zero reward without a collected test report is an infrastructure failure. After SDK/oracle execution, the host replaces the verifier output directory with an empty directory before grading, so an earlier report cannot count as this verifier's evidence. The verifier still installs its original pinned dependencies; download failures are not task failures and require a separately identified retry. The runner uses the task's original one-CPU/2 GiB limits and working directory and removes its own containers, reporting cleanup failures.

For a live attempt, build the public `session_task` example, then select `benchmark --mode live` with `--sdk-binary`, `--config`, `--connection` and the budget arguments below. Additional bounded caller arguments follow `--`. The binary, its resolved Linux shared libraries and host CA bundle are copied into a separate read-only runtime mount. Evidence records their hashes and the CA source; `SSL_CERT_FILE` points the SDK at that retained trust bundle, preserving certificate validation in task images without installed CA roots. Only the original instruction and task environment are available during Agent execution. The solution is never supplied, and hidden tests are copied in only after execution. Session data remains in the attempt's `session-data/`; expected task artifacts are listed in the manifest.

The original reward, collected/passed/failed test counts, SDK exit status, and cleanup result are recorded separately. Expected task artifacts are retained before grading, up to 32 MiB each; missing or oversized evidence cannot pass the live acceptance record. The SDK inspector streams original `events.jsonl`, requiring ordered committed cursors, matching prepared requests/responses, paired tool calls/results, unchanged admitted result views, and finalized `KUKUOUT1` output references with valid frames and byte counts. SDK counters/settings/reports must match those facts, and both the runtime and durable close barrier must report completion. Requests and billing are keyed by Agent, Turn and request identity, so a later Turn can reuse `req_1`. Session-wide usage remains separate from the selected Agent's snapshot. The original benchmark verifier still determines task success.

The inspector reads at most 1 MiB per event, 100,000 events and a 4 MiB SDK report, retaining bounded hashes/metadata rather than duplicating context history. Diagnostic limits are 4,096 prepared requests and 8,192 tool calls/provider reports. Unconfirmed context suffixes retain at most 16,384 tool-block fingerprints across segments; only the matching prepared head confirms their tool views. A retry replaces the candidate suffix, while confirmed prefixes and durable call/result matching remain strict. Recheck existing evidence without another model request using `inspect --sdk-result PATH --session-data DIR --container-data /kuku/data --connection NAME`; omit `--container-data` for a report written with host paths. This read-only selection needs neither credentials nor a budget reservation.

When the caller reports `streaming`, the inspector validates its counters, first-observation times and durable request/model-byte bounds. These are observations through the same SDK subscription, not replayable wire timings. Add `--require-streaming` to `inspect`, `live`, or a live benchmark when the selected protocol check requires nonempty model output without lagged updates. Older callers remain explicitly unreported. A model's `no_agents` setting disables its collaboration tools; it does not prevent the host from creating other independent Agents in the Session.

While the SDK runs, the selector samples SDK RSS/high-water RSS, CPU time and thread count separately from aggregate container CPU, memory, processes and I/O. Samples and output logs are bounded. Unsampled values stay `null`; do not attribute whole-container measurements to the SDK. The focused resource caller below measures the other lifecycle boundaries.

## Resource measurements

```bash
python3 scripts/acceptance/run.py metrics
python3 scripts/acceptance/run.py metrics --profile release
```

This local selection builds [`resource_probe.rs`](../../../crates/kuku/examples/resource_probe.rs) in the checkout's isolated target and runs its public `Session` calls against the existing fixed SSE/configuration helpers. It never loads paid credentials or the spend ledger. Defaults are 32 idle Agents, 64 real queued-input appends, five close/reopen cycles, a 4 MiB command output with a 1 MiB retention quota, and two independently advancing Agents. The child emitting output reuses the caller binary; it does not introduce another SDK executor.

`caller.json` records workload sizes, sample arrays and units, nearest-rank append/reopen percentiles, initial Session-open time, output bytes/quota/read bounds, concurrency and durable paths. It reports SDK `context_prepare_us` and `request_prepare_us` only when those actual preparation facts exist; input-to-completion latency is a separate measurement and cannot substitute for them. Initial open and reopen do not flush the operating-system filesystem cache.

Linux `/proc` sampling separates the SDK caller process from its tool descendants. The in-process fixed provider is included in the caller footprint, with its baseline recorded separately. CPU is in seconds and RSS in KiB; missing I/O counters or unsampled phases stay unknown. Logical retained-output and event-file bytes are recorded independently. The run includes build profile, target directory, binary/source hashes, process limits and cleanup result. `measurements_complete` and `missing_measurements` distinguish a successful local task from missing measurements; keep conditions fixed before comparing runs. The Rust caller remains portable, while this process sampler currently requires Linux.

## Live protocol acceptance

The user selected direct DeepSeek `deepseek-flash` for benchmarks and the three wire-protocol checks. The second model family uses OpenRouter's [`openai/gpt-6-luna`](https://openrouter.ai/openai/gpt-6-luna), pinned to the standard OpenAI provider through the OpenAI Responses API. This OpenRouter route is for protocol compatibility with a small fixed task. The services document the general routes below; the table is not a requirement to run every model/service/protocol combination. These are test connection inputs, not newly frozen SDK configuration names.

| Service | Wire protocol | Base URL | Request suffix | Source |
|---|---|---|---|---|
| DeepSeek | Chat Completions | `https://api.deepseek.com` | `/chat/completions` | [First call](https://api-docs.deepseek.com/) |
| DeepSeek | Responses | `https://api.deepseek.com` | `/responses` | [Responses compatibility](https://api-docs.deepseek.com/guides/responses_api/) |
| DeepSeek | Messages | `https://api.deepseek.com/anthropic` | `/v1/messages` | [Messages compatibility](https://api-docs.deepseek.com/guides/anthropic_api/) |
| OpenRouter | Chat Completions | `https://openrouter.ai/api/v1` | `/chat/completions` | [Quickstart](https://openrouter.ai/docs/quickstart) |
| OpenRouter | Responses | `https://openrouter.ai/api/v1` | `/responses` | [Responses reference](https://openrouter.ai/docs/api/api-reference/responses/create-a-response) |
| OpenRouter | Messages | `https://openrouter.ai/api/v1` | `/messages` | [Messages reference](https://openrouter.ai/docs/api/api-reference/anthropic-messages/create-a-message) |

Start with DeepSeek in stage 1. Stage 2 requires Chat Completions, Responses and Messages coverage on direct DeepSeek, plus the selected OpenRouter/OpenAI Responses compatibility path. Each selected path needs streaming output, a real tool-call/result continuation, multi-turn context, and available usage/settings reporting. Benchmarks remain on DeepSeek; OpenRouter targets the OpenAI model family without rerunning the benchmark set or requiring the other OpenRouter protocol combinations. Include a delegated task using direct DeepSeek and OpenRouter/OpenAI Agents with independent contexts.

For protocol-only checks, use the lowest supported enabled reasoning effort. DeepSeek currently documents `low` as that effort in its [thinking guide](https://api-docs.deepseek.com/guides/thinking_mode/); validate the chosen OpenRouter upstream's supported settings and encode them for the selected API rather than assuming all endpoints accept the same fields or values. Benchmark effort is chosen for the case and budget, recorded explicitly, and kept fixed within a comparison. Changes require a separately identified run, not silent escalation after failure. These settings apply to models under test, not to the implementing Codex session's or its development agents' model policies.

For the selected OpenRouter check, use `POST https://openrouter.ai/api/v1/responses` with standard OpenAI Responses request semantics. The model's [public endpoint metadata](https://openrouter.ai/api/v1/models/openai/gpt-6-luna/endpoints) identifies `openai` as the standard OpenAI endpoint tag; `openai/flex` and `openai/fast` select different service routes. Pin the selected route with this request snippet:

```json
{
  "model": "openai/gpt-6-luna",
  "provider": {
    "order": ["openai"],
    "allow_fallbacks": false,
    "require_parameters": true
  }
}
```

Verify that the account's routing policy allows that endpoint, and avoid automatic model selection or model fallback. OpenRouter's [routing documentation](https://openrouter.ai/docs/guides/routing/provider-selection) explains that default routing can otherwise fall back or pass unsupported parameters to a provider that ignores them. Record requested model, gateway, selected route, and reported upstream when available; an unavailable confirmation remains unknown. These are test connection options, not global runtime scheduling rules. Manual provider ordering overrides OpenRouter's automatic sticky routing; a fixed endpoint can still reuse its cache.

For ordinary multi-turn use, the recommended OpenRouter configuration is a stable `session_id` or `x-session-id`, supported-parameter filtering, and cost constraints on the eligible providers, leaving automatic sticky routing active. Derive the transport correlation from existing Session/Agent/connection identity and preserve it across Turns, retries, and reopen; separate Agents keep separate correlations. Do not create a new core identity or generate a fresh correlation on each Step. This is provider-adapter behavior, not a new scheduler or shared Agent transcript. Follow the [prompt-caching contract](https://openrouter.ai/docs/guides/best-practices/prompt-caching): explicit session correlation enables affinity from the first successful request; provider failure can change the route. The documented 60-minute inactivity expiry concerns routing affinity, not a guarantee of the upstream cache's lifetime. Stable routing does not replace immutable provider-visible prefixes.

OpenRouter's current default routing weighs effective price, including observed cache usage and input/output mix, alongside performance and reliability. It does not promise the cheapest request. Before choosing a provider allowlist or price caps, compare uncached input, cache-read/write, and output charges for the actual workload. `provider.max_price` limits eligible unit prices, not total session spend; maintain a separate run budget. Optional `provider.only` can restrict candidates without manually ordering them. Check available cached-token usage, final cost, and actual route over multiple turns. Validate these configuration recommendations for the selected endpoints; they do not guarantee cost savings.

Use focused native-protocol fixtures for rare or nondeterministic errors, batching, and provider-specific replay data that a gateway may normalize. OpenRouter coverage is gateway coverage; it is not evidence that the same requests work against official OpenAI or Anthropic endpoints. Direct official routes remain useful additional checks when credentials are available, without making new accounts a prerequisite for starting the rewrite. Record exactly which paths ran, and mark missing live coverage as unverified.

DeepSeek Responses is stateless and does not support `previous_response_id`; its Messages compatibility layer ignores options including `service_tier` and `cache_control`. A successful HTTP response cannot prove those settings took effect. Keep unsupported explicit choices visible and distinguish these endpoint capabilities from the wire protocol itself.

## Credentials and run artifacts

Use the local credential file outside Git at `/code/.secrets/kuku-acceptance.env`, with directory mode `700` and file mode `600`. The user edits secrets locally. `KUKU_TEST_DEEPSEEK_API_KEY` holds the direct DeepSeek key; `KUKU_TEST_OPENROUTER_API_KEY` holds the separate OpenRouter key. Reuse each service's key for its selected routes. These are test-input names, not built-in Codex settings or a new SDK API. The [config reference](../reference/config.md) explains environment-backed credentials.

The caller explicitly loads the file without echoing it or enabling shell tracing; another shell's exports need not survive into a new session. This handoff requires the same filesystem and appropriate access. File permissions do not hide credentials from an Agent running as the owning user; another host/cloud environment needs its own secret provisioning.

Record the user-authorized request/spend scope, model/settings, output limits, timeout, and retry policy before paid tests. Enforce the applicable limits, including retries. Configuration limits are not fresh authorization to repeat a completed test allowance. Do not infer credentials from the coding Agent's model or silently increase reasoning/budget after a failure.

`benchmarks/acceptance/budget.json` is the single local cost ledger for this rewrite. Initialize it once against the user's task authorization; do not recreate it to reset spending:

```bash
python3 scripts/acceptance/run.py budget init --authorization TASK_AUTHORIZATION --limit-usd 5 --opening-spent-usd 0
python3 scripts/acceptance/run.py budget show
```

Both `live` and `benchmark --mode live` require `--reserve-usd` and `--reservation-reason`. Before launching a paid process the selector hashes `--config` without copying its potentially inline credentials, then locks the ledger and reserves a conservative upper bound covering maximum requests, input/output tokens, provider prices, and all retries. Its `live` command requires `--sdk-result` and `--session-data` and runs the supplied public SDK/CLI caller after `--`, with credentials loaded explicitly from `--secrets`; it is not an HTTP connectivity probe. The caller must enforce the request/token/retry bounds used in the reservation. A price cap or ledger reservation alone does not bound a model loop; output limits remain an explicit caller choice rather than being reduced to fit a reservation.

Unfinished, failed or missing-usage runs keep their full reservation charged, including across interruption and restart. Null/absent usage stays unknown; zero input for a nonempty request does not establish free work. Partial reported counters/costs are labeled separately and cannot become a complete total. The selector does not automatically rerun paid commands or release reservations. Release unused reservation only after inspecting evidence, using `budget settle --entry RUN_ID --cost-usd AMOUNT --basis provider_reported|token_estimate --evidence PATH`. Keep provider-reported cost, token-based estimates and conservative upper bounds distinct. Use `budget show` before another request; insufficient remaining authorization prevents launch. A repeated run needs a new run ID and another reservation within the same authorization.

The sanitized local preflight and routing report is `benchmarks/provider-preflight.json` in the existing ignored development-output directory. Inspect its scope and coverage before repeating calls. It is not committed or guaranteed to exist in another checkout, and direct API results are not rewrite-runtime acceptance. Keep run timestamps and per-request details in such artifacts, not architecture or contributor prose.

## Documentation checks

Read back the changed pages and check local links/anchors, bilingual structure and meaning, technical literals, and `git diff --check`. If entrypoints or navigation change, re-check `README.md`, `docs/index.md`, `docs/en/index.md`, and `docs/zh/index.md` together. A passing structural check does not establish that the description is true.

## Zero warnings

The workspace/all-target Clippy command above requires zero warnings. Explain necessary lint allowances at the owning code location. See [Code Style](code-style.md) for code-level rules.
