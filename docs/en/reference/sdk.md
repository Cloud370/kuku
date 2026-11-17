# SDK

`kuku::Session` is the embedding entry point. [`session_task.rs`](../../../crates/kuku/examples/session_task.rs) is an ordinary Rust caller. The product server uses the same SDK and exposes it to terminal, script and browser clients through the [Server API](server-api.md).

## Operations

| Operation | Result |
|---|---|
| `Session::open(SessionOptions).await` | Acquire writer ownership and reconstruct facts without automatic activation |
| `Session::open_for_creation(options, &input, &config).await` | Claim an explicitly named Session for a first-send intent, or reopen that same claim after interrupted creation |
| `create_agent(command_id, address, AgentConfig)` | Durable idempotent creation receipt |
| `configure_agent(command_id, address, AgentConfig)` | Agent-local defaults for the next safe boundary |
| `send(Input)` / `advance(address)` | Durable delivery and explicit activation |
| `send_configured(Input, AgentConfig)` | Commit delivery and configuration together with a correlation that identifies the configuration intent |
| `agent(address)` / `agents()` | Query derived snapshots without consuming output |
| `agent_view(address)` | Selected Agent snapshot, queued inputs and a matching committed cursor |
| `agent_selection(address)` | Inspect effective model settings, capabilities and final tool schemas without dispatch |
| `receipt(command_id)` / `input_receipt(&Input)` | Query and verify an earlier acknowledgement without resuming work |
| `input_status(address, input_command_id)` | Query whether an input is queued, accepted or withdrawn |
| `withdraw_input(command_id, address, input_command_id)` | Withdraw an input if it remains queued and return its status |
| `advance_command(command_id, address)` | Idempotent explicit advancement |
| `reply_permission_receipt(...)` / `reply_question_receipt(...)` | Verify an earlier reply acknowledgement without resuming work |
| `events(after, limit)` / `observe(after)` | Bounded pages and reconnectable observation |
| `read_output(output_id, &OutputRead)` | Bounded range, literal search, or incremental output |
| `reply_permission(command_id, address, request_id, allow)` | Exact pending-action reply |
| `reply_question(command_id, address, request_id, text)` | Exact question reply delivered as correlated input |
| `cancel(address).await` / `close().await` | Await owned cleanup and durable settlement |
| `cancel_execution(command_id, address, execution_id).await` | Cancel the particular observed execution and await its owned cleanup |
| `Session::read_agents(events_path)` | Offline recovery snapshots without runtime resources |
| `Session::read_view(events_path, address)` / `Session::read_receipt(events_path, command_id)` | Read the selected Agent view or command receipt from a closed Session |

`SessionOptions::new(workspace, data_dir, config)` accepts named-provider configuration. Options add prompt assets, resource limits, and a host `ToolAuthorizer`. `AgentConfig` selects preset, tier, named connection, model, reasoning, service tier, output tokens, instructions, directory, and native skill defaults. Connection selection is independent of wire protocol. An explicit model overrides the selected tier’s model while retaining its other defaults. `AgentConfig.tools` overrides the preset’s final tool selection; collaboration tools are not added to a read-only preset. Preset instructions, tier and tool defaults are resolved and stored when creating or configuring the Agent. Later preset-file changes do not change those retained defaults. Lifecycle and receipt semantics live in [Sessions](../how-it-works/sessions.md).

`Input.timing` distinguishes `InputTiming::NextStep`, the default, from `InputTiming::NextTurn`. `AgentSnapshot.execution_id` identifies the current execution. `Observation::Live` carries its source Agent, execution identity, Turn and latest committed cursor when the update was published. Use `agent_view` to start observation at the cursor that matches the selected Agent state.

`AgentSnapshot.config` is the requested configuration, and `active_config` records the configuration used by active or waiting execution. Model and reasoning settings can change at the next unprepared model request. Workspace, behavior and tool settings take effect on the next Turn.

When exposing retryable creation over a transport, `open_for_creation` checks the initial input, configuration and optional Prompt directory against the persistent claim. The host still creates the Agent and submits its input using stable command identities. Use `verify_creation(&input, &config, prompts_dir)` if the claimed Session is already open.

`Session::read_agents` reconstructs offline recovery state from the journal and maps an unterminated `Running` state to `Interrupted`, even while another runtime is active. Use an open Session's `agent` or `agents` methods for its current live state.

## Host adapters

Set `SessionOptions.model_adapter` to an `Arc<dyn ModelAdapter>` to supply a model transport, and append `Arc<dyn ToolAdapter>` values to `SessionOptions.tools` for additional tools. The [external-caller tests](../../../crates/kuku/tests/adapter_runtime.rs) exercise both through the public SDK with real file actions, host denial, retained output and awaited cancellation cleanup.

A `ModelAdapter` receives the frozen `ModelRequest`: selected connection, protocol, endpoint, model, native settings, correlation and the exact prepared body, without authentication headers. It returns a `ModelStream` of `ModelEvent` values. Configuration selection and `agent_selection` do not resolve environment credentials; the built-in HTTP transport resolves them immediately before sending. Custom transports own authentication. The SDK retains the same preparation barrier, request quota, model capacity, response validation, usage and journal path. Dropping the adapter's pending future or returned stream must release its transport resources.

