# Advanced acceptance

Use [Testing](testing.md) to select the routine checks. Run the recipes below only when the change needs that evidence. Use the checkout under test and a unique `--run-id` for each paid attempt. The [selector](../../../scripts/acceptance/run.py) provides `--help` for each subcommand.

## Installed client recipes

[The installed-client selector](testing.md#installed-client-acceptance) uses the repository's Node version and installs with embedded Web assets when you supply neither `--binary` nor `--archive`. It uses a separate installation, service data directory, and workspace outside the checkout. Use `--scratch-base` to choose that location. The run record includes the binary hash, terminal type, and selected cases.

| Goal | Run `python3 scripts/acceptance/run.py interactive` with these arguments |
| --- | --- |
| Focused native smoke | `--binary PATH_TO_BINARY --native-smoke` |
| Browser handoff | `--archive ARCHIVE --browser` or an asset-enabled `--binary` |
| Upgrade during an active tool | `--archive NEW_ARCHIVE --upgrade-from OLD_ARCHIVE` |
| Linux desktop and IBus | `--archive ARCHIVE --desktop` |
| Windows Terminal | `--archive ARCHIVE --desktop --windows-terminal WINDOWS_TERMINAL_EXE` |

For browser checks, prepare Playwright Chromium as in [CI](../../../.github/workflows/ci-platform.yml). Both upgrade packages must support durable Sessions. Check that the restart defers, the tool finishes once, and the replacement reopens the Session. For graphical checks, Linux requires Docker and uses an isolated Xvfb/Xfce Terminal with IBus. Windows requires a dedicated active desktop, its own clipboard, Pillow, and the expected keyboard layout and shortcuts. Enable Microsoft Pinyin for composition coverage. Record missing IME coverage separately from the checks that passed. [Installed cases](../../../scripts/acceptance/interactive.py) · [Desktop fixtures](../../../scripts/acceptance/desktop)

For a screen comparison, hold fixture inputs and grid sizes fixed. Generate a multi-Turn settlement case and replay it using another binary. Replay makes no provider requests.

```bash
python3 scripts/acceptance/interactive.py --binary PATH_TO_BINARY --bridge PATH_TO_PTY_HELPER \
  --directory REVIEW_RUN --cases release-review
python3 scripts/acceptance/interactive.py --binary PATH_TO_OTHER_BINARY --bridge PATH_TO_PTY_HELPER \
  --replay-review REVIEW_RUN/release-review --renderings comparison.json
```

For streaming latency and layout, use separate scratch directories and the same fixture for each build. The native profile records first visible text and burst echo, and the Rust profile measures Markdown layout directly. [Streaming fixture](../../../scripts/acceptance/interactive_stream.py)

```bash
python3 scripts/acceptance/interactive.py --stream-profile --binary KUKU_BINARY --bridge NATIVE_PTY_HELPER --directory SCRATCH_DIR
cargo test --release -p kuku-tui stream_layout_profile -- --ignored --nocapture
```

### Paid installed runs

Use an authorized [ledger and credential file](#credentials-and-run-artifacts). The live release handoff checks a real edit and test result, client replacement during a tool, and handoff among terminal, browser, and script clients. Read `live-result.json`. [Live scenario](../../../scripts/acceptance/interactive_live.py)

```bash
python3 scripts/acceptance/run.py --run-id LIVE_RUN_ID interactive \
  --archive ARCHIVE --live --secrets CREDENTIAL_FILE --ledger AUTHORIZED_LEDGER
```

Use `--intent deepseek` or `--intent openrouter` to verify native reply, edit, and independent delegation with a real model. Select a case, pin the desired route, and verify the installed revision before reserving budget. Use `--archive` in place of `--binary` to test a release package.

```bash
python3 scripts/acceptance/run.py --run-id INTENT_RUN_ID interactive \
  --intent openrouter --intent-case delegation --binary PATH_TO_BINARY \
  --intent-model MODEL --intent-effort EFFORT --intent-upstream UPSTREAM \
  --revision EXPECTED_GIT_COMMIT --scratch-base PATH_OUTSIDE_CHECKOUT \
  --secrets CREDENTIAL_FILE --ledger AUTHORIZED_LEDGER
```

Read `intent/result.json` for case checks, captures, usage, and confirmed route. The runner bounds requests and output including retries. For a paid `release-review` run, set authorized `--intent-max-requests` and `--intent-max-output-tokens` explicitly. To keep the Session for replay, use the [native intent runner](../../../scripts/acceptance/interactive_intent.py) with `--case release-review --keep-session` and the corresponding `--max-requests` and `--max-output-tokens`. The reported `scratch_path` is the replay input.

Record hands-on results separately. Install outside the checkout, verify a real file change and test result, and reopen the same Session in the browser and another terminal. Record IME, paste, copy, and resizing on each platform actually exercised.

## Workspace execution acceptance

Run the same native log fixture through the SDK or installed TUI. Verify the exact report, command facts, and a named value that reproduces the report after reopen. The default uses a controlled provider and an isolated product `PATH`. [Workspace fixture](../../../scripts/acceptance/workspace.py)

```bash
cargo build --locked -p kuku --example session_task
python3 scripts/acceptance/run.py workspace --sdk target/debug/examples/session_task
python3 scripts/acceptance/run.py workspace --binary PATH_TO_BINARY
```

Use `--archive` for a local release or `--handoff-only` with `--sdk` to test delivery of a named report to an independent reviewer. Use `.exe` paths on Windows. For a live run, select `--provider deepseek` or `--provider openrouter`, pass the matching `--secrets` and `--ledger`, and choose the requested model and effort. For OpenRouter set `--upstream` when the run requires an exact endpoint. In `workspace/result.json`, keep actual upstream and reasoning unknown when the provider has not confirmed them. A confirmed route mismatch fails the check.

### Command output and long-task acceptance

Use the [command output contract](../architecture/workspace-execution.md#proposed-command-output-path). Reuse the existing tool, Session, and installed fixtures for these checks.

| Boundary | Verify |
| --- | --- |
| Native invocation | Spaces, quotes, and Unicode in literal arguments. Compare the authorized invocation, actual effect, and TUI title. |
| Output shape and failure | Compare the exact model-visible view with retained output. Confirm large results expose a useful retrieval path, and failed results keep decisive diagnostics and capture completeness. |
| Search and output rules | Test cross-frame matches, authorization, cancellation, truncation, and generic fallback. Resolve diagnostic references against the source capture and verify the admitted view after reopen. |
| Jobs and long work | Check fast completion, promotion, cancellation, and one terminal result. Include a slow Job that makes progress and a quiet Job with a concrete blocker. |

Use controlled fixtures for what the model can receive and budgeted live tasks for what the model chooses. Keep model, effort, tools, budget, environment, and verifier fixed. Compare short direct work, a task suited to `run_js`, and a longer Job or verification task. Reserve two different tasks from Prompt tuning. Vary Prompt and output view independently before comparing the combined change. Record original verifier results, retries, repeated checks, provider usage and cache reads, latency, peak RSS, and retained output bytes.

## Watch a TUI run together (optional)

Use a dedicated tmux server in the checkout under test. This setup enables color and uses default tmux bindings.

```sh
cargo build --locked -p kuku-app --bin kuku
tmux -L kuku-review -f /dev/null new-session -d -s kuku-review -c "$PWD"
tmux -L kuku-review set-option -g default-terminal tmux-256color
tmux -L kuku-review set-option -t kuku-review mouse on
tmux -L kuku-review respawn-pane -k -t kuku-review \
  env -u NO_COLOR "$PWD/target/debug/kuku" --workspace "$PWD"
tmux -L kuku-review capture-pane -p -t kuku-review
tmux -L kuku-review attach -t kuku-review
```

Wait for `Ready`. Let one person drive the reproduction while the other reads the same pane and checks relevant events with `kuku events SESSION_ID --limit 40`. For before/after comparisons keep separate windows, fixture copies, data directories, and explicit service configs. Hold terminal sizes fixed. Check `TERM`, `NO_COLOR`, and `tmux list-keys -T root` when colors or Alt keys behave unexpectedly. [Terminal controls](../reference/cli.md#terminal-interface) · [Remote connection](../guides/run-the-server.md#start-an-explicit-server)

## Original benchmark tasks

The [case manifest](../../../scripts/acceptance/cases.json) owns case IDs, source revisions, and upstream images. Use the original task instructions, environment, and verifier. Run on Linux with Git and Docker. Check the untouched environment and original oracle before spending model budget. A verifier dependency or image failure is an infrastructure failure.

```bash
python3 scripts/acceptance/run.py list
python3 scripts/acceptance/run.py --run-id cancel-environment benchmark --case cancel-async-tasks --mode preflight
```

For a live Terminal-Bench run, build `session_task` and provide `--sdk-binary`, `--config`, `--connection`, and the [budget arguments](#credentials-and-run-artifacts). The original verifier determines task success. Keep verifier results separate from SDK exit status, artifacts, and cleanup. When assessing a timeout, check the Agent deadline, supervisor result, durable Session evidence, and verifier separately.

Use the local `inspect` command to verify existing durable evidence.

```bash
python3 scripts/acceptance/run.py inspect --sdk-result SDK_REPORT --session-data SESSION_DATA
```

For reports from containers, add `--container-data` for the reported mount prefix. The inspector checks ordered records, paired calls/results, admitted views, retained output references, and close evidence. Use `--require-streaming` on `inspect`, `live`, or a live benchmark when the selected check requires nonempty, unlagged SDK streaming observations.

### FrontierHarness public sample

Run a preflight against the pinned `frontier` suite. It checks that the untouched submission fails the original grader. Docker images and grader dependencies may need network access.

```bash
cargo build --locked -p kuku --example session_task
python3 scripts/acceptance/run.py --run-id smoke-regex benchmark --suite frontier --case regex-log --mode preflight
```

For a live run, select exactly one runner of `--sdk-binary`, `--pi-prefix`, and `--codex-prefix`. The selector builds the config and uses the host meter with `--case-cap-usd`. Pin the upstream and record model, effort, output/request/time limits, and the authorized shared ledger. Use a fresh run ID for each case. Historical sample and reproduction details live in the [sample report](../../../scripts/acceptance/results/frontier-sample.md).

### Strict local Kuku–Pi comparison

Use Linux Docker with cgroup v2 and `memory.peak`. Use fresh containers, the original task and verifier, the same pinned model/effort, and identical request, time, CPU, memory, and spend ceilings. The strict default is one CPU, 2 GiB of memory, and no swap. For elapsed-time comparisons use the same idle host and alternate launch order across repeats. Record TUI observations separately.

This example uses the pinned Pi 1.5.2 reference, a compatible Linux Node.js (22.19+), and an existing authorized ledger. Retain the npm lockfile. Use new run IDs and set the approved individual cap before running.

```sh
pi_ref="$PWD/benchmarks/tools/pi-1.5.2"
npm install --prefix "$pi_ref" --registry=https://registry.npmjs.org --save-exact \
  --ignore-scripts --no-audit --no-fund @earendil-works/pi-coding-agent@1.5.2
cargo build --release --locked -p kuku --example session_task

case_id=large-scale-text-editing
ledger=PATH_TO_AUTHORIZED_LEDGER
keys=PATH_TO_CREDENTIAL_FILE
case_cap=AUTHORIZED_CASE_CAP_USD

paired_run() {
  run_id="$1"
  shift
  python3 scripts/acceptance/run.py --run-id "$run_id" benchmark \
    --suite frontier --case "$case_id" --mode live --provider deepseek --require-resource-evidence \
    --max-output-tokens 131072 --case-cap-usd "$case_cap" --ledger "$ledger" --secrets "$keys" "$@"
}
paired_run vim-kuku --sdk-binary target/release/examples/session_task
paired_run vim-pi --pi-prefix "$pi_ref"

python3 scripts/acceptance/benchmark_compare.py \
  --kuku benchmarks/acceptance/vim-kuku --pi benchmarks/acceptance/vim-pi \
  > benchmarks/acceptance/vim-comparison.json
```

The exporter validates matched inputs before reporting verifier outcomes, time, requests and spend, harness RSS, container peak memory, CPU, and OOM events. Keep missing measurements unknown. Record resource limits, build profiles, SDK hashes, and all attempts with the exported result.

### Strict local Kuku–Codex comparison

Pin an isolated Codex npm install. Place the selected `deepseek-flash` model catalog entry in `deepseek-flash-models.json` inside that prefix. [DeepSeek's Codex integration](https://api-docs.deepseek.com/quick_start/agent_integrations/codex/#option-2-edit-the-configuration-file-manually)

```sh
codex_ref="$PWD/benchmarks/tools/codex-0.164.0"
npm install --prefix "$codex_ref" --registry=https://registry.npmjs.org --save-exact \
  --ignore-scripts --no-audit --no-fund @openai/codex@0.164.0
```

Reuse the paired flags with `--wire openai-responses` on both trials. Choose `--sdk-binary` for Kuku and `--codex-prefix "$codex_ref"` for Codex. Export with `benchmark_compare.py --kuku RUN_DIRECTORY --codex RUN_DIRECTORY`. Record the binary, npm lockfile, and model catalog hashes.

| Compare | Vary one input | Exporter options |
| --- | --- | --- |
| Prompt | `--prompts-dir` | `--baseline RUN_DIRECTORY --candidate RUN_DIRECTORY` |
| Reasoning effort | `--reasoning-effort` on direct DeepSeek | `--compare-effort --baseline RUN_DIRECTORY --candidate RUN_DIRECTORY` |
| SDK build | `--sdk-binary` | `--compare-sdk --baseline RUN_DIRECTORY --candidate RUN_DIRECTORY` |

Match the remaining settings. Pin `--max-command-seconds` and `--max-request-bytes` across a comparison when exercising those limits. Inspect committed capacity rollovers and retained-output references when claiming that a run exercised context recovery. [Paired exporter](../../../scripts/acceptance/benchmark_compare.py)

## Resource measurements

```bash
python3 scripts/acceptance/run.py metrics --profile release
python3 scripts/acceptance/run.py metrics --profile release --cpu-count 1 --address-space-mib 512
```

The [resource caller](../../../crates/kuku/examples/resource_probe.rs) runs public SDK workloads with a controlled provider. It covers idle Agents, queue appends, reopen, output retention, and shared script-worker pressure. Choose the workload from `metrics --help`. On Linux, the optional CPU affinity covers the caller and descendants, and the address-space limit applies to each process. Process sampling is available on Linux and Windows.

Use `caller.json` to compare the recorded build, phase, process limits, and workload. Separate SDK measurements from tool descendants and filesystem bytes. Report preparation timings from durable preparation facts. Keep unsampled values unknown. Check `measurements_complete` as well as the task outcome.

### Long Session profile

```bash
python3 scripts/acceptance/run.py metrics --profile release --long-session-mib 500
```

This synthetic workload makes no paid requests. It builds a large journal and reopens it in another process to verify history, anchors, and rewind behavior. Use a smaller size for fixture development. Compare `fixture.json` and `caller.json` under matching workload, cache conditions, build profile, and process limits. The run retains generated data, so check available disk space.

## Live protocol acceptance

The rewrite's live protocol selection covers direct DeepSeek `deepseek-flash` on Chat Completions, Responses, and Messages, and OpenRouter `openai/gpt-6-luna` on Responses pinned to the `openai` endpoint with fallback disabled. Include a delegated task with independent Agents on the two connections when checking cross-provider delegation. For focused work, select the combinations relevant to the change and authorized for the run. [Delivery stages](../architecture/evolution.md) · [Connection configuration](../reference/config.md)

Cover streaming, a real tool-call/result continuation, multi-turn context, and available settings and usage. Use the lowest supported enabled reasoning effort for protocol-only checks. Gateway coverage applies to the confirmed route. Record provider-reported settings separately from those requested.

Start with the controlled `protocol` suite for streamed argument assembly, replay, and error behavior. Verify call/result identity, ordering and provider-native continuation frames in the protocol adapter. For a chosen live route, verify the actual upstream where available and keep unreported settings or cost unknown. Keep the model, effort, workload, and budget fixed for routing comparisons. Separate cold requests from continued Sessions and measure provider-reported input, cache usage, output, cost, and latency.

For `live`, supply `--config`, `--sdk-result`, `--session-data`, and an SDK/CLI caller after `--`. Pass the credential file with `--secrets` and the authorized ledger with `--ledger`. The selector verifies recorded durable evidence from the caller. Use the [paid-run requirements](#credentials-and-run-artifacts).

## Credentials and run artifacts

Use an explicit private file outside Git. The local convention is `/code/.secrets/kuku-acceptance.env` with directory mode `700` and file mode `600`. `KUKU_TEST_DEEPSEEK_API_KEY` and `KUKU_TEST_OPENROUTER_API_KEY` belong to their respective services. Load credentials without printing them or enabling shell tracing. [Credential configuration](../reference/config.md)

Record authorized spend, request and output limits, timeout, and retry policy before launching a paid attempt. Apply the limits to retries as well. Use the authorized ledger for the service. Separate authorizations use separate ledgers. Initialize a ledger once per authorization and preserve the history.

```bash
python3 scripts/acceptance/run.py budget init --ledger LEDGER \
  --authorization AUTHORIZATION_REF --limit-usd AUTHORIZED_USD --opening-spent-usd 0
python3 scripts/acceptance/run.py budget show --ledger LEDGER
```

`live` and Terminal-Bench live runs require `--reserve-usd` and `--reservation-reason`. The reservation must cover all admitted attempts at conservative prices. Frontier live runs use the host meter and `--case-cap-usd`. Paid `interactive` and `workspace` runs require their explicit credentials and ledger. The calling SDK and relay enforce their own limits. A spend cap alone does not bound output generation.

Failed, unfinished, and unreported attempts keep their full reservation until evidence supports settlement. Missing usage stays unknown. Inspect the evidence before releasing unused reservation and record the settlement basis.

```bash
python3 scripts/acceptance/run.py budget settle --ledger LEDGER --entry RUN_ID \
  --cost-usd AMOUNT --basis provider_reported --evidence PATH_TO_EVIDENCE
```

Use `--basis token_estimate` or `conservative_upper_bound` when appropriate. Check remaining authorization before another request. Give each repeat a new run ID and keep provider payloads, credentials, and Session journals private.
