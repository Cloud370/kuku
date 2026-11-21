# Testing

This page owns verification entrypoints, live-test configuration, and evidence requirements. [Development](development.md) owns the work loop; [Evolution](../architecture/evolution.md#validation-and-measurements) defines rewrite-specific cases and measurements.

## Verification selection

Match checks to the changed behavior. Server fixtures reuse one local HTTP client and a shared case deadline for readiness waits; per-step startup times are not performance assertions. Failure snapshots retain only bounded execution state, excluding configuration and long transcript text. Keep one owner for each invariant: retry limits and backoff belong to the existing virtual-clock provider tests, while Session tests cover interruption and explicit recovery. Do not duplicate their full combination by waiting through real backoff in another integration test. The maintained selector uses Python 3.10+ and writes bounded evidence under the ignored `benchmarks/acceptance/` directory:

```bash
python3 scripts/acceptance/run.py contract
python3 scripts/acceptance/run.py protocol
python3 scripts/acceptance/run.py integration
```

On Windows, the workspace selector runs independent Rust test cases sequentially (`--test-threads=1`); concurrency exercised inside each test is unchanged. Shell syntax and output checks use the shared child-start budget. The process-timeout test first observes its real child processes, then advances Tokio time to trigger the deadline and checks actual process cleanup.

`contract` runs the workspace tests with `kuku/test_support` enabled, including the gated runtime, shared client and TUI suites. It continues the remaining test binaries after a test failure and returns failure if any tests fail. `protocol` selects the provider adapter/configuration/error, integration, trace, and cache-notice fixtures without making paid requests. `integration` runs the read-only format check, workspace/all-target Clippy with the test feature, and `contract`. Draft PRs and main pushes run one Linux workspace contract and Python tooling selection, formatting and Clippy. Ready-for-review PRs run the full workspace contract and installed terminal and browser acceptance on Linux. macOS and Windows run installed native smoke checks for first run, model editing, terminal resizing, paste and terminal restoration. Python tooling checks run on all three platforms. The Linux browser handoff shares the installed application with the terminal cases. Live model calls and Linux benchmark containers require explicit local selection.

Contract captures keep the original 64 KiB output tail and full-output hash. A separate failure summary retains failed test names, panic locations with limited context, and Cargo target failures within 16 KiB of UTF-8 output. `failure_summary.truncated` reports omitted content; the subprocess exit status still determines failure.

Contract, release installation and PTY builds produce Cargo's native `--timings` reports under `target/cargo-timings/`. CI retains them in `build-timings-<os>` artifacts so compilation can be assessed separately from test execution. Compare cache state, features and profiles before attributing a timing change to source changes.

Both workflows support PRs targeting `main` or `feat/interactive-clients` and pushes to `main`. WebUI checks are limited to `apps/web/**` and their workflow file; Rust changes no longer trigger a duplicate installed-browser build. PR checks use GitHub's merge-ref checkout consistently. Record the actual commit/tree and reuse source-head evidence only when the trees match. Superseded runs for the same PR are canceled. Python tooling failures do not prevent Rust contract and installed acceptance jobs from collecting their own results. Rust dependency caches are retained after failures; Cargo still validates build fingerprints. The PTY helper shares the contract's workspace/`kuku/test_support` feature resolution and records its compiler identity. CI builds the Web assets once per run. Each native job uses those assets to install its platform binary, or reuses an installed binary cached for the same merge commit, OS, architecture and compiler. CI verifies the installed revision before running acceptance.

Ready PR acceptance reuses the debug application and Rust artifacts from the contract build where possible. Browser acceptance still uses embedded assets in the debug binary. Cache the installed application by exact revision and toolchain, and the independent PTY helper by its source, dependency lockfile, platform and compiler. Manual release acceptance continues to verify the release build on native runners.

Manual CI dispatch defaults to `platform=all`. Select `linux`, `macos` or `windows` to investigate one platform; the run title identifies the selection, and separate platform runs do not cancel each other. Manual dispatch runs the full contract and installed suite for each selected platform, including Windows desktop and Linux browser handoff.

A green draft PR is not evidence of installed acceptance. Before merging, require the ready-for-review PR checks for the intended revision to pass. Use a full manual run with `platform=all` for release candidates and broader cross-platform investigation. Installed acceptance uses a binary with embedded Web assets; Windows desktop preparation is required for its selected desktop check. Independent installed scenarios use isolated services and continue after a case fails, preserving the original error and an overall failing result. The existing per-command timeouts remain in force. Case durations are printed, and up to 16 KiB of failure details are repeated after the final case so later output cannot hide the failed terminal frame. Controlled provider replies must follow verified tool results; a fixture success message is not tool-success evidence. A command may finish in the foreground or return a Job handle. The fixture waits with `wait_job` and checks the durable command facts before reporting success. Installed recovery also deliberately waits for automatic Job promotion, then checks the restored start card and completion notice through normal history navigation. Tests that require an execution to remain active hold the provider request until the relevant assertions finish, rather than relying on a fixed response delay.

The selector fixes `CARGO_TARGET_DIR` to this checkout's `target/` without changing the host environment. Give each worktree its own target directory, even for serial builds: shared workspace artifacts can be incorrectly reused from another source tree. A local target may be seeded with third-party dependency cache, but clear workspace outputs once before its first build using `cargo clean --target-dir target -p kuku -p kuku-cli -p kuku-server -p kuku-client -p kuku-tui -p kuku-app`. Record commit/tree, target directory, features, profile and binary hash; a frozen binary built from a shared target is not by itself proof of its source revision.

Use focused checks while editing, then run the required Rust integration checks once for a runtime integration batch. Equivalent direct commands are:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --features kuku/test_support -- -D warnings
cargo test --workspace --no-fail-fast --features kuku/test_support
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
python3 scripts/acceptance/run.py interactive --native-smoke
python3 scripts/acceptance/run.py interactive --archive PATH_TO_PLATFORM_ARCHIVE --browser
python3 scripts/acceptance/run.py interactive --archive NEW_ARCHIVE --upgrade-from OLD_ARCHIVE
python3 scripts/acceptance/run.py interactive --desktop --archive PATH_TO_LINUX_ARCHIVE
python3 scripts/acceptance/run.py interactive --desktop --archive PATH_TO_WINDOWS_ARCHIVE --windows-terminal PATH_TO_WINDOWS_TERMINAL_EXE
```

The default selection uses the repository's Node version, builds the Web assets and installs the current workspace's `kuku` with `embedded-web-assets`. `--archive` tests the release `.tar.gz` or `.zip` for the current platform. `--binary` copies a previously built or installed executable. The temporary install prefix, fresh data directory and workspaces are outside the checkout. `--scratch-base` selects another external scratch root. The selector records the binary hash, native terminal type, chosen scenarios and results in its usual evidence directory.

To test a package upgrade, provide two platform archives with different executables that both support durable Sessions. The selector uses the platform installer to replace the executable while a tool is running. It checks that restart defers, the original tool finishes once, and the new service reopens the Session.

The terminal scenarios drive an installed `kuku` on Linux and macOS through a Unix PTY, and on Windows through ConPTY. The shared terminal emulator preserves the previous frame during DEC synchronized updates and exposes the new frame only after commit; [protocol tests](../../../apps/kuku/tests/terminal_state.rs) cover fragmented updates, resize and cursor reports. Use `--browser` with an asset-enabled binary to add the Chromium scenario. Install the Playwright Python package and its Chromium browser as configured in the [CI browser preparation](../../../.github/workflows/ci.yml). The scenarios use a controlled local model endpoint and exercise real clients and the real service without provider credentials or paid requests. The [interactive client plan](../architecture/interactive-clients.md#testing-during-development) lists the behaviors to verify at each layer. It drives Ctrl+C and menu completion through the terminal, holds a model request and a tool open to inspect their distinct states, and checks the connection indicator when the service disconnects. A controlled provider reports input, cache reads and output so the installed terminal can be checked against a known context percentage, cumulative usage and cache hit rate, alongside measured throughput and the selected Agent and directory.

The maintained lifecycle scenarios repair a missing credential through the first authenticated reply, save a model default and inspect the actual model used by new and reopened Sessions, and withdraw an exact queued input with keyboard Up/Delete while preserving its draft and another queued input. They also reopen read-only rewound content and verify that it is absent from the next provider request, select an exact rewind target beyond the latest 48 committed messages, and recover from a controlled 401 through a masked credential update and safe service restart. The latter verifies that only an explicit Continue retries the retained input, once, using the replacement credential. Keyboard coverage complements the existing pointer withdrawal checks.

The native smoke scenario checks composer actions at 60 and 120 columns, including slash completion, Ctrl+T, the Agent and model thinking settings, and the inline exit confirmation while a service request remains active. Use `interactive --native-smoke --binary PATH_TO_BINARY` or `--archive PATH_TO_PLATFORM_ARCHIVE` to check a candidate on the host platform. The terminal frames appear in `installed-interactive.log`. [Composer scenario](../../../scripts/acceptance/interactive_composer.py)

For terminal layout comparisons, use the same fixture inputs and grid sizes for both builds. The installed scenario checks `/statusline` preview, saving, reopening and defaults, and exercises navigation among working Agents with unsent drafts. The dense transcript fixture uses at least 20 calls in one Turn, including successful operations, a missing file and a gated command. It captures wide and narrow terminal grids during tool execution and an open code fence, then verifies the completed Chinese report, retained tool facts and the unsent draft. Read the bounded `installed-interactive.log` for the terminal frames and checks. [Terminal scenarios](../../../scripts/acceptance/interactive.py)

For a local streaming comparison, run the same replay against both native release binaries with a different scratch directory for each run. The profile records the source hash, first visible text and burst input echo, and late-chunk paint samples at 60 and 120 columns. The paint measurements begin at the local fixture's flush. The Rust profile measures Markdown layout directly and reports how many bytes and rows the renderer reused. [Native replay](../../../scripts/acceptance/interactive_stream.py)

```bash
python3 scripts/acceptance/interactive.py --stream-profile --binary KUKU_BINARY --bridge NATIVE_PTY_HELPER --directory SCRATCH_DIR
cargo test --release -p kuku-tui stream_layout_profile -- --ignored --nocapture
```

Use `--desktop` on Linux with Docker to add the Xfce Terminal and IBus Pinyin scenario. The selector builds the desktop image from Ubuntu packages and runs it in an isolated Xvfb session with only loopback networking. It drives Chinese composition, editable multiline paste, **Messages → Copy**, and terminal font zoom through the GUI. On a cold build, Docker needs access to the Ubuntu image and package repositories. The installed executable and the selector's native test helper must run in the image. `desktop/result.json` records package versions, terminal grid sizes, submitted and copied fixture text, and hashes for the bounded screenshots.

The desktop scenario reuses the dense transcript fixture to capture wide and narrow views and verify copying the report's source text. Keep those screenshots with `desktop/result.json`, alongside the action menu, model selector and status line editor captures when collected. [Desktop scenarios](../../../scripts/acceptance/desktop)

For Windows Terminal, use a dedicated active test desktop with its own clipboard, the US keyboard layout and default Windows Terminal shortcuts. Install Pillow in the scenario's Python environment, then pass the existing `WindowsTerminal.exe` with `--windows-terminal`. The scenario drags across part of a reply and checks the Windows clipboard after release. Enable Microsoft Pinyin to exercise candidate composition and commit. If Pinyin is unavailable, the scenario records the results for copy, editable multiline paste and font zoom and reports incomplete GUI coverage. The [Windows CI job](../../../.github/workflows/ci.yml) prepares its own portable Terminal, attempts to configure Pinyin and keeps the preflight report with the scenario evidence. Record macOS hands-on checks with their own platform evidence.

The explicit `--live` selection uses the platform installer, the release archive, Chromium, and a budgeted DeepSeek call through the installed clients. It requires the DeepSeek key in the explicit credential file and an existing [budget ledger](#credentials-and-run-artifacts). `--ledger` selects a different existing ledger when authorized. Use a unique run ID and the authorized ledger.

```bash
python3 scripts/acceptance/run.py --run-id LIVE_RUN_ID interactive --archive PATH_TO_PLATFORM_ARCHIVE --live --secrets PATH_TO_CREDENTIAL_FILE
```

`live-result.json` records the actual route and requests alongside the task result. The scenario checks the real project edit and test output, switching Agents, client replacement during a running tool, and handoff among the terminal, browser and script client. [Live scenario](../../../scripts/acceptance/interactive_live.py)

Use `--intent deepseek` or `--intent openrouter` to check real model actions through the native terminal. The cases verify visible reply wording, exact file contents across turns, and independent delegation. The delegation case checks that the first creation request succeeds and that main receives the child's result before showing it. `--intent-case delivery` or `--intent-case delegation` runs one part of the suite.

```bash
python3 scripts/acceptance/run.py --run-id INTENT_RUN_ID interactive \
  --intent openrouter --intent-case delegation --binary PATH_TO_BINARY \
  --intent-model openai/gpt-6.1-terra --intent-effort high --intent-upstream openai \
  --revision EXPECTED_GIT_COMMIT --scratch-base PATH_OUTSIDE_CHECKOUT \
  --secrets PATH_TO_CREDENTIAL_FILE --ledger PATH_TO_AUTHORIZED_LEDGER
```

The selection accepts `--archive` in place of `--binary` and checks the installed binary's revision before reserving budget. Its defaults are `deepseek-flash` for direct DeepSeek, `openai/gpt-6-luna` through the `openai` endpoint for OpenRouter, and `low` reasoning effort. Use `--intent-model` and `--intent-effort` to override the model and requested effort, and `--intent-upstream` to choose the exact OpenRouter endpoint tag. The runner enforces request and output limits, including retries. `intent/result.json` records case checks, terminal captures, and usage. The intent suite requires confirmation of the route. With OpenRouter it checks the reported model, upstream provider, and available cost against generation receipts. Settle the reservation using the selected ledger after reviewing the result. [Intent scenarios](../../../scripts/acceptance/interactive_intent.py) · [Credentials and run artifacts](#credentials-and-run-artifacts)

Record deterministic checks, platform execution and hands-on experience separately. For hands-on release evidence, install the candidate outside the checkout, use a configured provider to read a file, make an edit and run a check, then verify the changed files, diff and tool results. Resume the same Session in the browser and another terminal. Record Linux and Windows Terminal IME, paste, copy and resize checks along with any macOS coverage. Follow the live-call requirements below for provider evidence and mark any unavailable coverage as unverified.

## Workspace execution acceptance

Use `workspace` to exercise the same large-log task through the public SDK example or an installed terminal client. It runs a native log fixture, checks the exact generated report and observed command facts, then reopens the Session and verifies that a named value can reproduce the report without rerunning the fixture. The product runs with an isolated `PATH`. Python drives acceptance from outside the product process. [Workspace scenario](../../../scripts/acceptance/workspace.py)

```bash
cargo build --locked -p kuku --example session_task
python3 scripts/acceptance/run.py workspace --sdk target/debug/examples/session_task
python3 scripts/acceptance/run.py workspace --binary PATH_TO_BINARY
```

Use a `.exe` path for the Windows build. `--archive` tests a local release archive in place of `--binary`. The selector builds the native fixture unless `--bridge` supplies an existing build. Add `--scratch-base` to select a scratch directory outside the source tree. With `--sdk`, add `--handoff-only` to seed a named report and verify that an independent reviewer reads the shared reference and reports its findings.

The default uses a local deterministic provider. For live workspace acceptance, run direct DeepSeek `deepseek-flash` at `high` and cross-check OpenRouter `openai/gpt-6.1-terra` at `high` through `openai`. These are the defaults when selecting the corresponding `--provider`. Pass the matching `--secrets` and `--ledger` for each service. The following commands record each selection explicitly through the public SDK caller.

```bash
python3 scripts/acceptance/run.py --run-id WORKSPACE_DEEPSEEK_RUN workspace \
  --sdk target/debug/examples/session_task --provider deepseek --model deepseek-flash \
  --effort high --secrets PATH_TO_CREDENTIAL_FILE --ledger PATH_TO_DEEPSEEK_LEDGER
python3 scripts/acceptance/run.py --run-id WORKSPACE_OPENROUTER_RUN workspace \
  --sdk target/debug/examples/session_task --provider openrouter --model openai/gpt-6.1-terra \
  --effort high --upstream openai --secrets PATH_TO_CREDENTIAL_FILE --ledger PATH_TO_OPENROUTER_LEDGER
```

`workspace/result.json` records task checks, accounting, the model and reasoning effort sent, and route evidence. The `workspace` selector allows `upstream_confirmed: null` when confirmation is unavailable. If OpenRouter returns no generation receipt, report the requested endpoint pin and keep the actual upstream unknown. Report the reasoning effort as requested unless the provider reports it. A confirmed route mismatch fails the run. [Credentials and run artifacts](#credentials-and-run-artifacts)

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

### Watch a TUI run together (optional)

Use an interactive terminal to submit the task and watch the Agent work. When tmux is available, a named session lets someone join the same TUI and lets you capture the current viewport. From the checkout or worktree under test:

```sh
cargo build --locked -p kuku-app --bin kuku
tmux new-session -d -s kuku-review -c "$PWD" \
  "$PWD/target/debug/kuku" --workspace "$PWD"
tmux set-option -t kuku-review mouse on
tmux attach-session -t kuku-review
```

From an already attached tmux client, use `tmux switch-client -t kuku-review`. In the TUI, use the mouse wheel or Page Up and Page Down to scroll, and Ctrl+End to return to the latest output. Normal attachment forwards these inputs. Use a different session name for each concurrent case. When different worktrees need independent managed services, give each TUI its own `--data-dir` and select its configuration with `--config`.

Use the Session journal or `kuku events SESSION_ID` with the same connection and workspace options for task progress, tool results and request boundaries. Use `tmux capture-pane -p -t kuku-review` to check how those facts appear on screen. Keep any captured text with the run evidence.

For an upstream benchmark, run the kuku service in the pinned task container. If kuku is already available and configured in that container, run the TUI there through the host's tmux. Set `benchmark_container` to the running task container's name:

```sh
tmux new-session -d -s kuku-benchmark \
  docker exec -it "$benchmark_container" kuku --workspace /app
tmux set-option -t kuku-benchmark mouse on
tmux attach-session -t kuku-benchmark
```

A TUI on the host can also connect to the container service with `--remote` and the container's path as `--workspace`. Paste the original instructions, then run the original verifier after the Agent and its Jobs settle. Use the original benchmark procedure above for verifier isolation and evidence. [Terminal controls](../reference/cli.md#terminal-interface) · [Remote connection](../guides/run-the-server.md#start-an-explicit-server)

## FrontierHarness public sample

The [case manifest](../../../scripts/acceptance/cases.json) pins the `frontier` suite to FrontierHarness Eval v1.1.0 tasks. The historical sample covers log aggregation, async cancellation, regular expressions and large-file Vim editing. The local comparison also includes Nginx configuration and a SQLite coverage build. Each task uses the upstream prompt, image build context and grader. On Linux with Git, Docker, Python 3.10+ and the repository's Rust toolchain, first check that an empty submission fails the original grader. This step requires network access for build and grader dependencies and uses no model credits. [Dataset provenance](https://hub.harborframework.com/datasets/frontier-harness-eval/frontier-harness-eval)

```sh
cargo build --locked -p kuku --example session_task
python3 scripts/acceptance/run.py --run-id smoke-regex benchmark \
  --suite frontier --case regex-log --mode preflight
```

For live runs, place `KUKU_TEST_OPENROUTER_API_KEY=...` in a private file outside the repository. A host-side meter holds the upstream key and caps each case against a shared USD ledger. The example reserves up to $0.75 per case and pins OpenRouter to DeepSeek's own endpoint for `deepseek/deepseek-v4.1-flash-1017` at `max` effort. Choose the total before the first run, reuse its ledger for repeats and use a fresh run ID for each attempt. The example passes an explicit 131,072-token allowance to reproduce the historical cohort. Omit `--max-output-tokens` to use the fixed route’s declared completion capacity. If the route reports no capacity, supply an explicit value. The meter checks that one worst-case request fits the dollar budget before reserving funds. The per-case dollar cap and task timeout bound the default run. To reproduce a historical request-limited cohort, add its recorded `--max-requests` value. Token ceilings bound generation and only actual usage or explicitly identified conservative estimates contribute to settled spend.

```sh
ledger="$PWD/benchmarks/acceptance/frontier-budget.json"
keys="$HOME/.config/kuku/frontier.env"
python3 scripts/acceptance/run.py budget init --ledger "$ledger" \
  --authorization local-frontier-sample --limit-usd 3
printf '%s\n' log-summary-date-ranges cancel-async-tasks regex-log large-scale-text-editing | \
  xargs -P 4 -I '{}' python3 scripts/acceptance/run.py --run-id 'sample-{}' benchmark \
    --suite frontier --case '{}' --mode live --provider openrouter --upstream deepseek \
    --max-output-tokens 131072 --case-cap-usd 0.75 \
    --sdk-binary target/debug/examples/session_task --ledger "$ledger" --secrets "$keys"
```

If account data policy excludes the requested provider, use a separately named trial with a compatible pinned endpoint such as `--upstream baseten`. Record the route change alongside the result. OpenRouter may report cost in the completed stream before the generation receipt becomes available. Add `--refresh-receipts "$keys"` to the report command below to fetch available generation receipts for those attempts. The report reuses verified receipts offline and counts each charge once.

Download the official snapshot and generate a shareable report. Supply each case's attempts in chronological order, including every failure. Each local result separates the upstream reward, SDK completion, model cost and verifier duration. Pi's official figures use the same model version and include four trials per task, audited scoring, Runta and Baseten. For cache comparisons use the recorded service provider and observed cached input divided by observed total input, with coverage and the initial request identified. Private session files, credentials and raw request payloads stay out of the public report. [Official results](https://frontierharness.org/live/) · [OpenRouter routing](https://openrouter.ai/docs/guides/routing/provider-selection)

```sh
mkdir -p benchmarks/acceptance/public
curl -fsSL https://frontierharness.org/api/live -o benchmarks/acceptance/public/live.json
python3 scripts/acceptance/frontier_report.py --official-live benchmarks/acceptance/public/live.json \
  --attempt log-summary-date-ranges benchmarks/acceptance/sample-log-summary-date-ranges \
  --attempt cancel-async-tasks benchmarks/acceptance/sample-cancel-async-tasks \
  --attempt regex-log benchmarks/acceptance/sample-regex-log \
  --attempt large-scale-text-editing benchmarks/acceptance/sample-large-scale-text-editing \
  --output benchmarks/acceptance/public/results.json
```

The SDK samples its own Linux `getrusage(RUSAGE_SELF)` immediately before writing its result, including process peak RSS, CPU time and filesystem block input/output counts. The parent sampler estimates mean RSS by time-weighting adjacent SDK samples and reports its observed window. The last Linux `/proc/PID/io` reading can include waited-for children. It distinguishes logical characters and system calls from storage bytes. Container-wide snapshots include the tool processes. Verifier time and resources are outside the SDK sampling window. The meter distinguishes its time to the first content-bearing stream delta from native provider metrics and reports denominators for TPS and cache comparisons.

The [sample results and session findings](../../../scripts/acceptance/results/frontier-sample.md) include the exploratory attempts and the shared-protocol rerun.

### Strict local Kuku–Pi comparison

Use one Pi reference run per selected task. Start with a task that distinguishes the harnesses, and use a second task with a different workload to check whether a proposed improvement generalizes. For rapid feedback, run two different tasks concurrently with unique run IDs and a shared spend ledger. Add `--cpus 2` to both runs for a task when gathering feedback on CPU-heavy work such as the SQLite build. The report requires matching CPU quotas. Identify concurrent runs and their quotas in the results. For wall-time and CPU comparisons, run each attempt on the same idle host. For repeats, alternate which harness goes first and retain every attempt, including failures. Use the headless commands below for comparison and record TUI observation runs separately.

Use Linux Docker with cgroup v2 and `memory.peak`, and a Linux Node.js executable compatible with Pi (22.19 or newer for Pi 1.5.2). Each run starts a fresh container from the original pinned task. Both harnesses use the original prompt and verifier, direct DeepSeek Chat Completions, `deepseek-flash` at `max`, and identical request, time and spend ceilings. The strict defaults are one CPU, 2 GiB of memory and no swap. Resource acceptance requires matching kernel limits, a harness RSS observation and zero cgroup OOM events during the agent phase.

Install the reference in an ignored directory and build the Kuku runner in release mode. Keep the npm lockfile to reproduce that dependency tree with `npm ci`. Put `KUKU_TEST_DEEPSEEK_API_KEY=...` in the private file referenced by `keys`, and initialize the ledger with your authorized total before a paid run.

```sh
pi_ref="$PWD/benchmarks/tools/pi-1.5.2"
npm install --prefix "$pi_ref" --registry=https://registry.npmjs.org --save-exact \
  --ignore-scripts --no-audit --no-fund @earendil-works/pi-coding-agent@1.5.2
cargo build --release --locked -p kuku --example session_task

ledger="$PWD/benchmarks/acceptance/paired-budget.json"
keys="$HOME/.config/kuku/frontier.env"
case_id=large-scale-text-editing
python3 scripts/acceptance/run.py budget init --ledger "$ledger" \
  --authorization local-paired-frontier --limit-usd 1.50

paired_run() {
  run_id="$1"
  shift
  python3 scripts/acceptance/run.py --run-id "$run_id" benchmark \
    --suite frontier --case "$case_id" --mode live --provider deepseek --require-resource-evidence \
    --max-output-tokens 131072 --case-cap-usd 0.75 --ledger "$ledger" --secrets "$keys" "$@"
}
paired_run vim-kuku --sdk-binary target/release/examples/session_task || test "$?" -eq 1
paired_run vim-pi --pi-prefix "$pi_ref" || test "$?" -eq 1

python3 scripts/acceptance/benchmark_compare.py \
  --kuku benchmarks/acceptance/vim-kuku --pi benchmarks/acceptance/vim-pi \
  > benchmarks/acceptance/vim-comparison.json
```

The exporter checks the task file digest, Docker image ID, model route and limits before pairing the attempts. Compare verifier reward and tests, agent and verifier wall time, model requests, token counts and estimated USD, then resource acceptance, harness RSS, container peak memory, CPU seconds and OOM events. Container peak is the kernel high-water mark from startup to the pre-verifier snapshot and includes memory charged to the cgroup for the harness, tool subprocesses and measurement overhead. Harness RSS covers the primary process's resident pages, including shared mappings. CPU seconds are the delta between snapshots around agent execution. Share the comparison JSON together with the rounds, host and build profile used for the test. Include the Pi lockfile if readers need to reproduce the exact dependency tree. Keep credentials, raw prompts, tool output and Session files private.

## Resource measurements

```bash
python3 scripts/acceptance/run.py metrics
python3 scripts/acceptance/run.py metrics --profile release
python3 scripts/acceptance/run.py metrics --profile release --cpu-count 1 --address-space-mib 512
```

This local selection builds [`resource_probe.rs`](../../../crates/kuku/examples/resource_probe.rs) in the checkout's isolated target and runs its public `Session` calls against the existing fixed SSE/configuration helpers. It never loads paid credentials or the spend ledger. Defaults are 32 idle Agents, 64 real queued-input appends, five close/reopen cycles, a 4 MiB command output with a 1 MiB retention quota, and two independently advancing Agents. The child emitting output reuses the caller binary; it does not introduce another SDK executor.

`caller.json` records workload sizes, sample arrays and units, nearest-rank append/reopen percentiles, initial Session-open time, output bytes/quota/read bounds, concurrency and durable paths. It reports SDK `context_prepare_us` and `request_prepare_us` only when those actual preparation facts exist; input-to-completion latency is a separate measurement and cannot substitute for them. Initial open and reopen do not flush the operating-system filesystem cache.

The workspace workload compares the same native log processed in bounded pages by the public SDK reader and the embedded script. It records first and repeated script latency, worker counts and the process samples at peak load and after the task. Two Sessions share one `WorkspaceExecutionHost` while starting more scripts than the worker budget. The retention checks track disk growth, reclaim ordinary captures and verify that a named baseline survives reopen and attempts to exceed quota. Defaults are six repeated tasks, two script workers, and four concurrent script host operations. Set `--repeated-tasks`, `--script-workers`, or `--script-host-operations` to vary the workload.

Linux `/proc` and Windows process APIs sample the SDK caller and its tool descendants separately. The in-process fixed provider is included in the caller footprint, with its baseline recorded separately. CPU is in seconds and RSS in KiB. Unsampled phases and unavailable counters stay unknown. Logical retained-output and event-file bytes are recorded independently. The run includes build profile, target directory, binary/source hashes, process limits and cleanup result. `measurements_complete` and `missing_measurements` distinguish local task success from missing measurements.

On Linux, `--cpu-count` applies CPU affinity to the caller and descendants. `--address-space-mib` limits each process's virtual address space. The Rust caller is portable. Process sampling in this selector is currently available on Linux and Windows.

Use `--pressure-rounds 3` to compare several peak and idle phases in one caller. On Linux, the selector also records the anonymous and file-backed memory reported by `/proc/<pid>/smaps_rollup` when available.

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

Use recorded streams to verify provider reasoning activity, visible text, interleaved argument fragments and tool dispatch. Check that partial arguments stay attached to the right call, that execution starts after dispatch, and that protocol-native reasoning data survives a tool continuation and then a reopen. Verify that the transcript exposes activity and visible reply text while keeping private reasoning out of the displayed output.

- For OpenAI Responses, match argument fragments by `output_index` and `item_id`, and match tool results by `call_id`. For stateless replay, verify output order, encrypted reasoning items and assistant message phase. [Streaming events](https://developers.openai.com/api/reference/resources/responses/streaming-events) · [Reasoning](https://developers.openai.com/api/docs/guides/reasoning)
- For Anthropic Messages, use the content block index to assemble tool input and reasoning independently. Include a signed thinking block with omitted text and verify the preserved block and signature on continuation. [Streaming messages](https://platform.claude.com/docs/en/build-with-claude/streaming) · [Thinking](https://platform.claude.com/docs/en/build-with-claude/thinking)
- For DeepSeek Chat Completions, verify that requests carrying `tools` replay `reasoning_content` from prior Turns, including Turns without tool calls. In DeepSeek Responses, verify the native reasoning items and `reasoning_text` events on the stateless path. [Thinking Mode](https://api-docs.deepseek.com/guides/thinking_mode/) · [Responses](https://api-docs.deepseek.com/guides/responses_api/)
- For OpenRouter Chat Completions, verify ordered `reasoning_details` and any signed or encrypted material in the continuation. For OpenRouter Responses, retain the ordered output items. Use reported upstream metadata to establish which provider served a live request. [Reasoning](https://openrouter.ai/docs/guides/best-practices/reasoning-tokens) · [Provider routing](https://openrouter.ai/docs/guides/routing/provider-selection)

DeepSeek Responses is stateless and does not support `previous_response_id`; its Messages compatibility layer ignores options including `service_tier` and `cache_control`. A successful HTTP response cannot prove those settings took effect. Keep unsupported explicit choices visible and distinguish these endpoint capabilities from the wire protocol itself.

## Credentials and run artifacts

Use the local credential file outside Git at `/code/.secrets/kuku-acceptance.env`, with directory mode `700` and file mode `600`. The user edits secrets locally. `KUKU_TEST_DEEPSEEK_API_KEY` holds the direct DeepSeek key; `KUKU_TEST_OPENROUTER_API_KEY` holds the separate OpenRouter key. Reuse each service's key for its selected routes. These are test-input names, not built-in Codex settings or a new SDK API. The [config reference](../reference/config.md) explains environment-backed credentials.

The caller explicitly loads the file without echoing it or enabling shell tracing; another shell's exports need not survive into a new session. This handoff requires the same filesystem and appropriate access. File permissions do not hide credentials from an Agent running as the owning user; another host/cloud environment needs its own secret provisioning.

Record the user-authorized request/spend scope, model/settings, output limits, timeout, and retry policy before paid tests. Enforce the applicable limits, including retries. Configuration limits are not fresh authorization to repeat a completed test allowance. Do not infer credentials from the coding Agent's model or silently increase reasoning/budget after a failure.

Use the ledger authorized for the selected API service. When DeepSeek and OpenRouter have separate authorizations, use separate ledgers. `benchmarks/acceptance/budget.json` is the default for generic budget commands. Select the applicable file explicitly when working with separate ledgers. Initialize a ledger once against the authorized scope and preserve its spending history.

```bash
python3 scripts/acceptance/run.py budget init --ledger PATH_TO_LEDGER \
  --authorization TASK_AUTHORIZATION --limit-usd 5 --opening-spent-usd 0
python3 scripts/acceptance/run.py budget show --ledger PATH_TO_LEDGER
```

Both `live` and `benchmark --mode live` require `--reserve-usd` and `--reservation-reason`. Before launching a paid process the selector hashes `--config` without copying its potentially inline credentials, then locks the ledger and reserves a conservative upper bound covering maximum requests, input/output tokens, provider prices, and all retries. Its `live` command requires `--sdk-result` and `--session-data` and runs the supplied public SDK/CLI caller after `--`, with credentials loaded explicitly from `--secrets`; it is not an HTTP connectivity probe. The caller must enforce the request/token/retry bounds used in the reservation. A price cap or ledger reservation alone does not bound a model loop; output limits remain an explicit caller choice rather than being reduced to fit a reservation.

Unfinished, failed or missing-usage runs keep their full reservation charged, including across interruption and restart. Null/absent usage stays unknown; zero input for a nonempty request does not establish free work. Partial reported counters/costs are labeled separately and cannot become a complete total. The selector does not automatically rerun paid commands or release reservations. Release unused reservation only after inspecting evidence, using `budget settle --entry RUN_ID --cost-usd AMOUNT --basis provider_reported|token_estimate --evidence PATH`. Keep provider-reported cost, token-based estimates and conservative upper bounds distinct. Use `budget show` before another request; insufficient remaining authorization prevents launch. A repeated run needs a new run ID and another reservation within the same authorization.

The sanitized local preflight and routing report is `benchmarks/provider-preflight.json` in the existing ignored development-output directory. Inspect its scope and coverage before repeating calls. It is not committed or guaranteed to exist in another checkout, and direct API results are not rewrite-runtime acceptance. Keep run timestamps and per-request details in such artifacts, not architecture or contributor prose.

## Documentation checks

Read back the changed pages and check local links/anchors, bilingual structure and meaning, technical literals, and `git diff --check`. If entrypoints or navigation change, re-check `README.md`, `docs/index.md`, `docs/en/index.md`, and `docs/zh/index.md` together. A passing structural check does not establish that the description is true.

## Zero warnings

The workspace/all-target Clippy command above requires zero warnings. Explain necessary lint allowances at the owning code location. See [Code Style](code-style.md) for code-level rules.