A `ToolAdapter` supplies an immutable `ToolSpec` at open and executes the final `ToolAuthorization` with a `ToolCancellation` signal. Built-in names cannot be replaced. Presets and `AgentConfig.tools` select from the combined available definitions. Every invocation passes the existing host authorizer and durable preparation/dispatch boundary. Declared writes and external actions join the Agent’s ordered workspace action group; declared reads stay outside that group and may overlap other tools. The tool must clean up owned work before returning, including after cancellation; the SDK waits for that return. A known incomplete cleanup is returned as `ToolOutput.cleanup_incomplete` and makes `cancel` or `close` fail.

Tool content uses the command output format and public `read_output` fallback. The first model view includes a complete reference and truncation flags and is bounded by `ToolSpec.max_result_bytes` and the runtime preview limit. Oversized structured data is retained in that output instead of the journal. Returned buffers are allocated by the host adapter; the SDK's retained capture and model view have separate limits.

## Calling program

```bash
cargo run -p kuku --example session_task -- \
  --workspace /path/to/work --prompt-file /path/to/task.txt \
  --config /path/to/config.toml --data-dir /path/to/session-data \
  --connection deepseek --reasoning-effort low \
  --max-steps 12 --max-requests 32 --max-output-tokens 32768 --max-context-tokens 131072 \
  --output /path/to/result.json
```

Use `--session-id` to reopen and `--command-id` to retry delivery after losing a receipt. Results include durable model/tool/turn fact counts, journal path/cursor, provider settings/reports, limits, duration, and cleanup. `runtime_completed` means execution settled; a benchmark still needs its actual judge. `--max-requests` limits all Agent requests across this open runtime; it defaults to `--max-steps`. `--max-agents` sets the retained Agent limit. Automatic paid retries are disabled. Serialized request bytes are capped by `--max-context-tokens`, a conservative input-token bound.

## Limits and accounting

Defaults: 64 agents, 128 pending inputs per agent, 65,536 input bytes, 512 KiB per prepared request, four simultaneous model streams, 20 requests per turn, eight tools per agent, 8 MiB per retained output, 16 KiB first preview, and 128 transient observations. `RuntimeLimits.max_output_tokens_per_request` optionally caps every Agent’s resolved output tokens; explicit Agent values are reduced with `min` and selection/prepared facts expose that effective value. `session_task --max-output-tokens` sets this host ceiling as well as the primary Agent’s selection. `RuntimeLimits.max_requests_per_open` optionally caps total attempts across all Agent turns, resets on reopen, and rejects excess work before dispatch; its default is `None`. Zero or unsupported bounds fail validation; output previews require at least 1 KiB for a complete reference and flags.

`RuntimeLimits.retry_transient_provider_errors` defaults to true. The SDK can retry once after a retryable transport or provider-unavailable error before the stream opens. It reuses the prepared request and charges the retry to the same request budget. Set the option to false to disable this retry, as the bounded `session_task` caller does.

Journal records are capped at 1 MiB. Pages contain at most 256 records and stop at a 1 MiB target, with one-record overrun below 2 MiB. Output reads return at most 80,000 bytes. The retained-output quota includes framing/metadata and marks incomplete capture while continuing to drain. These are per-output bounds; hosts choose session retention and total disk capacity.

`UsageTotals` retains absent counters as `None`; `unreported_requests` identifies incomplete input/output reporting. Input totals include cached input, so cache counters are subsets. Raw optional provider reports remain separate facts. Known incomplete cleanup makes `cancel` or `close` fail.

## Controlled discussion and timer

[`session_orchestrate.rs`](../../../crates/kuku/examples/session_orchestrate.rs) is a small host for two independent Agents in one Session. It admits exactly three rounds: a writer draft, a reviewer response using the selected writer result, and a writer continuation after closing, reopening, and waiting for a host timer. The reviewer receives its own instructions and the selected result; the writer retains its own history across reopen.

Provide the task, reviewer instructions, and continuation instructions in three text files. For example, ask the writer to inspect and repair a small program and run its tests, ask the reviewer to inspect the shared files, then ask the writer to apply the selected review and check the result. Use named connections from your [configuration](config.md), with credentials supplied through environment references.

```bash
cargo run -p kuku --example session_orchestrate -- \
  --workspace /path/to/work --config /path/to/config.toml \
  --prompt-file /path/to/draft.txt \
  --review-prompt-file /path/to/review.txt \
  --continue-prompt-file /path/to/continue.txt \
  --writer-connection direct --writer-tier balanced \
  --reviewer-connection openrouter-openai-responses --reviewer-tier light \
  --max-steps 8 --max-requests 16 --timer-ms 250 \
  --max-output-tokens 32768 --max-context-tokens 131072 \
  --data-dir /path/to/session-data --output /path/to/result.json
```

The host sends the first two inputs with `Activation::Enqueue`, explicitly advances each recipient, and delivers the timer input as `InputSource::External("timer")` with a correlation. Every delivery has a stable command ID and verifies an identical retry's receipt. The example owns two Agents, disables model-created Agents and Skills, and caps requests separately for each of its two opens. Reopening renews that host-selected request allowance. Output and context defaults remain 32,768 and 131,072 tokens.

The host owns the timer lifetime and starts it after reopen; the Session persists the delivered input. Observers are drained after execution settles, so subscription consumption cannot drive the work. JSON results retain per-round receipts and snapshots, provider and tool facts, bounded retained command output, dormant request counters, and close/reopen evidence. `runtime_completed` requires all three rounds to settle idle with no pending input and a successful durable close; verify task artifacts separately. A waiting Agent is reported as incomplete.
